#!/usr/bin/env python3
"""LSP サーバの比較: crustc lsp / rust-analyzer / rust-glancer を同じ手順で動かして測る。

usage: python3 bench/lsp_compare.py [--servers crustc,rust-analyzer,rust-glancer] [--targets synth,crustc]
                                    [--out bench/lsp_compare_result.md] [--reps 10] [--timeout 300]
(このディレクトリで実行。事前に cargo build --release。rust-glancer は RUST_GLANCER=<バイナリ> か PATH から)

測るもの (サーバごと・対象ごと):
- init: initialize の応答までの時間
- ready: 起動から、workspace/symbol が対象の既知のシンボルを返すまでの時間 (索引ができるまで)。0.1 秒ごとに問い合わせる
- docsym: textDocument/documentSymbol の応答の時間 (中央値) とシンボルの数 (木を平らにした数)
- wsym: workspace/symbol の応答の時間 (いくつかの問い合わせの中央値) と、既知の名前が見つかったか
- edit→docsym: didChange (関数を 1 つ足す) の直後の documentSymbol の時間と、足した関数が出たか
- edit→diag: didChange から publishDiagnostics (そのドキュメントの) が来るまで (来なければ -)
- peak RSS: サーバのプロセスの VmHWM と、子孫のプロセス (cargo metadata・proc-macro-srv など) を足した RSS の最大 (50 ms ごとに見る)

サーバから来る要求 (workspace/configuration・window/workDoneProgress/create・client/registerCapability など) には空で答える。
対象:
- synth: 生成した Cargo のクレート (モジュール 40 個・各 25 関数 + struct / enum / impl)
- crustc: このディレクトリ (Cargo のワークスペース。依存に cranelift などがある)
"""

import argparse, json, os, shutil, statistics, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def uri(p):
    return "file://" + os.path.abspath(p)


class Lsp:
    def __init__(self, cmd, root, env=None):
        self.p = subprocess.Popen(cmd, cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env)
        self.next_id = 1
        self.lock = threading.Lock()
        self.waiting = {}
        self.diags = []  # (時刻, uri, version)
        self.alive = True
        self.tree_peak = 0
        threading.Thread(target=self._read, daemon=True).start()
        threading.Thread(target=self._sample, daemon=True).start()

    def _sample(self):
        """サーバと子孫のプロセスの RSS の合計の最大 (50 ms ごと)"""
        while self.p.poll() is None:
            kids = {}
            for d in os.listdir("/proc"):
                if d.isdigit():
                    try:
                        st = open(f"/proc/{d}/stat").read()
                        kids.setdefault(int(st[st.rindex(")") + 2:].split()[1]), []).append(int(d))
                    except (OSError, ValueError):
                        pass
            tot, todo = 0, [self.p.pid]
            while todo:
                q = todo.pop()
                try:
                    for l in open(f"/proc/{q}/status"):
                        if l.startswith("VmRSS:"):
                            tot += int(l.split()[1])
                except OSError:
                    pass
                todo += kids.get(q, [])
            self.tree_peak = max(self.tree_peak, tot)
            time.sleep(0.05)

    def _send(self, msg):
        b = json.dumps(msg).encode()
        try:
            self.p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
            self.p.stdin.flush()
        except (BrokenPipeError, OSError):
            self.alive = False

    def _read(self):
        f = self.p.stdout
        while True:
            n = None
            while True:
                line = f.readline()
                if not line:
                    self.alive = False
                    for ev in self.waiting.values():
                        ev[0].set()
                    return
                line = line.strip()
                if not line:
                    break
                if line.lower().startswith(b"content-length:"):
                    n = int(line.split(b":")[1])
            body = f.read(n)
            try:
                m = json.loads(body)
            except Exception:
                continue
            if "id" in m and "method" in m:
                # サーバからの要求: 空で答える
                meth = m["method"]
                if meth == "workspace/configuration":
                    res = [None for _ in m.get("params", {}).get("items", [])]
                else:
                    res = None
                self._send({"jsonrpc": "2.0", "id": m["id"], "result": res})
            elif "id" in m:
                with self.lock:
                    ev = self.waiting.get(m["id"])
                if ev:
                    ev[1].append(m)
                    ev[0].set()
            elif m.get("method") == "textDocument/publishDiagnostics":
                pr = m.get("params", {})
                self.diags.append((time.time(), pr.get("uri"), pr.get("version")))

    def request(self, method, params, timeout=60):
        with self.lock:
            i = self.next_id
            self.next_id += 1
            ev = (threading.Event(), [])
            self.waiting[i] = ev
        self._send({"jsonrpc": "2.0", "id": i, "method": method, "params": params})
        ok = ev[0].wait(timeout)
        with self.lock:
            self.waiting.pop(i, None)
        if not ok or not ev[1]:
            return None
        return ev[1][0]

    def notify(self, method, params):
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def peak_rss_mb(self):
        try:
            for l in open(f"/proc/{self.p.pid}/status"):
                if l.startswith("VmHWM:"):
                    return int(l.split()[1]) / 1024
        except OSError:
            pass
        return None

    def close(self):
        if self.alive:
            self.request("shutdown", None, timeout=5)
            self.notify("exit", None)
        try:
            self.p.wait(5)
        except subprocess.TimeoutExpired:
            self.p.kill()


def flat_count(syms):
    n = 0
    for s in syms or []:
        n += 1
        n += flat_count(s.get("children"))
    return n


def flat_names(syms):
    out = []
    for s in syms or []:
        out.append(s.get("name"))
        out += flat_names(s.get("children"))
    return out


# ---------------------------------------------------------------- 対象


def make_synth(dir):
    """生成した Cargo のクレート: src/main.rs + src/m{k}.rs (40 個)"""
    os.makedirs(os.path.join(dir, "src"), exist_ok=True)
    open(os.path.join(dir, "Cargo.toml"), "w").write('[package]\nname = "synth"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n')
    mods = 40
    for k in range(mods):
        lines = [f"//! module {k}", "", f"pub struct Point{k} {{ pub x: i64, pub y: i64 }}", "",
                 f"pub enum Shape{k} {{ Circle(i64), Rect {{ w: i64, h: i64 }}, Empty }}", "",
                 f"impl Point{k} {{", f"    pub fn new(x: i64, y: i64) -> Point{k} {{ Point{k} {{ x, y }} }}",
                 f"    pub fn norm{k}(&self) -> i64 {{ self.x * self.x + self.y * self.y }}", "}", ""]
        for j in range(25):
            prev = f"f{k}_{j - 1}(n + 1)" if j > 0 else "n"
            lines += [f"pub fn f{k}_{j}(n: i64) -> i64 {{", f"    let p = Point{k}::new(n, {j});", f"    p.norm{k}() + {prev}", "}", ""]
        open(os.path.join(dir, "src", f"m{k}.rs"), "w").write("\n".join(lines))
    main = [f"mod m{k};" for k in range(mods)] + ["", "fn main() {", "    let mut s = 0;"]
    main += [f"    s += m{k}::f{k}_0({k});" for k in range(mods)]
    main += ['    println!("{s}");', "}", ""]
    open(os.path.join(dir, "src", "main.rs"), "w").write("\n".join(main))
    return {
        "root": dir,
        "open": "src/m7.rs",
        "ready": ("norm31", "norm31"),
        "queries": ["norm1", "Point2", "f39_24", "Shape", "rect"],
        "expect_doc": "norm7",
        "edit_after": "pub enum Shape7",
    }


def crustc_target():
    return {
        "root": HERE,
        "open": "crates/core/src/driver2.rs",
        "ready": ("build_workspace", "build_workspace"),
        "queries": ["Session2", "check", "split_inc", "Recorder", "lower"],
        "expect_doc": "check",
        "edit_after": "pub struct Diagnostic",
    }


# ---------------------------------------------------------------- 測る


def bench(name, cmd, t, reps, timeout, env=None):
    r = {"server": name}
    root = t["root"]
    path = os.path.join(root, t["open"])
    text = open(path).read()
    u = uri(path)
    t0 = time.time()
    s = Lsp(cmd, root, env)
    caps = {
        "textDocument": {"documentSymbol": {"hierarchicalDocumentSymbolSupport": True}, "publishDiagnostics": {"versionSupport": True},
                         "synchronization": {"didSave": True}},
        "workspace": {"symbol": {}, "configuration": True, "workspaceFolders": True},
        "window": {"workDoneProgress": True},
    }
    init = s.request("initialize", {"processId": os.getpid(), "rootUri": uri(root), "rootPath": root, "capabilities": caps,
                                    "workspaceFolders": [{"uri": uri(root), "name": os.path.basename(root)}]}, timeout=timeout)
    r["init_ms"] = (time.time() - t0) * 1000 if init else None
    if not init:
        s.close()
        r["error"] = "no initialize response"
        return r
    s.notify("initialized", {})
    s.notify("textDocument/didOpen", {"textDocument": {"uri": u, "languageId": "rust", "version": 1, "text": text}})
    # 索引ができるまで: 既知の名前が workspace/symbol に出るまで
    q, want = t["ready"]
    r["ready_ms"] = None
    while time.time() - t0 < timeout and s.alive:
        m = s.request("workspace/symbol", {"query": q}, timeout=timeout)
        res = (m or {}).get("result") or []
        if any(x.get("name") == want for x in res):
            r["ready_ms"] = (time.time() - t0) * 1000
            break
        time.sleep(0.1)
    # documentSymbol
    ts = []
    n = None
    names = []
    for _ in range(reps):
        a = time.time()
        m = s.request("textDocument/documentSymbol", {"textDocument": {"uri": u}}, timeout=60)
        ts.append((time.time() - a) * 1000)
        res = (m or {}).get("result")
        if res is not None:
            n = flat_count(res)
            names = flat_names(res)
    r["docsym_ms"] = statistics.median(ts)
    r["docsym_n"] = n
    r["docsym_ok"] = t["expect_doc"] in names
    # workspace/symbol
    ts = []
    found = 0
    for q in t["queries"]:
        for _ in range(max(1, reps // 2)):
            a = time.time()
            m = s.request("workspace/symbol", {"query": q}, timeout=60)
            ts.append((time.time() - a) * 1000)
        res = (m or {}).get("result") or []
        if any(q.lower() in (x.get("name") or "").lower() for x in res):
            found += 1
    r["wsym_ms"] = statistics.median(ts)
    r["wsym_found"] = f"{found}/{len(t['queries'])}"
    # 編集: 関数を 1 つ足す
    at = text.find(t["edit_after"])
    at = text.rfind("\n", 0, at) + 1 if at >= 0 else len(text)
    new = text[:at] + "pub fn added_by_bench() -> u32 { 1 }\n\n" + text[at:]
    nd = len(s.diags)
    a = time.time()
    s.notify("textDocument/didChange", {"textDocument": {"uri": u, "version": 2}, "contentChanges": [{"text": new}]})
    m = s.request("textDocument/documentSymbol", {"textDocument": {"uri": u}}, timeout=60)
    r["edit_docsym_ms"] = (time.time() - a) * 1000
    r["edit_docsym_ok"] = "added_by_bench" in flat_names((m or {}).get("result"))
    r["edit_diag_ms"] = None
    while time.time() - a < 20:
        hit = [d for d in s.diags[nd:] if d[1] == u]
        if hit:
            r["edit_diag_ms"] = (hit[0][0] - a) * 1000
            break
        time.sleep(0.01)
    r["peak_rss_mb"] = s.peak_rss_mb()
    r["tree_rss_mb"] = s.tree_peak / 1024
    s.close()
    return r


def fmt(x, unit=""):
    if x is None:
        return "-"
    if isinstance(x, bool):
        return "yes" if x else "no"
    if isinstance(x, float):
        return f"{x:.1f}{unit}" if x < 100 else f"{x:.0f}{unit}"
    return f"{x}{unit}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--servers", default="crustc,rust-analyzer,rust-glancer")
    ap.add_argument("--targets", default="synth,crustc")
    ap.add_argument("--reps", type=int, default=10)
    ap.add_argument("--timeout", type=float, default=300)
    ap.add_argument("--out", default=None)
    ap.add_argument("--cold", action="store_true", help="rust-glancer の索引 (target/rust_glancer) を消してから測る")
    a = ap.parse_args()
    glancer = os.environ.get("RUST_GLANCER") or shutil.which("rust-glancer") or "/home/user/rust-glancer/target/release/rust-glancer"
    servers = {
        "crustc": [os.path.join(HERE, "target/release/crustc"), "lsp"],
        "rust-analyzer": [os.environ.get("RUST_ANALYZER") or "rust-analyzer"],
        "rust-glancer": [glancer, "lsp"],
    }
    tmp = tempfile.mkdtemp(prefix="lsp_compare_")
    targets = {}
    for t in a.targets.split(","):
        if t == "synth":
            targets["synth"] = make_synth(os.path.join(tmp, "synth"))
        elif t == "crustc":
            targets["crustc"] = crustc_target()
    out = ["| 対象 | サーバ | init | ready | docsym (数) | wsym (見つかった) | edit→docsym | edit→diag | peak RSS (子も) |",
           "|---|---|---:|---:|---:|---:|---:|---:|---:|"]
    for tn, t in targets.items():
        for sn in a.servers.split(","):
            cmd = servers[sn]
            if not (os.path.exists(cmd[0]) or shutil.which(cmd[0])):
                print(f"skip {sn}: {cmd[0]} not found", file=sys.stderr)
                continue
            if a.cold and sn == "rust-glancer":
                shutil.rmtree(os.path.join(t["root"], "target", "rust_glancer"), ignore_errors=True)
            print(f"== {tn} / {sn}", file=sys.stderr)
            r = bench(sn, cmd, t, a.reps, a.timeout)
            print(json.dumps(r), file=sys.stderr)
            if r.get("error"):
                out.append(f"| {tn} | {sn} | {r['error']} | | | | | | |")
                continue
            ds = f"{fmt(r['docsym_ms'], ' ms')} ({fmt(r['docsym_n'])}{'' if r['docsym_ok'] else ', 期待の名前なし'})"
            ed = f"{fmt(r['edit_docsym_ms'], ' ms')}{'' if r['edit_docsym_ok'] else ' (足した関数なし)'}"
            out.append(f"| {tn} | {sn} | {fmt(r['init_ms'], ' ms')} | {fmt(r['ready_ms'], ' ms')} | {ds} | {fmt(r['wsym_ms'], ' ms')} ({r['wsym_found']}) | "
                       f"{ed} | {fmt(r['edit_diag_ms'], ' ms')} | {fmt(r['peak_rss_mb'], ' MB')} ({fmt(r['tree_rss_mb'], ' MB')}) |")
    print("\n".join(out))
    if a.out:
        open(a.out, "w").write("\n".join(out) + "\n")
    shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
