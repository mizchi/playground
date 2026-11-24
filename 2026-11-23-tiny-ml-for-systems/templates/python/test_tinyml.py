"""逆伝搬の勾配チェックと XOR。  python test_tinyml.py"""

import numpy as np

from tinyml import MLP, auc, gradcheck

rng = np.random.default_rng(1)
X = rng.uniform(-1, 1, (8, 5))
y = (rng.uniform(size=8) < 0.5).astype(float)
m = MLP([5, 7, 4, 1], seed=1)
err = gradcheck(m, X, y)
print(f"gradient check: max relative error = {err:.2e} ({m.param_count()} params)")
assert err < 1e-6, "backprop is wrong"

Xx = np.array([[0, 0], [0, 1], [1, 0], [1, 1]], float)
yx = np.array([0, 1, 1, 0], float)
xor = MLP([2, 16, 1], seed=7).fit(Xx, yx, epochs=2000, batch_size=4, lr=0.02)
out = xor.predict(Xx)
print("XOR:", " ".join(f"{v:.3f}" for v in out), "(expected ~0 1 1 0)")
assert np.array_equal(np.round(out), yx)

assert abs(auc([0.1, 0.4, 0.35, 0.8], [0, 0, 1, 1]) - 0.75) < 1e-12
round_trip = MLP.from_json(xor.to_json())
assert np.allclose(round_trip.predict(Xx), out)
print("OK")
