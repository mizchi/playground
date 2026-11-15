"""exrustc と rustc (incremental) のビルド時間・ピークメモリ・ディスク消費・バイナリサイズを比べる。
usage: python3 bench/run.py [N ...]   (このディレクトリで実行。事前に cargo build --release)"""
import os, resource, shutil, subprocess, sys, time

EXR = os.path.abspath("target/release/exrustc")
W = os.path.abspath("out/bench")


def measure(cmd):
    # 子プロセスを 1 段挟み、そのプロセスの RUSAGE_CHILDREN で単一コマンドの maxrss を取る
    code = ("import resource,subprocess,sys,time;t=time.perf_counter();"
            "subprocess.run(sys.argv[1:],check=True,stdout=subprocess.DEVNULL);"
            "print((time.perf_counter()-t)*1e3, resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)")
    out = subprocess.run([sys.executable, "-c", code, *cmd], check=True, capture_output=True, text=True).stdout
    ms, kib = out.split()
    return float(ms), int(kib)


def du(path):
    if os.path.isfile(path):
        return os.path.getsize(path)
    return sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(path) for f in fs)


def gen(n, variant, path):
    with open(path, "w") as f:
        subprocess.run([sys.executable, "bench/gen.py", str(n), str(variant)], stdout=f, check=True)


def bench(n):
    d = os.path.join(W, str(n))
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    src = os.path.join(d, "p.rs")
    rows = []

    def scenario(tool, build, out, store):
        gen(n, 0, src)
        cold = measure(build)
        noop = measure(build)
        gen(n, 1, src)
        edit = measure(build)
        r1 = subprocess.run([out], capture_output=True, text=True).stdout.strip()
        rows.append((tool, cold, noop, edit, os.path.getsize(out), du(store), r1))

    exe = os.path.join(d, "exr")
    scenario("exrustc (auto)", [EXR, src, "-o", exe], exe, exe + ".exrc")
    for lk in ["bfd", "lld", "wild"]:
        if lk == "wild" and not shutil.which("wild"):
            continue
        exe2 = os.path.join(d, "exr-" + lk)
        scenario(f"exrustc (--linker={lk})", [EXR, src, "-o", exe2, f"--linker={lk}"], exe2, exe2 + ".exrc")
    for name, flags in [("rustc -Cincremental dbg=0", ["-Cdebuginfo=0"]),
                        ("rustc -Cincremental dbg=2", ["-Cdebuginfo=2"])][: 1 if n > 1000 else 2]:
        inc = os.path.join(d, "inc-" + flags[0][-1])
        rexe = os.path.join(d, "r-" + flags[0][-1])
        scenario(name, ["rustc", "-Awarnings", "-Cincremental=" + inc, *flags, src, "-o", rexe], rexe, inc)
    if n > 1000:  # rustc -O は関数数に対して超線形に遅くなるので省略
        return print_rows(n, rows)
    rexe = os.path.join(d, "r-O")
    gen(n, 1, src)
    t, kib = measure(["rustc", "-Awarnings", "-O", "-Cstrip=symbols", src, "-o", rexe])
    rows.append(("rustc -O (参考, 非incremental)", (t, kib), None, None, os.path.getsize(rexe), 0,
                 subprocess.run([rexe], capture_output=True, text=True).stdout.strip()))
    print_rows(n, rows)


def print_rows(n, rows):
    print(f"\n### N = {n} 関数\n")
    print("| tool | cold ms | no-op ms | 1関数編集 ms | peak RSS (cold) | binary | cache/incr dir | out |")
    print("|---|---:|---:|---:|---:|---:|---:|---|")
    for tool, cold, noop, edit, size, store, res in rows:
        f = lambda x: "-" if x is None else f"{x[0]:.1f}"
        sys.stdout.flush()
        print(f"| {tool} | {f(cold)} | {f(noop)} | {f(edit)} | {cold[1]/1024:.1f} MiB | {size/1024:.1f} KiB | {store/1024:.1f} KiB | {res} |")


for n in [int(a) for a in sys.argv[1:]] or [1000, 5000]:
    bench(n)
