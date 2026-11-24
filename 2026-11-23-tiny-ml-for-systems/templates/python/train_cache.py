"""雛形の使い方の例: キャッシュ admission のモデルを学習し、評価し、重みとスコアを書き出す。

  python train_cache.py [../../bench/data]

  1. データ: bench/data の cache_X.f32 (60 万 × 11) と cache_y.f32 (H リクエスト以内にまた来るか)
     作り方は bench/README.md (DUMP_DIR=bench/data node ts/src/cache-admission.ts)
  2. 時系列で分ける: 先頭 30% で学習、残りで評価
  3. 書き出し: out/<名前>.json (重み。TS / Rust の自前推論が読む) と
              <data>/scores/py-template-<名前>.f32 (全行のスコア。bench/rust の sim がヒット率を測る)
"""

import sys
import time
from pathlib import Path

import numpy as np

from tinyml import MLP, auc

data = Path(sys.argv[1] if len(sys.argv) > 1 else Path(__file__).resolve().parent / "../../bench/data").resolve()
n, d = 600_000, 11
X = np.fromfile(data / "cache_X.f32", "<f4").reshape(n, d).astype(np.float64)
y = np.fromfile(data / "cache_y.f32", "<f4").astype(np.float64)
train = n * 3 // 10

out = Path(__file__).resolve().parent / "out"
out.mkdir(exist_ok=True)
(data / "scores").mkdir(exist_ok=True)
for name, sizes in [("logreg", [d, 1]), ("mlp16", [d, 16, 1])]:
    t0 = time.perf_counter()
    m = MLP(sizes, seed=1).fit(X[:train], y[:train], epochs=4, batch_size=128, lr=3e-3)
    train_s = time.perf_counter() - t0
    scores = m.predict(X)
    print(f"{name:<7} {m.param_count():>4} params  train {train_s:.2f}s  test AUC {auc(scores[train:], y[train:]):.4f}")
    print(f"bench lang=python task=train model={name} seconds={train_s:.3f} auc={auc(scores[train:], y[train:]):.4f}")
    (out / f"{name}.json").write_text(m.to_json())
    scores.astype("<f4").tofile(data / "scores" / f"py-template-{name}.f32")
print(f"wrote out/*.json and {data / 'scores'}/py-template-*.f32")
