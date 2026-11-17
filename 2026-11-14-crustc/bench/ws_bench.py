"""複数クレートのワークスペースのビルドの時間 (crustc build と rustc の比較)。

usage: python3 bench/ws_bench.py [--crates K] [--files F] [--fns N] [--reps R] [--rustc]

場面:
  cold      何もない状態から全部
  noop      何も変えずにもう一度
  leaf-body いちばん下のクレート (c0) の関数の本体を 1 つ変える (全部のクレートが c0 を使う)
  leaf-sig  c0 の関数のシグネチャ (と本体) を変える
  app-body  bin の main を変える
rustc は rlib を作ってリンクする (cargo と同じく、依存が変われば使う側もコンパイルし直す)。-C incremental を付ける
"""
import os
import shutil
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRUSTC = os.path.join(ROOT, "target/release/crustc")


def run(cmd, cwd, env=None):
    t = time.perf_counter()
    r = subprocess.run(cmd, cwd=cwd, shell=True, capture_output=True, text=True, env=env)
    dt = time.perf_counter() - t
    if r.returncode != 0:
        print(r.stdout, r.stderr)
        raise SystemExit(f"failed: {cmd}")
    return dt, r.stderr


def edit(path, old, new):
    s = open(path).read()
    assert old in s, (path, old)
    open(path, "w").write(s.replace(old, new, 1))


def main():
    a = sys.argv[1:]
    opt = {"--crates": 6, "--files": 8, "--fns": 40, "--reps": 3}
    use_rustc = "--rustc" in a
    i = 0
    while i < len(a):
        if a[i] in opt:
            opt[a[i]] = int(a[i + 1])
            i += 2
        else:
            i += 1
    d = os.path.join(ROOT, "out/wsbench")
    shutil.rmtree(d, ignore_errors=True)
    subprocess.run([sys.executable, os.path.join(ROOT, "bench/ws_gen.py"), d, "--crates", str(opt["--crates"]),
                    "--files", str(opt["--files"]), "--fns", str(opt["--fns"])], check=True)
    nfn = opt["--crates"] * opt["--files"] * opt["--fns"]
    print(f"workspace: {opt['--crates']} lib crates x {opt['--files']} files x {opt['--fns']} fns = {nfn} fns (+ app)")
    tools = [("crustc", f"{CRUSTC} build workspace.txt -o target", lambda: shutil.rmtree(os.path.join(d, "target"), ignore_errors=True))]
    if use_rustc:
        tools.append(("rustc", "FLAGS='-C incremental=inc' OUT=target_rustc bash build_rustc.sh",
                      lambda: (shutil.rmtree(os.path.join(d, "target_rustc"), ignore_errors=True), shutil.rmtree(os.path.join(d, "inc"), ignore_errors=True))))
    m0 = os.path.join(d, "c0/src/m0.rs")
    main_rs = os.path.join(d, "app/src/main.rs")
    results = {}
    for name, cmd, clean in tools:
        res = {}
        for rep in range(opt["--reps"]):
            clean()
            # (毎回同じソースから)
            subprocess.run([sys.executable, os.path.join(ROOT, "bench/ws_gen.py"), d, "--crates", str(opt["--crates"]),
                            "--files", str(opt["--files"]), "--fns", str(opt["--fns"])], check=True)
            steps = [
                ("cold", None),
                ("noop", None),
                ("leaf-body", lambda r=rep: edit(m0, "let mut s = x.wrapping_add(3);", f"let mut s = x.wrapping_add({1003 + r});")),
                ("leaf-sig", lambda r=rep: (edit(m0, "pub fn f0_3(x: u64) -> u64 {", "pub fn f0_3(x: u64, _e: u8) -> u64 {"),
                                            edit(m0, "f0_3(x >> 1)", "f0_3(x >> 1, 0)"))),
                ("app-body", lambda r=rep: edit(main_rs, "let mut s = std::env::args().count() as u64;",
                                                f"let mut s = std::env::args().count() as u64 + {r + 1};")),
            ]
            outs = None
            for step, f in steps:
                if f:
                    f()
                dt, _ = run(cmd, d)
                res.setdefault(step, []).append(dt)
            exe = os.path.join(d, "target/app" if name == "crustc" else "target_rustc/app")
            outs = subprocess.run([exe], capture_output=True, text=True).stdout
            res.setdefault("_out", []).append(outs)
        results[name] = res
    if use_rustc and results["crustc"]["_out"] != results["rustc"]["_out"]:
        print("OUTPUT DIFFERS between crustc and rustc")
    print(f"| step | " + " | ".join(n for n, _, _ in tools) + " |")
    print("|---|" + "---:|" * len(tools))
    for step in ["cold", "noop", "leaf-body", "leaf-sig", "app-body"]:
        cells = []
        for n, _, _ in tools:
            v = sorted(results[n][step])
            cells.append(f"{v[len(v) // 2] * 1000:.0f} ms")
        print(f"| {step} | " + " | ".join(cells) + " |")


main()
