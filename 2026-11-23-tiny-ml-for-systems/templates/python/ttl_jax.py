"""微分可能プログラミング (JAX) で、キャッシュの TTL を「容量の制約の下でのヒット」そのものに対して最適化する。

  python ttl_jax.py [bhr|ohr]      (データは ../../bench/data、作り方は bench/README.md)

考え方:
  各リクエスト s に TTL_s = exp(f_θ(x_s)) を割り当てる (f_θ は 11-16-1 の MLP、x_s はキャッシュ実験の特徴量)。
  TTL 付きキャッシュ (アクセスのたびに TTL を更新) では、次のアクセスまでの間隔 g_s について
      ヒット   = [g_s <= TTL_s]
      占有量   = size_s * min(g_s, TTL_s)      (リクエスト数を時間の単位にした、バイト×時間)
  になる。ヒットの段差を sigmoid でならすと、全体が θ について微分できる:
      soft_hit = sigmoid((log TTL_s - log g_s) / tau)
  「平均占有量 <= 容量」の制約は、容量を超えた分だけに二乗の罰則をかけて扱う。
  学習の後、バイアス b2 を二分探索でずらして、学習区間の平均占有量をちょうど容量に合わせる。

  近似のラベル (H 以内にまた来るか) を使わず、ヒットと容量を直接扱うのが、手で逆伝搬を書く版との違い。
  学習は先頭 30% だけを使い、学習区間の外に出る間隔は「来なかった」(∞) として扱う (未来を見ない)。
"""

import json
import sys
import time
from pathlib import Path

import jax
import jax.numpy as jnp
import numpy as np

jax.config.update("jax_enable_x64", True)
DATA = (Path(__file__).resolve().parent / "../../bench/data").resolve()
meta = json.load(open(DATA / "cache.json"))
N, D, TR, CAP, H0 = meta["n"], meta["d"], meta["train"], float(meta["capacity"]), meta["H"]

X = np.fromfile(DATA / "cache_X.f32", "<f4").reshape(N, D)
ids = np.fromfile(DATA / "cache_id.u32", "<u4")
size = np.fromfile(DATA / "cache_size.u32", "<u4").astype(np.float64)

# 次に同じ id が来るまでの間隔。学習区間 [0, TR) の外に出るものは ∞
gap = np.full(N, np.inf)
last = {}
for t in range(N - 1, -1, -1):
    n = last.get(ids[t])
    if n is not None and n < TR:
        gap[t] = n - t
    last[ids[t]] = t
Xtr, gtr, ztr = jnp.asarray(X[:TR], jnp.float64), jnp.asarray(gap[:TR]), jnp.asarray(size[:TR])
LOG_INF = 30.0
lgtr = jnp.where(jnp.isinf(gtr), LOG_INF, jnp.log(jnp.where(jnp.isinf(gtr), 1.0, gtr)))

TAU = 0.25
TARGET = sys.argv[1] if len(sys.argv) > 1 else "bhr"  # bhr: バイトで数える / ohr: リクエスト数で数える
w = ztr / ztr.sum() if TARGET == "bhr" else jnp.ones(TR) / TR


def log_ttl(p, x):
    h = jax.nn.relu(x @ p["W1"].T + p["b1"])
    return h @ p["w2"] + p["b2"]


def terms(p):
    lt = log_ttl(p, Xtr)
    hit = jnp.sum(w * jax.nn.sigmoid((lt - lgtr) / TAU))
    occ = jnp.sum(ztr * jnp.minimum(gtr, jnp.exp(lt))) / TR
    return hit, occ


MU = 10.0


def loss(p):
    hit, occ = terms(p)
    return -hit + MU * jnp.square(jax.nn.relu(occ / CAP - 1.0))


grad = jax.jit(jax.value_and_grad(loss))
terms_j = jax.jit(terms)

# 初期値は Rust 版と同じ決定的な値 (乱数を使わない) にして、両者の結果を比べられるようにする
H = 16
W1 = np.array([[np.sin(1.0 + 0.7 * i + 0.3 * j) * 0.3 for j in range(D)] for i in range(H)])
p = {"W1": jnp.asarray(W1), "b1": jnp.full(H, 0.1), "w2": jnp.full(H, 0.0), "b2": jnp.asarray(np.log(H0))}

lr, steps = 0.01, 500
m = jax.tree.map(jnp.zeros_like, p)
v = jax.tree.map(jnp.zeros_like, p)
t0 = time.perf_counter()
for k in range(1, steps + 1):
    val, g = grad(p)
    m = jax.tree.map(lambda a, b: 0.9 * a + 0.1 * b, m, g)
    v = jax.tree.map(lambda a, b: 0.999 * a + 0.001 * b * b, v, g)
    p = jax.tree.map(lambda q, a, b: q - lr * (a / (1 - 0.9**k)) / (jnp.sqrt(b / (1 - 0.999**k)) + 1e-8), p, m, v)
    if k in (1, 100, 250, 500):
        hit, occ = terms_j(p)
        print(f"step {k:>3}  loss {float(val):.4f}  soft {TARGET} {float(hit):.4f}  occupancy/capacity {float(occ) / CAP:.3f}")
train_s = time.perf_counter() - t0

# バイアスをずらして、学習区間の平均占有量をちょうど容量に合わせる (占有量は b2 について単調)
lo, hi = -20.0, 20.0
for _ in range(60):
    mid = (lo + hi) / 2
    occ = float(terms_j({**p, "b2": p["b2"] + mid})[1])
    lo, hi = (mid, hi) if occ < CAP else (lo, mid)
p = {**p, "b2": p["b2"] + lo}
hit, occ = terms_j(p)
print(f"calibrated: soft {TARGET} {float(hit):.4f}  occupancy/capacity {float(occ) / CAP:.3f}")
print(f"train {train_s:.2f}s ({steps} steps, full batch {TR} rows, jit)")

# 重みを共通の JSON 形式で書く (TS / Rust の MLP.fromJSON で読み、exp(predictLogit(x)) が TTL)
out = Path(__file__).resolve().parent / "out"
out.mkdir(exist_ok=True)
(out / f"ttl-jax-{TARGET}.json").write_text(json.dumps({
    "sizes": [D, H, 1],
    "W": [np.asarray(p["W1"]).ravel().tolist(), np.asarray(p["w2"]).tolist()],
    "b": [np.asarray(p["b1"]).tolist(), [float(p["b2"])]],
}))

ttl = np.exp(np.asarray(log_ttl(p, jnp.asarray(X, jnp.float64)))).astype("<f4")
(DATA / "ttl").mkdir(exist_ok=True)
ttl.tofile(DATA / "ttl" / f"py-template-jax-{TARGET}.f32")
print(f"wrote ttl/py-template-jax-{TARGET}.f32  median TTL {np.median(ttl):.0f} requests")
