"""4 言語の雛形を同じデータ・同じ設定で動かし、速度を表にする。標準ライブラリだけで動く。

  python3 compare.py                 # 学習・推論・TTL (JAX と Enzyme.jl)
  python3 compare.py --with-autodiff # Rust std::autodiff の TTL も (nightly が必要、1 回 2 分ほど)
  python3 compare.py --skip-ttl      # 学習と推論だけ
  python3 compare.py --out RESULT.md # 表を Markdown で書き出す

使うコマンドは環境変数で差し替えられる:
  NODE (既定 node)、CARGO (既定 cargo)、PYTHON (既定 ../bench/python/.venv/bin/python、無ければ python3)、JULIA (既定 julia)

各スクリプトは最後に `bench lang=... task=... key=value ...` の行を出し、ここではその行だけを読む。
入っていない道具の行は「未実行」になる。データは ../bench/data (作り方は ../bench/README.md)。
"""

import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DATA = (HERE / "../bench/data").resolve()
args = sys.argv[1:]
skip_ttl = "--skip-ttl" in args
with_autodiff = "--with-autodiff" in args
out_md = args[args.index("--out") + 1] if "--out" in args else None

default_py = HERE / "../bench/python/.venv/bin/python"
TOOLS = {
    "node": os.environ.get("NODE", "node"),
    "cargo": os.environ.get("CARGO", "cargo"),
    "python": os.environ.get("PYTHON", str(default_py) if default_py.exists() else "python3"),
    "julia": os.environ.get("JULIA", "julia"),
}


def have(tool):
    return shutil.which(TOOLS[tool]) is not None


def run(label, tool, cwd, cmd, env=None):
    """コマンドを実行し、bench 行を dict のリストで返す。失敗したら理由の文字列を返す"""
    if not have(tool):
        return f"{TOOLS[tool]} が無い"
    print(f"  {label}: {' '.join(cmd)}", flush=True)
    r = subprocess.run([TOOLS[tool], *cmd], cwd=HERE / cwd, capture_output=True, text=True, env={**os.environ, **(env or {})})
    if r.returncode != 0:
        tail = (r.stderr or r.stdout).strip().splitlines()[-1:] or ["(no output)"]
        return f"失敗: {tail[0][:80]}"
    rows = []
    for line in r.stdout.splitlines():
        if line.startswith("bench "):
            rows.append(dict(kv.split("=", 1) for kv in line.split()[1:]))
    return rows


if not (DATA / "cache_X.f32").exists():
    sys.exit(f"{DATA}/cache_X.f32 がない。../bench/README.md の手順でデータを作る")

results = {}
print("train / infer")
plan = [
    ("TS", "node", "ts", ["train_cache.ts"], ["infer.ts", "out/mlp16.json"]),
    ("Rust", "cargo", "rust", ["run", "--release", "-q", "--example", "train_cache"],
     ["run", "--release", "-q", "--example", "infer", "--", "out/mlp16.json"]),
    ("Python", "python", "python", ["train_cache.py"], ["infer.py", "out/mlp16.json"]),
    ("Julia", "julia", "julia", ["train_cache.jl"], ["infer.jl", "out/mlp16.json"]),
]
for lang, tool, cwd, train_cmd, infer_cmd in plan:
    t = run(f"{lang} train", tool, cwd, train_cmd)
    i = run(f"{lang} infer", tool, cwd, infer_cmd) if not isinstance(t, str) else t
    results[lang] = (t, i)

ttl = {}
if not skip_ttl:
    print("ttl (OHR を直接最適化、500 ステップ × 18 万行)")
    ttl["Python (JAX)"] = run("JAX", "python", "python", ["ttl_jax.py", "ohr"])
    ttl["Julia (Enzyme.jl)"] = run("Enzyme.jl", "julia", "julia", ["--project=diff", "diff/ttl_enzyme.jl", "ohr"])
    if with_autodiff:
        ttl["Rust (std::autodiff)"] = run("std::autodiff", "cargo", "rust-autodiff", ["+nightly", "run", "--release", "-q", "--", "ohr"],
                                          env={"RUSTFLAGS": "-Zautodiff=Enable"})


def cell(rows, task, key, model=None, fmt="{}"):
    if isinstance(rows, str):
        return "未実行"
    for r in rows:
        if r.get("task") == task and (model is None or r.get("model") == model):
            return fmt.format(float(r[key])) if key in r else "—"
    return "—"


lines = [f"環境: {platform.system()} {platform.machine()}, {os.cpu_count()} コア。各値は 1 回の実行。", ""]
lines += ["### 学習（18 万行 × 11 次元、4 エポック、バッチ 128）", "",
          "| 言語 | LR の学習 | MLP（11-16-1）の学習 | MLP のテスト AUC |", "|---|---|---|---|"]
for lang, (t, _) in results.items():
    note = f"（{t}）" if isinstance(t, str) else ""
    lines.append(f"| {lang}{note} | {cell(t, 'train', 'seconds', 'logreg', '{:.2f} 秒')} | "
                 f"{cell(t, 'train', 'seconds', 'mlp16', '{:.2f} 秒')} | {cell(t, 'train', 'auc', 'mlp16', '{:.4f}')} |")
lines += ["", "TS と Rust は 1 サンプルずつのループで、Python と Julia はミニバッチの行列演算で学習する（雛形の書き方のまま）。",
          "Julia は初回のコンパイルを除いた時間。"]
lines += ["", "### 推論（MLP 11-16-1、1 行あたり）", "",
          "| 言語 | 1 行ずつ（製品の判断と同じ使い方） | まとめて |", "|---|---|---|"]
for lang, (_, i) in results.items():
    lines.append(f"| {lang} | {cell(i, 'infer', 'one_row_ns', fmt='{:.0f} ns')} | {cell(i, 'infer', 'batch_ns', fmt='{:.0f} ns')} |")
lines += ["", "1 行ずつは、先頭 10 万行（Python は 2 万行）を 1 行ずつ推論した平均。TS は特徴量を 1 行ぶん複写する時間を含む。",
          "まとめては、10 万行を 1 回の行列演算で推論した平均。TS と Rust は 1 行ずつの経路しかないので同じ値。",
          "Python と Julia は、暖機の後に 3 回測った最速の値。"]
if ttl:
    lines += ["", "### 微分可能プログラミング（TTL、OHR を直接最適化、500 ステップ × 18 万行）", "",
              "| 実装 | 学習 | 初回のコンパイル | 補正後の soft OHR |", "|---|---|---|---|"]
    for name, r in ttl.items():
        note = f"（{r}）" if isinstance(r, str) else ""
        lines.append(f"| {name}{note} | {cell(r, 'ttl', 'seconds', fmt='{:.1f} 秒')} | "
                     f"{cell(r, 'ttl', 'compile_seconds', fmt='{:.1f} 秒')} | {cell(r, 'ttl', 'soft', fmt='{:.4f}')} |")
    if not with_autodiff:
        lines.append("| Rust (std::autodiff) | `--with-autodiff` で測る | — | — |")
    lines += ["", "JAX は jit、Enzyme.jl と Rust は 1 スレッド。Rust のコンパイルは `cargo build` の時に済むので「—」。"]

text = "\n".join(lines)
print("\n" + text)
if out_md:
    Path(out_md).write_text(text + "\n")
    print(f"\nwrote {out_md}")
