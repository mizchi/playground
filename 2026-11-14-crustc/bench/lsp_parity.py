#!/usr/bin/env python3
"""LSP の機能の比較: crustc-lsp / rust-analyzer / rust-glancer が、同じ問い合わせに同じ答えを返すか。
速さの比較 (lsp_compare.py) は「何を返すか」を見ないので、機能の不足と答えの違いをここで見る。rust-analyzer を正解とする。

(事前に cargo build --release --workspace: target/release/crustc-lsp を使う)
usage: python3 bench/lsp_parity.py [--servers crustc,rust-analyzer,rust-glancer] [--targets synth,fixture,crustc,ide]
                                   [--points 40] [--out bench/lsp_parity_result.md] [--json bench/lsp_parity.json]
                                   [--compare bench/lsp_parity.json] [--detail [--detail-out FILE]]

見るもの (対象ごと・サーバごと):
- caps: initialize が返した機能 (documentSymbol / workspaceSymbol / definition / hover / completion / references / ...)
- docsym: 開いたファイルの documentSymbol の (名前, 種類) の集合が rust-analyzer と何割同じか (足りない・余分の例)
- wsym: 問い合わせごとの上位 20 個の名前の集合の一致 (Jaccard)
- definition: ファイルの中の識別子 (ワークスペースで定義された名前) の位置で、行き先 (ファイル, 行) が rust-analyzer と同じ割合
- hover: 同じ位置で、空でない hover を返した割合
- completion: `.` の直後 (`x.|name`) と、識別子の 2 文字目の後 (`na|me`) で、そこに実際に書いてある名前が候補に入る割合と候補の数
- references: 同じ位置で、参照の数が rust-analyzer と同じ割合
- documentHighlight / rename / signatureHelp: 数・書き換えの集合・(名前, 引数の数, 今の引数) が同じ割合
- 一致の平均: 上の割合の平均
各問い合わせの時間の p50 / p90、起動から索引ができるまでの時間、メモリの最大 (サーバと子のプロセスの RSS) も出す。
--json で crustc の数字を残し、次に --compare で前回との差 (一致は点、時間は ms) を列に足す。
"""

import argparse, json, os, re, shutil, statistics, sys, tempfile, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lsp_compare import HERE, Lsp, make_synth, crustc_target, uri  # noqa: E402

CAPS = [("documentSymbolProvider", "docsym"), ("workspaceSymbolProvider", "wsym"), ("definitionProvider", "definition"),
        ("hoverProvider", "hover"), ("completionProvider", "completion"), ("referencesProvider", "references"),
        ("renameProvider", "rename"), ("signatureHelpProvider", "signature"), ("semanticTokensProvider", "semantic"),
        ("documentHighlightProvider", "highlight"), ("inlayHintProvider", "inlay"), ("codeActionProvider", "codeAction"), ("documentFormattingProvider", "format")]


def start(cmd, t, timeout):
    root = t["root"]
    t0 = time.time()
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
        "experimental": {"serverStatusNotification": True},
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
    while time.time() - t0 < timeout and s.alive:
        m = s.request("workspace/symbol", {"query": q}, timeout=timeout)
        if any(x.get("name") == want for x in ((m or {}).get("result") or [])):
            break
        time.sleep(0.05)
    s.ready_ms = (time.time() - t0) * 1000
    # (名前が引けても、rust-analyzer は型の解析などをまだ続けている: 終わるまで待つ。待った時間は別に残す)
    s.wait_idle(timeout)
    s.idle_ms = (time.time() - t0) * 1000
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


def edits_of(res):
    """rename の WorkspaceEdit → (ファイル名, 行, 桁, 新しい文字列) の集合"""
    if not res:
        return None
    out = set()
    for u, es in (res.get("changes") or {}).items():
        for e in es:
            out.add((u.rsplit("/", 1)[-1], e["range"]["start"]["line"], e["range"]["start"]["character"], e["newText"]))
    for dc in res.get("documentChanges") or []:
        u = dc.get("textDocument", {}).get("uri", "")
        for e in dc.get("edits", []):
            out.add((u.rsplit("/", 1)[-1], e["range"]["start"]["line"], e["range"]["start"]["character"], e["newText"]))
    return out


def call_points(text, n):
    """呼び出し `name(a, b)` の、最初の引数の始めの位置と、引数の番号 (最初のカンマの後なら 1)"""
    pts = []
    for li, l in enumerate(text.split("\n")):
        if l.lstrip().startswith("//"):
            continue
        for m in re.finditer(r"(?<!fn )\b([A-Za-z_][A-Za-z0-9_]*)\((?=[^)])", l):
            if m.group(1) in ("if", "while", "match", "for", "return", "Some", "Ok", "Err", "matches", "assert", "println", "format", "write", "writeln", "vec"):
                continue
            pts.append((li, m.end(), m.group(1)))
    if len(pts) > n:
        step = len(pts) / n
        pts = [pts[int(i * step)] for i in range(n)]
    return pts


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
    cpts = call_points(text, npts // 2)
    res = {}
    for sn, cmd in servers.items():
        print(f"== {sn}", file=sys.stderr)
        s, caps = start(cmd, t, timeout)
        if s is None:
            res[sn] = None
            continue
        times = {}
        r = {"caps": {k: bool(caps.get(c)) for c, k in CAPS}, "times": times, "ready_ms": s.ready_ms, "idle_ms": s.idle_ms}
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
        r["hl"], r["ren"] = [], []
        for (li, col, name) in pts:
            p = {"textDocument": {"uri": u}, "position": {"line": li, "character": col + 1}}
            h = req(s, "textDocument/documentHighlight", p, times)
            r["hl"].append(len(h) if isinstance(h, list) else None)
            r["ren"].append(edits_of(req(s, "textDocument/rename", dict(p, newName="zz_renamed"), times)))
        r["sig"] = []
        for (li, col, name) in cpts:
            h = req(s, "textDocument/signatureHelp", {"textDocument": {"uri": u}, "position": {"line": li, "character": col}}, times)
            sg = (h or {}).get("signatures") or []
            if sg:
                act = sg[0].get("activeParameter", h.get("activeParameter"))
                r["sig"].append((name in sg[0].get("label", ""), len(sg[0].get("parameters") or []), act))
            else:
                r["sig"].append(None)
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
        r["rss_mb"] = s.tree_peak / 1024
        s.close()
        res[sn] = r
    return res, len(pts), len(dpts), (pts, cpts)


def pct(a, b):
    return f"{100 * a / b:.0f}%" if b else "-"


def detail(tn, res, pts):
    """--detail: rust-analyzer と違った所 (definition の行き先・references の数・rename・signatureHelp)"""
    pts, cpts = pts
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
            if r["hl"][i] != ref["hl"][i] and ref["hl"][i] is not None:
                print(f"[{tn}/{sn}] {li + 1}:{col + 1} {name}: highlight {r['hl'][i]} vs ra {ref['hl'][i]}", file=sys.stderr)
            if r["ren"][i] != ref["ren"][i]:
                a, b = r["ren"][i] or set(), ref["ren"][i] or set()
                print(f"[{tn}/{sn}] {li + 1}:{col + 1} {name}: rename {'断った' if r['ren'][i] is None else len(a)} vs ra {'断った' if ref['ren'][i] is None else len(b)}; 足りない {sorted(b - a)[:3]} 余分 {sorted(a - b)[:3]}", file=sys.stderr)
            if ref["hover"][i] and not r["hover"][i]:
                print(f"[{tn}/{sn}] {li + 1}:{col + 1} {name}: hover なし", file=sys.stderr)
        for q, v in ref["wsym"].items():
            w = r["wsym"].get(q, set())
            if v != w:
                print(f"[{tn}/{sn}] workspace/symbol {q!r}: 足りない {sorted(v - w)[:5]} 余分 {sorted(w - v)[:5]}", file=sys.stderr)
        a, b = r["docsym"], ref["docsym"]
        if a != b:
            print(f"[{tn}/{sn}] documentSymbol: 足りない {sorted(b - a, key=str)[:6]} 余分 {sorted(a - b, key=str)[:6]}", file=sys.stderr)
        for i, (li, col, name) in enumerate(cpts):
            if ref["sig"][i] and r["sig"][i] != ref["sig"][i]:
                print(f"[{tn}/{sn}] {li + 1}:{col + 1} {name}(: signature {r['sig'][i]} vs ra {ref['sig'][i]}", file=sys.stderr)
        # (completion: rust-analyzer は候補に入れたが、こちらは入れなかった所)
        for key in ["dot", "pre"]:
            for i, (li, col, name) in enumerate(r.get(key + "_at", [])):
                if i < len(ref[key]) and ref[key][i] and not r[key][i]:
                    print(f"[{tn}/{sn}] {li + 1}:{col + 1} {name}: completion ({key}) に入らない (候補 {r[key + '_n'][i]})", file=sys.stderr)


def pctile(v, q):
    v = sorted(v)
    return v[min(len(v) - 1, int(q * len(v)))] if v else 0.0


TIMED = ["definition", "hover", "completion", "references", "rename", "signatureHelp", "documentHighlight"]


def metrics(res):
    """サーバごとの一致の数字 {名前: (割合 0..1 か None, 表の文字列)} (rust-analyzer を正解とする)"""
    ref = res.get("rust-analyzer")
    out = {}
    for sn, r in res.items():
        if not r:
            continue
        m = {}
        if ref:
            a, b = r["docsym"], ref["docsym"]
            an, bn = set(norm_name(n) for n, _ in a), set(norm_name(n) for n, _ in b)
            miss, extra = sorted(bn - an)[:4], sorted(an - bn)[:4]
            v = len(an & bn) / len(bn) if bn else None
            m["documentSymbol (名前の一致)"] = (v, f"名前 {pct(len(an & bn), len(bn))} (種類も {pct(len(a & b), len(b))})" + (f"; 足りない {', '.join(miss)}" if miss else "") + (f"; 余分 {', '.join(extra)}" if extra else ""))
            js = []
            for q, v2 in ref["wsym"].items():
                w = r["wsym"].get(q, set())
                js.append(len(v2 & w) / len(v2 | w) if v2 | w else 1.0)
            v = statistics.mean(js) if js else None
            m["workspace/symbol (上位 20 の一致)"] = (v, f"{100 * v:.0f}%" if v is not None else "-")
            ok = sum(1 for x, y in zip(r["def"], ref["def"]) if y and x == y)
            n = sum(1 for y in ref["def"] if y)
            m["definition (行き先の一致)"] = (ok / n if n else None, f"同じ {pct(ok, n)} (答えた {pct(sum(1 for x in r['def'] if x), len(r['def']))})")
            m["hover (名前を含む)"] = (sum(r["hover"]) / len(r["hover"]) if r["hover"] else None, pct(sum(r["hover"]), len(r["hover"])))
            for key, label in [("refs", "references (数の一致)"), ("hl", "documentHighlight (数の一致)")]:
                ok = sum(1 for x, y in zip(r[key], ref[key]) if y is not None and x == y)
                n = sum(1 for y in ref[key] if y is not None)
                m[label] = (ok / n if n else None, pct(ok, n))
            ok = sum(1 for x, y in zip(r["ren"], ref["ren"]) if y and x == y)
            n = sum(1 for y in ref["ren"] if y)
            same_no = sum(1 for x, y in zip(r["ren"], ref["ren"]) if not y and not x)
            m["rename (書き換えの一致)"] = (ok / n if n else None, f"同じ {pct(ok, n)} (断ったのも同じ {same_no}/{sum(1 for y in ref['ren'] if not y)})")
            ok = sum(1 for x, y in zip(r["sig"], ref["sig"]) if y and x == y)
            n = sum(1 for y in ref["sig"] if y)
            m["signatureHelp (名前・引数の数・今の引数の一致)"] = (ok / n if n else None, f"同じ {pct(ok, n)} (答えた {pct(sum(1 for x in r['sig'] if x), len(r['sig']))})")
            for key, label in [("dot", "completion `x.\\|` (書いてある名前を含む)"), ("pre", "completion `na\\|me` (同)")]:
                v = sum(r[key]) / len(r[key]) if r[key] else None
                m[label] = (v, f"{pct(sum(r[key]), len(r[key]))} (候補 {statistics.median(r[key + '_n']) if r[key + '_n'] else 0:.0f})")
            vs = [x[0] for x in m.values() if x[0] is not None]
            m["一致の平均"] = (statistics.mean(vs) if vs else None, f"{100 * statistics.mean(vs):.1f}%" if vs else "-")
        out[sn] = m
    return out


def timing(r):
    """{メソッド: (p50, p90)} (ms)"""
    out = {}
    for k in TIMED:
        v = r["times"].get("textDocument/" + k)
        if v:
            out[k] = (statistics.median(v), pctile(v, 0.9))
    return out


def report(tn, res, npts, ndots, prev=None):
    out = [f"#### {tn} ({npts} 箇所、`.` の後 {ndots} 箇所。比べる相手は rust-analyzer)", ""]
    sv = [k for k in res if res[k]]
    ms = metrics(res)
    pm = (prev or {}).get(tn, {})
    head = sv + (["crustc の前回との差"] if pm and "crustc" in sv else [])
    out.append("| 機能 | " + " | ".join(head) + " |")
    out.append("|---|" + "---|" * len(head))
    out.append("| 対応を返す機能 | " + " | ".join(", ".join(k for k, v in res[s]["caps"].items() if v) or "-" for s in sv) + (" | |" if len(head) > len(sv) else " |"))
    labels = list(ms[sv[0]].keys()) if sv and ms.get(sv[0]) else []
    for lb in labels:
        cells = [ms[s].get(lb, (None, "-"))[1] for s in sv]
        if len(head) > len(sv):
            now, old = ms["crustc"].get(lb, (None,))[0], pm.get("metrics", {}).get(lb)
            cells.append(f"{100 * (now - old):+.1f} 点" if now is not None and old is not None else "")
        out.append(f"| {lb} | " + " | ".join(cells) + " |")
    def tm(s):
        return ", ".join(f"{k[:4]} {a:.1f}/{b:.1f}" for k, (a, b) in timing(res[s]).items())
    cells = [tm(s) for s in sv]
    if len(head) > len(sv):
        pt = pm.get("timing", {})
        cells.append(", ".join(f"{k[:4]} {a - pt[k][0]:+.1f}" for k, (a, b) in timing(res["crustc"]).items() if k in pt))
    out.append("| 時間 p50/p90 (ms) | " + " | ".join(cells) + " |")
    cells = [f"{res[s]['ready_ms']:.0f} ms ({res[s]['idle_ms']:.0f} ms) / {res[s]['rss_mb']:.0f} MB" for s in sv]
    if len(head) > len(sv):
        cells.append(f"{res['crustc']['ready_ms'] - pm.get('ready_ms', 0):+.0f} ms / {res['crustc']['rss_mb'] - pm.get('rss_mb', 0):+.0f} MB" if "ready_ms" in pm else "")
    out.append("| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | " + " | ".join(cells) + " |")
    return "\n".join(out)


def summary(res):
    """JSON に残すもの (crustc の分): 一致の数字・時間・起動・メモリ"""
    r = res.get("crustc")
    if not r:
        return {}
    return {"metrics": {k: v[0] for k, v in metrics(res).get("crustc", {}).items()}, "timing": timing(r), "ready_ms": r["ready_ms"], "rss_mb": r["rss_mb"]}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--servers", default="crustc,rust-analyzer,rust-glancer")
    ap.add_argument("--targets", default="synth,fixture,crustc,ide")
    ap.add_argument("--points", type=int, default=40)
    ap.add_argument("--timeout", type=float, default=300)
    ap.add_argument("--out", default=None)
    ap.add_argument("--detail", action="store_true", help="rust-analyzer と違った所を stderr に出す")
    ap.add_argument("--detail-out", default=None, help="--detail の出力を (stderr の代わりに) このファイルに書く")
    ap.add_argument("--json", default=None, help="crustc の一致・時間・起動・メモリを JSON に書く (次の --compare に使う)")
    ap.add_argument("--compare", default=None, help="前の --json と比べて差の列を足す")
    a = ap.parse_args()
    glancer = os.environ.get("RUST_GLANCER") or shutil.which("rust-glancer") or "/home/user/rust-glancer/target/release/rust-glancer"
    allsv = {"crustc": [os.path.join(HERE, "target/release/crustc-lsp")], "rust-analyzer": [os.environ.get("RUST_ANALYZER") or "rust-analyzer"], "rust-glancer": [glancer, "lsp"]}
    servers = {k: allsv[k] for k in a.servers.split(",") if os.path.exists(allsv[k][0]) or shutil.which(allsv[k][0])}
    tmp = tempfile.mkdtemp(prefix="lsp_parity_")
    prev = json.load(open(a.compare)) if a.compare and os.path.exists(a.compare) else None
    if a.detail and a.detail_out:
        sys.stderr = open(a.detail_out, "w")
    parts, js = [], {}
    for tn in a.targets.split(","):
        t = target(tn, tmp)
        res, n, nd, pts = run(servers, t, a.points, a.timeout)
        if a.detail:
            detail(tn, res, pts)
        parts.append(report(tn, res, n, nd, prev))
        js[tn] = summary(res)
    text = "\n\n".join(parts) + "\n"
    print(text)
    if a.out:
        open(a.out, "w").write(text)
    if a.json:
        json.dump(js, open(a.json, "w"), ensure_ascii=False, indent=1)
    shutil.rmtree(tmp, ignore_errors=True)


def target(tn, tmp):
    """対象: synth (生成した 40 モジュール)・fixture (ide のテストの fixture のワークスペース)・crustc (このリポジトリの
    driver2.rs を開く)・ide (このリポジトリの navigate.rs を開く: 字句の部品が多い別の書き方のファイル)"""
    if tn == "synth":
        return make_synth(os.path.join(tmp, "synth"))
    if tn == "fixture":
        d = os.path.join(tmp, "fixture")
        shutil.copytree(os.path.join(HERE, "crates/ide/tests/fixture/ws"), d)
        return {"root": d, "open": "src/lib.rs", "ready": ("shout_len", "shout_len"), "queries": ["Counter", "Wrapper", "shout", "area", "Point"]}
    if tn == "ide":
        t = dict(crustc_target())
        t.update({"open": "crates/ide/src/navigate.rs", "queries": ["FileIndex", "def_at", "references", "Loc", "complete"]})
        return t
    return crustc_target()


if __name__ == "__main__":
    main()
