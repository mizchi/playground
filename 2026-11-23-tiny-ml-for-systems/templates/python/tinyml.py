"""小さいモデルを自前で作るための雛形 (Python 版)。依存は numpy だけ。

ts/src/nn.ts・rust/src/lib.rs と同じ構造:
  - 隠れ層 ReLU、出力 1 ユニット + sigmoid、損失は交差エントロピー (sizes=[d, 1] ならロジスティック回帰)
  - 誤差逆伝搬は手で書く:  δ_L = p - y,  ∂W_l = δ_lᵀ a_l,  δ_{l-1} = (δ_l W_l) ⊙ [a_{l-1} > 0]
  - Adam、ミニバッチ
  - 数値微分による勾配チェック、AUC、重みの JSON 書き出し (TS の MLP.toJSON と同じ形)

製品の中で推論するときは、to_json() で書き出した重みを TS / Rust の自前実装に読ませる。
Python のまま 1 行ずつ推論すると、1 回の呼び出しに数十 µs かかる (bench/README.md)。
"""

from __future__ import annotations

import json

import numpy as np


class MLP:
    def __init__(self, sizes: list[int], seed: int = 1):
        if sizes[-1] != 1:
            raise ValueError("output size must be 1")
        rng = np.random.default_rng(seed)
        self.sizes = sizes
        # W[l] は (out, in)。He 初期化
        self.W = [rng.normal(0, np.sqrt(2 / i), (o, i)) for i, o in zip(sizes, sizes[1:])]
        self.b = [np.zeros(o) for o in sizes[1:]]
        self._m = [np.zeros_like(p) for p in self.W + self.b]
        self._v = [np.zeros_like(p) for p in self.W + self.b]
        self._t = 0

    def param_count(self) -> int:
        return sum(p.size for p in self.W + self.b)

    def _forward(self, X: np.ndarray) -> tuple[list[np.ndarray], np.ndarray]:
        acts = [X]
        for l, (W, b) in enumerate(zip(self.W, self.b)):
            z = acts[-1] @ W.T + b
            acts.append(np.maximum(z, 0) if l < len(self.W) - 1 else z)
        p = 1 / (1 + np.exp(-acts[-1][:, 0]))
        return acts, p

    def predict(self, X: np.ndarray) -> np.ndarray:
        """X: (n, d) → P(y=1) の (n,)"""
        return self._forward(np.atleast_2d(X))[1]

    def grads(self, X: np.ndarray, y: np.ndarray, weight: np.ndarray | None = None):
        """バッチ平均の損失と勾配 (W の勾配のリスト + b の勾配のリスト) を返す。逆伝搬の本体"""
        n = len(y)
        w = np.ones(n) if weight is None else weight
        acts, p = self._forward(X)
        eps = 1e-12
        loss = -np.sum(w * (y * np.log(p + eps) + (1 - y) * np.log(1 - p + eps))) / n
        delta = ((p - y) * w / n)[:, None]  # δ_L = p - y (sigmoid + 交差エントロピー)
        gW, gb = [None] * len(self.W), [None] * len(self.W)
        for l in range(len(self.W) - 1, -1, -1):
            gW[l] = delta.T @ acts[l]
            gb[l] = delta.sum(0)
            if l > 0:
                delta = (delta @ self.W[l]) * (acts[l] > 0)  # ReLU の微分は 0 か 1
        return loss, gW, gb

    def step(self, gW, gb, lr: float = 3e-3) -> None:
        """Adam 1 ステップ"""
        self._t += 1
        c1, c2 = 1 - 0.9**self._t, 1 - 0.999**self._t
        for i, (p, g) in enumerate(zip(self.W + self.b, gW + gb)):
            self._m[i] = 0.9 * self._m[i] + 0.1 * g
            self._v[i] = 0.999 * self._v[i] + 0.001 * g * g
            p -= lr * (self._m[i] / c1) / (np.sqrt(self._v[i] / c2) + 1e-8)

    def fit(self, X, y, epochs=4, batch_size=128, lr=3e-3, pos_weight=1.0, seed=2, log=False):
        rng = np.random.default_rng(seed)
        X, y = np.asarray(X, np.float64), np.asarray(y, np.float64)
        for ep in range(epochs):
            order = rng.permutation(len(y))
            total = 0.0
            for s in range(0, len(y), batch_size):
                idx = order[s : s + batch_size]
                w = np.where(y[idx] > 0.5, pos_weight, 1.0)
                loss, gW, gb = self.grads(X[idx], y[idx], w)
                self.step(gW, gb, lr)
                total += loss * len(idx)
            if log:
                print(f"  epoch {ep + 1}/{epochs} loss={total / len(y):.4f}")
        return self

    def to_json(self) -> str:
        """TS の MLP.toJSON() と同じ形: W は (out, in) の row-major を平らにしたもの"""
        return json.dumps({"sizes": self.sizes, "W": [W.ravel().tolist() for W in self.W], "b": [b.tolist() for b in self.b]})

    @classmethod
    def from_json(cls, text: str) -> "MLP":
        d = json.loads(text)
        m = cls(d["sizes"])
        m.W = [np.array(w).reshape(o, i) for w, i, o in zip(d["W"], d["sizes"], d["sizes"][1:])]
        m.b = [np.array(b) for b in d["b"]]
        return m


def gradcheck(m: MLP, X, y, h: float = 1e-5) -> float:
    """解析的勾配と数値微分の最大相対誤差。逆伝搬を書き換えたら必ず回す"""
    _, gW, gb = m.grads(X, y)
    worst = 0.0
    for p, g in zip(m.W + m.b, gW + gb):
        for i in np.ndindex(p.shape):
            orig = p[i]
            p[i] = orig + h
            lp = m.grads(X, y)[0]
            p[i] = orig - h
            lm = m.grads(X, y)[0]
            p[i] = orig
            num = (lp - lm) / (2 * h)
            worst = max(worst, abs(num - g[i]) / max(1e-8, abs(num) + abs(g[i])))
    return worst


def auc(scores, labels) -> float:
    """ROC AUC (順位統計)"""
    order = np.argsort(scores, kind="stable")
    labels = np.asarray(labels)[order] > 0.5
    pos = labels.sum()
    neg = len(labels) - pos
    rank_sum = np.sum(np.nonzero(labels)[0] + 1)
    return float((rank_sum - pos * (pos + 1) / 2) / (pos * neg))
