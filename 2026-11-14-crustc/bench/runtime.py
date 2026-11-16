"""crustc と rustc で、実行時間・ビルド時間・バイナリサイズを比べる (bench/rt/*.rs)。
usage: python3 bench/runtime.py [NAME ...]   (このディレクトリで実行。事前に cargo build --release)
各設定でビルド 3 回・実行 3 回の中央値。出力が rustc -O と一致するかも確かめる。"""
import os, shutil, statistics, subprocess, sys, time

CRUSTC = os.path.abspath("target/release/crustc")
W = os.path.abspath("out/runtime")
# (driver2: syn・Cranelift を使わない経路。`env` で CRUSTC_DRIVER2 などを渡す。MINCLIF_NO_REGALLOC はレジスタ割り当てなし)
CONFIGS = [
    ("crustc", lambda s, o: [CRUSTC, s, "-o", o, "--no-cache"]),
    ("crustc -O", lambda s, o: [CRUSTC, s, "-o", o, "-O", "--no-cache"]),
    ("driver2 -O (割り当てなし)", lambda s, o: ["env", "CRUSTC_DRIVER2=1", "MINCLIF_NO_REGALLOC=1", CRUSTC, s, "-o", o, "-O", "--no-cache"]),
    ("driver2 -O", lambda s, o: ["env", "CRUSTC_DRIVER2=1", CRUSTC, s, "-o", o, "-O", "--no-cache"]),
    ("rustc (debug)", lambda s, o: ["rustc", "--edition", "2021", "-Awarnings", s, "-o", o]),
    ("rustc -O", lambda s, o: ["rustc", "--edition", "2021", "-Awarnings", "-O", s, "-o", o]),
]


def timed(cmd, out=subprocess.DEVNULL):
    t = time.perf_counter()
    r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=True)
    return (time.perf_counter() - t) * 1e3, r.stdout


def bench(name):
    src = os.path.abspath(f"bench/rt/{name}.rs")
    rows, want = [], None
    for label, mk in reversed(CONFIGS):  # rustc -O の出力を正解にする
        exe = os.path.join(W, f"{name}-{abs(hash(label)) % 10**8}")
        builds = [timed(mk(src, exe))[0] for _ in range(3)]
        runs, out = [], None
        for _ in range(3):
            ms, out = timed([exe])
            runs.append(ms)
        if want is None:
            want = out
        # (rustc のバイナリは std のデバッグ情報とシンボルを含むので、strip した大きさも出す)
        subprocess.run(["strip", "-o", exe + ".s", exe], check=True)
        rows.append((label, statistics.median(builds), os.path.getsize(exe), os.path.getsize(exe + ".s"),
                     statistics.median(runs), out == want))
    rows.reverse()
    base = next(r for r in rows if r[0] == "rustc -O")[4]
    print(f"\n### {name}\n")
    print("| 設定 | ビルド ms | バイナリ KiB | strip 後 KiB | 実行 ms | rustc -O 比 | 出力 |")
    print("|---|---:|---:|---:|---:|---:|---|")
    for label, b, size, ssize, run, ok in rows:
        print(f"| {label} | {b:.0f} | {size / 1024:.1f} | {ssize / 1024:.1f} | {run:.0f} | {run / base:.2f}x | {'一致' if ok else '不一致'} |")
    sys.stdout.flush()


shutil.rmtree(W, ignore_errors=True)
os.makedirs(W)
names = sys.argv[1:] or sorted(f[:-3] for f in os.listdir("bench/rt") if f.endswith(".rs"))
for n in names:
    bench(n)
