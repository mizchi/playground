#!/usr/bin/env python3
"""ide のゴールデン (crates/ide/tests/golden/navigation.txt) を rust-analyzer と比べる。
fixture のワークスペース (crates/ide/tests/fixture/ws) をコピーして rust-analyzer に開かせ、ゴールデンの各識別子の位置で
definition / references (定義を含む) / hover を聞く。

比べるもの:
- definition: rust-analyzer の行き先がワークスペースの中のとき、(ファイル, 行, 桁) が同じか
- references: 数が同じか (std の定義のものも、ワークスペースの中の参照の数で比べられる)
- hover: rust-analyzer が返した所で返したか
usage: python3 bench/ide_golden_ra.py [--detail]
"""

import json, os, re, shutil, sys, tempfile, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lsp_compare import HERE, Lsp, uri  # noqa: E402

FX = os.path.join(HERE, "crates/ide/tests/fixture/ws")
GOLDEN = os.path.join(HERE, "crates/ide/tests/golden/navigation.txt")
KEYWORDS = set("as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while".split())


def golden():
    """ws の各識別子: (ファイル, 行, 桁, 名前, 定義の (ファイル, 行, 桁) か None, 参照の数, hover があるか)"""
    out = []
    for l in open(GOLDEN):
        m = re.match(r"^ws/(\S+):(\d+):(\d+) (\S+) -> (\S+)(?: \S+)* \| refs (\d+)/\d+.*\| (.*)$", l.rstrip("\n"))
        if not m:
            continue
        f, li, col, name, d, n, h = m.groups()
        dm = re.match(r"^ws/(\S+):(\d+):(\d+)\+", d)
        lib = d.startswith("lib/")
        out.append((f, int(li), int(col), name, (dm.group(1), int(dm.group(2)), int(dm.group(3))) if dm else ("<lib>" if lib else None), int(n), h != "-"))
    return out


def main():
    detail = "--detail" in sys.argv
    ra = os.environ.get("RUST_ANALYZER") or "rust-analyzer"
    tmp = tempfile.mkdtemp(prefix="ide_golden_")
    root = os.path.join(tmp, "ws")
    shutil.copytree(FX, root)
    s = Lsp([ra], root)
    caps = {"textDocument": {"hover": {"contentFormat": ["markdown"]}}, "workspace": {"symbol": {}}}
    s.request("initialize", {"processId": os.getpid(), "rootUri": uri(root), "capabilities": caps, "workspaceFolders": [{"uri": uri(root), "name": "ws"}]}, timeout=300)
    s.notify("initialized", {})
    files = sorted({g[0] for g in golden()})
    for f in files:
        p = os.path.join(root, f)
        s.notify("textDocument/didOpen", {"textDocument": {"uri": uri(p), "languageId": "rust", "version": 1, "text": open(p).read()}})
    t0 = time.time()
    while time.time() - t0 < 300:
        m = s.request("workspace/symbol", {"query": "shout_len"}, timeout=300)
        if any(x.get("name") == "shout_len" for x in ((m or {}).get("result") or [])):
            break
        time.sleep(0.5)
    time.sleep(2)
    st = {"def": [0, 0], "refs": [0, 0], "hover": [0, 0]}
    for (f, li, col, name, d, n, h) in golden():
        if name in KEYWORDS:
            continue
        p = {"textDocument": {"uri": uri(os.path.join(root, f))}, "position": {"line": li - 1, "character": col - 1}}
        r = (s.request("textDocument/definition", p, timeout=60) or {}).get("result")
        if isinstance(r, dict):
            r = [r]
        rd = None
        for x in r or []:
            u = x.get("uri") or x.get("targetUri")
            rg = x.get("targetSelectionRange") or x.get("range")
            if u.startswith(uri(root)):
                rd = (u[len(uri(root)) + 1:], rg["start"]["line"] + 1, rg["start"]["character"] + 1)
            else:
                rd = "<lib>"
            break
        if isinstance(rd, tuple):
            st["def"][1] += 1
            if rd == d:
                st["def"][0] += 1
            elif detail:
                print(f"def  {f}:{li}:{col} {name}: crustc {d} / ra {rd}")
        rf = (s.request("textDocument/references", dict(p, context={"includeDeclaration": True}), timeout=60) or {}).get("result")
        rn = len(rf) if isinstance(rf, list) else 0
        if rn:
            st["refs"][1] += 1
            if rn == n:
                st["refs"][0] += 1
            elif detail:
                print(f"refs {f}:{li}:{col} {name}: crustc {n} / ra {rn}")
        hv = (s.request("textDocument/hover", p, timeout=60) or {}).get("result")
        if hv:
            st["hover"][1] += 1
            if h:
                st["hover"][0] += 1
            elif detail:
                print(f"hover {f}:{li}:{col} {name}: crustc なし")
    s.close()
    shutil.rmtree(tmp, ignore_errors=True)
    for k, (a, b) in st.items():
        print(f"{k}: {a}/{b} ({100 * a / max(b, 1):.0f}%)")


if __name__ == "__main__":
    main()
