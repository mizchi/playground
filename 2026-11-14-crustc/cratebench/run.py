"""本物の小さなクレート (vendor/) を、それを使う app (apps/) と一緒に crustc と rustc でビルドして比べる。

usage: python3 cratebench/run.py [NAME ...] [--reps R]   (experiment のディレクトリで。事前に cargo build --release)

各クレートで:
  - 出力が rustc -O と同じか
  - ビルド時間: cold (キャッシュなし) / noop (何も変えない) / edit (app の main を 1 か所変える)。中央値 (ms)
  - 実行時間 (中央値, ms) とバイナリサイズ
crustc はワークスペース (lib + bin) としてビルドする。rustc は lib を rlib にして --extern でリンクする
(edit は app だけをコンパイルし直す: cargo と同じ)。
"""
import os
import shutil
import statistics
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
CRUSTC = os.path.join(ROOT, "target/release/crustc")
OUT = os.path.join(ROOT, "out/cratebench")


def sh(cmd, cwd=HERE, check=True):
    t = time.perf_counter()
    r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    dt = (time.perf_counter() - t) * 1e3
    if check and r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)}\n{r.stdout}{r.stderr}")
    return dt, r


def crates():
    out = []
    for line in open(os.path.join(HERE, "crates.txt")):
        line = line.split("#")[0].split()
        if line:
            out.append((line[0], line[1], line[2].split(",") if len(line) > 2 and line[2] != "-" else []))
    return out


def setup(name, ed, feats, d):
    """作業用のコピー (edit で app を書き換えるので) とワークスペースの説明ファイル"""
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    shutil.copytree(os.path.join(HERE, "vendor", name), os.path.join(d, "lib"))
    os.makedirs(os.path.join(d, "app"))
    shutil.copy(os.path.join(HERE, "apps", name, "main.rs"), os.path.join(d, "app/main.rs"))
    lname = name.replace("-", "_")
    f = f" features={','.join(feats)}" if feats else ""
    open(os.path.join(d, "ws.txt"), "w").write(f"lib {lname} lib/src/lib.rs{f}\nbin app app/main.rs {lname}\n")
    return lname


def rustc_cmds(lname, ed, feats, d, opt):
    o = ["-O"] if opt else []
    cfg = sum([["--cfg", f'feature="{x}"'] for x in feats], [])
    t = os.path.join(d, "rt" + ("O" if opt else ""))
    os.makedirs(t, exist_ok=True)
    lib = ["rustc", "--edition", ed, "-Awarnings", *o, *cfg, "--crate-type", "rlib", "--crate-name", lname,
           os.path.join(d, "lib/src/lib.rs"), "-o", os.path.join(t, f"lib{lname}.rlib")]
    app = ["rustc", "--edition", "2021", "-Awarnings", *o, "--crate-name", "app", os.path.join(d, "app/main.rs"),
           "-L", t, "--extern", f"{lname}={os.path.join(t, f'lib{lname}.rlib')}", "-o", os.path.join(t, "app")]
    return lib, app, os.path.join(t, "app")


def toggle_edit(d, k):
    p = os.path.join(d, "app/main.rs")
    s = open(p).read()
    a, b = ('println!("done");', 'println!("done.");') if k % 2 == 0 else ('println!("done.");', 'println!("done");')
    open(p, "w").write(s.replace(a, b, 1))


def median_run(exe, reps):
    ts = []
    out = None
    for _ in range(reps):
        dt, r = sh([exe], check=False)
        ts.append(dt)
        out = (r.stdout, r.returncode)
    return statistics.median(ts), out


def bench(name, ed, feats, reps):
    d = os.path.join(OUT, name)
    lname = setup(name, ed, feats, d)
    rows = []
    # rustc -O が基準
    want = None
    for opt in (False, True):
        lib, app, exe = rustc_cmds(lname, ed, feats, d, opt)
        cold = statistics.median([sh(lib)[0] + sh(app)[0] for _ in range(reps)])
        edits = []
        for k in range(reps * 2):
            toggle_edit(d, k)
            edits.append(sh(app)[0])
        rt, out = median_run(exe, reps)
        if opt:
            want = out
        rows.append((f"rustc{' -O' if opt else ''}", cold, None, statistics.median(edits), rt, os.path.getsize(exe), None))
    for opt in (False, True):
        o = ["-O"] if opt else []
        tgt = os.path.join(d, "ct" + ("O" if opt else ""))
        colds = []
        for _ in range(reps):
            shutil.rmtree(tgt, ignore_errors=True)
            colds.append(sh([CRUSTC, "build", "ws.txt", "-o", tgt, *o], cwd=d)[0])
        noop = statistics.median([sh([CRUSTC, "build", "ws.txt", "-o", tgt, *o], cwd=d)[0] for _ in range(reps)])
        edits = []
        for k in range(reps * 2):
            toggle_edit(d, k)
            edits.append(sh([CRUSTC, "build", "ws.txt", "-o", tgt, *o], cwd=d)[0])
        exe = os.path.join(tgt, "app")
        rt, out = median_run(exe, reps)
        rows.append((f"crustc{' -O' if opt else ''}", statistics.median(colds), noop, statistics.median(edits), rt, os.path.getsize(exe), out == want))
    return rows


def main():
    a = sys.argv[1:]
    reps = 3
    if "--reps" in a:
        i = a.index("--reps")
        reps = int(a[i + 1])
        del a[i : i + 2]
    sel = [c for c in crates() if not a or c[0] in a]
    os.makedirs(OUT, exist_ok=True)
    print("| crate | compiler | cold | noop | edit | run | size | same output |")
    print("|---|---|---:|---:|---:|---:|---:|---|")
    for name, ed, feats in sel:
        try:
            rows = bench(name, ed, feats, reps)
        except RuntimeError as e:
            print(f"| {name} | FAIL | {str(e).splitlines()[-1][:120]} |||||")
            continue
        for (c, cold, noop, edit, rt, size, same) in rows:
            ns = "" if noop is None else f"{noop:.0f}"
            sm = "" if same is None else ("yes" if same else "**no**")
            print(f"| {name} | {c} | {cold:.0f} | {ns} | {edit:.0f} | {rt:.0f} | {size // 1024} KiB | {sm} |")


if __name__ == "__main__":
    main()
