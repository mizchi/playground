"""crustc と本物の rustc を同じプログラム・同じ手順で比べ、表 (Markdown) にする。
usage: python3 bench/vs_rustc.py [NAME ...]   (このディレクトリで実行。事前に cargo build --release)
  NAME は gen1000 (bench/gen.py の 1000 関数) か bench/rt/NAME.rs。省略するとすべて。

各プログラムで次を測る (時間は 3 回の中央値):
  cold    キャッシュも incremental ディレクトリもない状態からのビルド
  no-op   何も変えずにもう一度ビルド
  edit    1 か所変えてビルド (gen1000 は f500 の本体、bench/rt は末尾に足した関数の定数)
  disk    ビルドが残すもの (crustc: キャッシュ + .o、rustc: incremental ディレクトリ)
  binary  出力の大きさ (strip 後)、run 実行時間 (出力は rustc -O と一致するか確かめる)
比べる設定: crustc (debug, 既定のリンカ。wild が PATH にあれば wild)、crustc -O、
rustc -Cincremental (debug)、rustc -O (非 incremental)"""
import os, shutil, statistics, subprocess, sys, time

CRUSTC = os.path.abspath("target/release/crustc")
W = os.path.abspath("out/vs_rustc")


def run(cmd):
    t = time.perf_counter()
    r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if r.returncode != 0:
        raise SystemExit(f"failed: {' '.join(cmd)}\n{r.stderr.decode()[:2000]}")
    return (time.perf_counter() - t) * 1e3, r.stdout


def du(*paths):
    n = 0
    for p in paths:
        if os.path.isfile(p):
            n += os.path.getsize(p)
        elif os.path.isdir(p):
            n += sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(p) for f in fs)
    return n


def source(name, variant):
    if name.startswith("gen"):
        n = int(name[3:])
        return subprocess.run([sys.executable, "bench/gen.py", str(n), str(variant)],
                              capture_output=True, text=True, check=True).stdout
    src = open(f"bench/rt/{name}.rs").read()
    # (編集: 末尾の、どこからも呼ばれない関数の定数を変える)
    return src + f"\n#[allow(dead_code)]\nfn __bench_edit() -> u64 {{ {variant} }}\n"


def median3(f):
    return statistics.median(f() for _ in range(3))


def bench(name):
    d = os.path.join(W, name)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    src = os.path.join(d, "p.rs")
    rows = []

    def scenario(label, build, exe, store, incremental=True):
        def write(v):
            with open(src, "w") as f:
                f.write(source(name, v))

        def cold():
            for p in store:
                shutil.rmtree(p, ignore_errors=True) if os.path.isdir(p) else (os.path.exists(p) and os.remove(p))
            write(0)
            return run(build)[0]

        c = median3(cold)
        noop = median3(lambda: run(build)[0]) if incremental else None
        k = [0]

        def edit():
            k[0] += 1
            write(k[0])
            return run(build)[0]

        e = median3(edit) if incremental else None
        write(0)
        run(build)
        r = statistics.median(run([exe])[0] for _ in range(3))
        out = run([exe])[1]
        # (crustc の出力はシンボルを持たない。rustc の出力は std のシンボルとデバッグ情報を含むので strip した大きさで比べる)
        subprocess.run(["strip", "-o", exe + ".strip", exe], check=True)
        rows.append((label, c, noop, e, du(*store), os.path.getsize(exe + ".strip"), r, out))

    exe = os.path.join(d, "c")
    scenario("crustc", [CRUSTC, src, "-o", exe], exe, [exe + ".crcache2", exe + ".o"])
    exe = os.path.join(d, "cO")
    scenario("crustc -O", [CRUSTC, src, "-o", exe, "-O"], exe, [exe + ".crcache2", exe + ".o"])
    exe, inc = os.path.join(d, "r"), os.path.join(d, "r-incr")
    scenario("rustc -Cincremental", ["rustc", "--edition", "2021", "-Awarnings", "-Cincremental=" + inc, src, "-o", exe],
             exe, [inc])
    exe = os.path.join(d, "rO")
    scenario("rustc -O", ["rustc", "--edition", "2021", "-Awarnings", "-O", src, "-o", exe], exe, [], incremental=False)
    want = rows[-1][7]
    base = rows[-1][6]
    f = lambda x: "-" if x is None else f"{x:.0f}"
    print(f"\n### {name}\n")
    print("| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|---|")
    for label, c, noop, e, disk, size, r, out in rows:
        print(f"| {label} | {f(c)} | {f(noop)} | {f(e)} | {disk / 1024:.1f} | {size / 1024:.1f} | {r:.0f} | {r / base:.2f}x | {'一致' if out == want else '不一致'} |")
    sys.stdout.flush()


linker = next((l for l in ["wild", "mold", "ld.lld", "ld"] if shutil.which(l)), "?")
print(f"crustc のリンカ: {linker} / {subprocess.run(['rustc', '--version'], capture_output=True, text=True).stdout.strip()}")
names = sys.argv[1:] or ["gen1000"] + sorted(f[:-3] for f in os.listdir("bench/rt") if f.endswith(".rs"))
for n in names:
    bench(n)
