"""推論側の使い方 (Python 版)。どの言語の雛形が書いた重み (JSON) でも読み込んで推論する。

  python infer.py <weights.json> [学習側が書いたスコア.f32] [--ttl]

1 行ずつの推論 (製品の判断と同じ使い方) と、まとめての推論の両方を測る。
Python は 1 回の呼び出しの手間が大きいので、製品の中の判断には TS か Rust を使う。ここは比較と検証用。
"""

import sys
import time
from pathlib import Path

import numpy as np

from tinyml import MLP

args = [a for a in sys.argv[1:] if a != "--ttl"]
ttl_mode = "--ttl" in sys.argv
if not args:
    sys.exit("usage: python infer.py <weights.json> [scores.f32] [--ttl]")
weights = Path(args[0])
model = MLP.from_json(weights.read_text())
d = model.sizes[0]
data = (Path(__file__).resolve().parent / "../../bench/data").resolve()
X = np.fromfile(data / "cache_X.f32", "<f4").reshape(-1, d)[:100_000].astype(np.float64)
rows = len(X)


def logit(x):
    a = x
    for l, (W, b) in enumerate(zip(model.W, model.b)):
        z = W @ a + b
        a = np.maximum(z, 0) if l < len(model.W) - 1 else z
    return a[0]


def best_of(n, f):
    """暖機を 1 回してから n 回測り、一番速い回の秒数と結果を返す"""
    out = f()
    best = float("inf")
    for _ in range(n):
        t0 = time.perf_counter()
        out = f()
        best = min(best, time.perf_counter() - t0)
    return best, out


# 1 行ずつ (製品の判断と同じ使い方)。10 万行は遅いので先頭 2 万行で測る
k = 20_000
sec, one = best_of(3, lambda: np.array([logit(X[i]) for i in range(k)]))
one_ns = sec * 1e9 / k

# まとめて
sec, (acts, p) = best_of(3, lambda: model._forward(X))
batch_ns = sec * 1e9 / rows
z = acts[-1][:, 0]
ours = np.exp(z) if ttl_mode else p

name = "/".join(weights.parts[-3:])
line = f"py infer {name}: {rows} rows, 1-row {one_ns:.0f} ns/row, batch {batch_ns:.0f} ns/row"
line += f", median TTL {np.median(ours):.0f}" if ttl_mode else f", admit {np.mean(p > 0.5) * 100:.1f}%"
if len(args) > 1:
    theirs = np.fromfile(args[1], "<f4")[:rows]
    diff = np.abs(ours - theirs)
    if ttl_mode:
        diff = diff / np.maximum(1e-9, np.abs(theirs))
    line += f", max {'relative ' if ttl_mode else ''}|diff| vs trainer = {diff.max():.1e}"
assert np.allclose(1 / (1 + np.exp(-one)), p[:k]), "1-row and batch disagree"
print(line)
print(f"bench lang=python task=infer one_row_ns={one_ns:.0f} batch_ns={batch_ns:.0f}")
