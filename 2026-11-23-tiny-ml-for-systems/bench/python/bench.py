"""Python 側のベンチ。Rust 側 (../rust/src/main.rs) と同じデータ・同じ指標で測る。

  uv run --python .venv/bin/python bench.py

指標:
  train   学習の壁時計時間
  AUC     テストデータの ROC AUC
  1-row   1 行ずつ推論したときの 1 行あたりの時間 (システムの判断部品はこの使い方になる)
  batch   全テスト行をまとめて推論したときの 1 行あたりの時間
"""

import os
import time
import warnings
from pathlib import Path

import lightgbm as lgb
import numpy as np
import sklearn
from sklearn.ensemble import HistGradientBoostingClassifier
from sklearn.exceptions import ConvergenceWarning
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import roc_auc_score
from sklearn.neural_network import MLPClassifier

warnings.filterwarnings("ignore", category=ConvergenceWarning)
DATA = Path(__file__).resolve().parent.parent / "data"


def f32(name):
    return np.fromfile(DATA / name, dtype="<f4")


def load_bloom():
    d = 256
    return (f32("bloom_train_X.f32").reshape(-1, d), f32("bloom_train_y.f32"),
            f32("bloom_test_X.f32").reshape(-1, d), f32("bloom_test_y.f32"))


def load_cache():
    d, n = 11, 600_000
    train = n * 3 // 10
    x, y = f32("cache_X.f32").reshape(n, d), f32("cache_y.f32")
    return x[:train], y[:train], x[train:], y[train:]


def fmt_ns(ns):
    return f"{ns / 1000:.1f}µs" if ns >= 1000 else f"{ns:.0f}ns"


def bench(task, name, fit, proba, data, one_row=None):
    xtr, ytr, xte, yte = data
    t0 = time.perf_counter()
    model = fit(xtr, ytr)
    train_s = time.perf_counter() - t0
    t0 = time.perf_counter()
    scores = proba(model, xte)
    batch_ns = (time.perf_counter() - t0) * 1e9 / len(yte)
    k = 2000
    one = one_row or (lambda m, row: proba(m, row))
    t0 = time.perf_counter()
    for i in range(k):
        one(model, xte[i : i + 1])
    one_ns = (time.perf_counter() - t0) * 1e9 / k
    print(f"{task:<6} {name:<28} train {train_s:>7.2f}s  AUC {roc_auc_score(yte, scores):.4f}  "
          f"1-row {fmt_ns(one_ns):>9}  batch {fmt_ns(batch_ns):>9}", flush=True)


sk = lambda m, x: m.predict_proba(x)[:, 1]
lgb_proba = lambda m, x: m.predict(x)


def lgb_fit(x, y):
    return lgb.train({"objective": "binary", "verbose": -1, "seed": 1}, lgb.Dataset(x, y), num_boost_round=100)


def mlp(hidden, batch, epochs):
    # Rust 側の自前 MLP と同じ: Adam, lr 3e-3, ミニバッチ, エポック数固定
    return lambda x, y: MLPClassifier(hidden_layer_sizes=hidden, solver="adam", learning_rate_init=3e-3,
                                      batch_size=batch, max_iter=epochs, random_state=1).fit(x, y)


print(f"python {'.'.join(map(str, os.sys.version_info[:3]))}, numpy {np.__version__}, "
      f"scikit-learn {sklearn.__version__}, lightgbm {lgb.__version__}, cores {os.cpu_count()}")
b = load_bloom()
bench("url", "sklearn LogisticRegression", lambda x, y: LogisticRegression(max_iter=200).fit(x, y), sk, b)
bench("url", "sklearn MLP (8,)", mlp((8,), 64, 8), sk, b)
bench("url", "lightgbm 100 trees", lgb_fit, lgb_proba, b)
bench("url", "sklearn HistGB", lambda x, y: HistGradientBoostingClassifier(random_state=1).fit(x, y), sk, b)
c = load_cache()
bench("cache", "sklearn LogisticRegression", lambda x, y: LogisticRegression(max_iter=200).fit(x, y), sk, c)
bench("cache", "sklearn MLP (16,)", mlp((16,), 128, 4), sk, c)
bench("cache", "lightgbm 100 trees", lgb_fit, lgb_proba, c)
bench("cache", "sklearn HistGB", lambda x, y: HistGradientBoostingClassifier(random_state=1).fit(x, y), sk, c)
