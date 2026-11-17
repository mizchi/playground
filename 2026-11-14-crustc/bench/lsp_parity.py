#!/usr/bin/env python3
"""LSP の機能の比較: crustc-lsp / rust-analyzer / rust-glancer が、同じ問い合わせに同じ答えを返すか。
速さの比較 (lsp_compare.py) は「何を返すか」を見ないので、機能の不足と答えの違いをここで見る。rust-analyzer を正解とする。

(事前に cargo build --release --workspace: target/release/crustc-lsp を使う)
usage: python3 bench/lsp_parity.py [--servers crustc,rust-analyzer,rust-glancer] [--targets synth,crustc]
                                   [--points 40] [--out bench/lsp_parity_result.md]

見るもの (対象ごと・サーバごと):
- caps: initialize が返した機能 (documentSymbol / workspaceSymbol / definition / hover / completion / references / ...)
- docsym: 開いたファイルの documentSymbol の (名前, 種類) の集合が rust-analyzer と何割同じか (足りない・余分の例)
- wsym: 問い合わせごとの上位 20 個の名前の集合の一致 (Jaccard)
- definition: ファイルの中の識別子 (ワークスペースで定義された名前) の位置で、行き先 (ファイル, 行) が rust-analyzer と同じ割合
- hover: 同じ位置で、空でない hover を返した割合
- completion: `.` の直後 (`x.|name`) と、識別子の 2 文字目の後 (`na|me`) で、そこに実際に書いてある名前が候補に入る割合と候補の数
- references: 同じ位置で、参照の数が rust-analyzer と同じ割合
各問い合わせの時間の中央値も出す。
"""

import argparse, json, os, re, shutil, statistics, sys, tempfile, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lsp_compare import HERE, Lsp, make_synth, crustc_target, uri  # noqa: E402

CAPS = [("documentSymbolProvider", "docsym"), ("workspaceSymbolProvider", "wsym"), ("definitionProvider", "definition"),
        ("hoverProvider", "hover"), ("completionProvider", "completion"), ("referencesProvider", "references"),
        ("renameProvider", "rename"), ("signatureHelpProvider", "signature"), ("semanticTokensProvider", "semantic"),
        ("inlayHintProvider", "inlay"), ("codeActionProvider", "codeAction"), ("documentFormattingProvider", "format")]


def start(cmd, t, timeout):
    root = t["root"]
    s = Lsp(cmd, root)
    caps = {
        "textDocument": {
            "documentSymbol": {"hierarchicalDocumentSymbolSupport": True},
            "completion": {"completionItem": {"snippetSupport": False}},
            "hover": {"contentFormat": ["plaintext", "markdown"]},
            "publishDiagnostics": {"versionSupport": True},
        },
        "workspace": {"symbol": {}, "configuration": True, "workspaceFolders": True},
        "window": {"workDoneProgress": True},
    }
    init = s.request("initialize", {"processId": os.getpid(), "rootUri": uri(root), "rootPath": root, "capabilities": caps,
                                    "workspaceFolders": [{"uri": uri(root), "name": os.path.basename(root)}]}, timeout=timeout)
    if not init:
        s.close()
        return None, {}
    s.notify("initialized", {})
    path = os.path.join(root, t["open"])
    s.notify("textDocument/didOpen", {"textDocument": {"uri": uri(path), "languageId": "rust", "version": 1, "text": open(path).read()}})
    # (索引ができるまで待つ)
    q, want = t["ready"]
    t0 = time.time()
    while time.time() - t0 < timeout and s.alive:
        m = s.request("workspace/symbol", {"query": q}, timeout=timeout)
        if any(x.get("name") == want for x in ((m or {}).get("result") or [])):
            break
        time.sleep(0.2)
    return s, (init.get("result") or {}).get("capabilities") or {}


def req(s, method, params, times):
    a = time.time()
    m = s.request(method, params, timeout=30)
    times.setdefault(method, []).append((time.time() - a) * 1000)
    if m is None or "error" in m:
        return None
    return m.get("result")


def sym_set(res):
    out = set()

    def go(v):
        for x in v or []:
            out.add((x.get("name"), x.get("kind")))
            go(x.get("children"))

    go(res)
    return out


def norm_name(n):
    # (impl の名前の書き方はサーバで違う: `impl Tr for S` / `impl Tr for S<T>` / `S`。名前だけで比べるときは impl を外す)
    return re.sub(r"^impl\s+", "", n or "")


def loc_of(res):
    """definition の結果 → (ファイル名, 行) の集合"""
    if res is None:
        return set()
    if isinstance(res, dict):
        res = [res]
    out = set()
    for x in res:
        u = x.get("uri") or x.get("targetUri")
        r = x.get("range") or x.get("targetSelectionRange") or x.get("targetRange")
        if u and r:
            out.add((u.rsplit("/", 1)[-1], r["start"]["line"]))
    return out


def comp_labels(res):
    if res is None:
        return []
    items = res.get("items", []) if isinstance(res, dict) else res
    return [re.split(r"[(<\s]", (x.get("filterText") or x.get("label") or "").strip())[0] for x in items]


def points(text, names, n):
    """ファイルの中の、names のどれかの名前の識別子の位置 (行, 桁) を n 個まで (均等に)"""
    lines = text.split("\n")
    pts = []
    for li, l in enumerate(lines):
        if l.lstrip().startswith("//"):
            continue
        for m in re.finditer(r"\b[A-Za-z_][A-Za-z0-9_]*\b", l):
            if m.group(0) in names:
                pts.append((li, m.start(), m.group(0)))
    if len(pts) > n:
        step = len(pts) / n
        pts = [pts[int(i * step)] for i in range(n)]
    return pts


def dot_points(text, n):
    """`x.name` の `.` の直後の位置と name"""
    pts = []
    for li, l in enumerate(text.split("\n")):
        if l.lstrip().startswith("//"):
            continue
        for m in re.finditer(r"[A-Za-z0-9_)\]]\.([a-z_][A-Za-z0-9_]*)", l):
            pts.append((li, m.start(1), m.group(1)))
    if len(pts) > n:
        step = len(pts) / n
        pts = [pts[int(i * step)] for i in range(n)]
    return pts


def run(servers, t, npts, timeout):
    path = os.path.join(t["root"], t["open"])
    text = open(path).read()
    u = uri(path)
    # (問い合わせる位置: ワークスペースで定義された名前の出てくる所)
    names = set(re.findall(r"\b(?:fn|struct|enum|trait|type|const|static)\s+([A-Za-z_][A-Za-z0-9_]*)", text))
    names |= set(n for n in re.findall(r"\b([A-Z][A-Za-z0-9_]+)\b", text))
    pts = points(text, names, npts)
    dpts = dot_points(text, npts // 2)
    res = {}
    for sn, cmd in servers.items():
        print(f"== {sn}", file=sys.stderr)
        s, caps = start(cmd, t, timeout)
        if s is None:
            res[sn] = None
            continue
        times = {}
        r = {"caps": {k: bool(caps.get(c)) for c, k in CAPS}, "times": times}
        r["docsym"] = sym_set(req(s, "textDocument/documentSymbol", {"textDocument": {"uri": u}}, times))
        r["wsym"] = {}
        for q in t["queries"]:
            v = req(s, "workspace/symbol", {"query": q}, times) or []
            r["wsym"][q] = set(norm_name(x.get("name")) for x in v[:20])
        r["def"], r["hover"], r["refs"] = [], [], []
        for (li, col, name) in pts:
            p = {"textDocument": {"uri": u}, "position": {"line": li, "character": col + 1}}
            r["def"].append(loc_of(req(s, "textDocument/definition", p, times)))
            h = req(s, "textDocument/hover", p, times)
            hv = json.dumps((h or {}).get("contents", "")) if h else ""
            r["hover"].append(bool(h) and name in hv)
            rf = req(s, "textDocument/references", dict(p, context={"includeDeclaration": True}), times)
            r["refs"].append(len(rf) if isinstance(rf, list) else None)
        r["dot"], r["pre"], r["dot_n"], r["pre_n"], r["dot_at"], r["pre_at"] = [], [], [], [], [], []
        for (li, col, name) in dpts:
            c = comp_labels(req(s, "textDocument/completion", {"textDocument": {"uri": u}, "position": {"line": li, "character": col}}, times))
            r["dot"].append(name in c)
            r["dot_at"].append((li, col, name))
            r["dot_n"].append(len(c))
        for (li, col, name) in pts:
            if len(name) < 4:
                continue
            c = comp_labels(req(s, "textDocument/completion", {"textDocument": {"uri": u}, "position": {"line": li, "character": col + 2}}, times))
            r["pre"].append(name in c)
            r["pre_at"].append((li, col, name))
            r["pre_n"].append(len(c))
        s.close()
        res[sn] = r
    return res, len(pts), len(dpts), pts


def pct(a, b):
    return f"{100 * a / b:.0f}%" if b else "-"


def detail(tn, res, pts):
    """--detail: rust-analyzer と違った所 (definition の行き先・references の数)"""
    ref = res.get("rust-analyzer")
    if not ref:
        return
    for sn, r in res.items():
        if sn == "rust-analyzer" or not r:
            continue
        for i, (li, col, name) in enumerate(pts):
            a, b = r["def"][i], ref["def"][i]
            ra, rb = r["refs"][i], ref["refs"][i]
            if (b and a != b) or (rb is not None and ra != rb):
                print(f"[{tn}/{sn}] {li + 1}:{col + 1} {name}: def {sorted(a)} vs ra {sorted(b)}; refs {ra} vs ra {rb}", file=sys.stderr)
            if ref["hover"][i] and not r["hover"][i]:
                print(f"[{tn}/{sn}] {li + 1}:{col + 1} {name}: hover なし", file=sys.stderr)
        # (completion: rust-analyzer は候補に入れたが、こちらは入れなかった所)
        for key in ["dot", "pre"]:
            for i, (li, col, name) in enumerate(r.get(key + "_at", [])):
                if i < len(ref[key]) and ref[key][i] and not r[key][i]:
                    print(f"[{tn}/{sn}] {li + 1}:{col + 1} {name}: completion ({key}) に入らない (候補 {r[key + '_n'][i]})", file=sys.stderr)


def report(tn, res, npts, ndots):
    ref = res.get("rust-analyzer")
    out = [f"#### {tn} ({npts} 箇所、`.` の後 {ndots} 箇所。比べる相手は rust-analyzer)", ""]
    sv = [k for k in res if res[k]]
    out.append("| 機能 | " + " | ".join(sv) + " |")
    out.append("|---|" + "---|" * len(sv))
    out.append("| 対応を返す機能 | " + " | ".join(", ".join(k for k, v in res[s]["caps"].items() if v) or "-" for s in sv) + " |")
    rows = []
    def row(label, f):
        rows.append(f"| {label} | " + " | ".join(f(res[s]) for s in sv) + " |")
    if ref:
        def docsym(r):
            a, b = r["docsym"], ref["docsym"]
            an = set(norm_name(n) for n, _ in a)
            bn = set(norm_name(n) for n, _ in b)
            miss = sorted(bn - an)[:4]
            extra = sorted(an - bn)[:4]
            same_kind = len(a & b)
            return f"名前 {pct(len(an & bn), len(bn))} (種類も {pct(same_kind, len(b))})" + (f"; 足りない {', '.join(miss)}" if miss else "") + (f"; 余分 {', '.join(extra)}" if extra else "")
        row("documentSymbol (名前の一致)", docsym)
        def wsym(r):
            js = []
            for q, v in ref["wsym"].items():
                w = r["wsym"].get(q, set())
                js.append(len(v & w) / len(v | w) if v | w else 1.0)
            return f"{100 * statistics.mean(js):.0f}%"
        row("workspace/symbol (上位 20 の一致)", wsym)
        def defs(r):
            ok = sum(1 for a, b in zip(r["def"], ref["def"]) if b and a == b)
            ans = sum(1 for a in r["def"] if a)
            return f"同じ {pct(ok, sum(1 for b in ref['def'] if b))} (答えた {pct(ans, len(r['def']))})"
        row("definition (行き先の一致)", defs)
        row("hover (名前を含む)", lambda r: pct(sum(r["hover"]), len(r["hover"])))
        def refs(r):
            ok = sum(1 for a, b in zip(r["refs"], ref["refs"]) if b is not None and a == b)
            return f"{pct(ok, sum(1 for b in ref['refs'] if b is not None))}"
        row("references (数の一致)", refs)
        row("completion `x.\\|` (書いてある名前を含む)", lambda r: f"{pct(sum(r['dot']), len(r['dot']))} (候補 {statistics.median(r['dot_n']) if r['dot_n'] else 0:.0f})")
        row("completion `na\\|me` (同)", lambda r: f"{pct(sum(r['pre']), len(r['pre']))} (候補 {statistics.median(r['pre_n']) if r['pre_n'] else 0:.0f})")
    def tm(r):
        ks = ["textDocument/definition", "textDocument/hover", "textDocument/completion", "textDocument/references"]
        return ", ".join(f"{k.split('/')[1][:4]} {statistics.median(r['times'][k]):.1f}" for k in ks if r["times"].get(k))
    row("時間の中央値 (ms)", tm)
    out += rows
    return "\n".join(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--servers", default="crustc,rust-analyzer,rust-glancer")
    ap.add_argument("--targets", default="synth,crustc")
    ap.add_argument("--points", type=int, default=40)
    ap.add_argument("--timeout", type=float, default=300)
    ap.add_argument("--out", default=None)
    ap.add_argument("--detail", action="store_true", help="rust-analyzer と違った所を stderr に出す")
    a = ap.parse_args()
    glancer = os.environ.get("RUST_GLANCER") or shutil.which("rust-glancer") or "/home/user/rust-glancer/target/release/rust-glancer"
    allsv = {"crustc": [os.path.join(HERE, "target/release/crustc-lsp")], "rust-analyzer": [os.environ.get("RUST_ANALYZER") or "rust-analyzer"], "rust-glancer": [glancer, "lsp"]}
    servers = {k: allsv[k] for k in a.servers.split(",") if os.path.exists(allsv[k][0]) or shutil.which(allsv[k][0])}
    tmp = tempfile.mkdtemp(prefix="lsp_parity_")
    parts = []
    for tn in a.targets.split(","):
        t = make_synth(os.path.join(tmp, "synth")) if tn == "synth" else crustc_target()
        res, n, nd, pts = run(servers, t, a.points, a.timeout)
        if a.detail:
            detail(tn, res, pts)
        parts.append(report(tn, res, n, nd))
    text = "\n\n".join(parts) + "\n"
    print(text)
    if a.out:
        open(a.out, "w").write(text)
    shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
