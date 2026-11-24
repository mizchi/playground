"""ONNX の経路を測るためのモデルを 2 つ書き出す。

  ../python/.venv/bin/python export.py

  1. lgbm.onnx   キャッシュ admission の LightGBM (100 本、bench/python/bench.py と同じ設定)。onnxmltools で変換
  2. mlp16.onnx  templates/rust/out/mlp16.json の自前 MLP を、MatMul・Add・Relu・Sigmoid の ONNX グラフにしたもの

どちらも入力は "x" (n × 11 の float32)、出力は "p" (n × 1、P(y=1))。
比べる相手として、Python で計算した予測を ref_lgbm.f32・ref_mlp16.f32 (先頭 10 万行) に書く。
"""

import json
import time
from pathlib import Path

import lightgbm as lgb
import numpy as np
import onnx
import onnxruntime as ort
from onnx import TensorProto, helper, numpy_helper
from onnxmltools import convert_lightgbm
from onnxmltools.convert.common.data_types import FloatTensorType

HERE = Path(__file__).resolve().parent
DATA = HERE.parent / "data"
n, d = 600_000, 11
train = n * 3 // 10
X = np.fromfile(DATA / "cache_X.f32", "<f4").reshape(n, d)
y = np.fromfile(DATA / "cache_y.f32", "<f4")
rows = 100_000

# ---- 1. LightGBM → ONNX
clf = lgb.LGBMClassifier(n_estimators=100, random_state=1, verbose=-1).fit(X[:train], y[:train])
lgbm = convert_lightgbm(clf, initial_types=[("x", FloatTensorType([None, d]))], zipmap=False, target_opset=15)
# 出力を "p" (P(y=1) の 1 列) にそろえる: probabilities (n×2) の 2 列目を取り出すノードを足す
g = lgbm.graph
prob = [o.name for o in g.output if "prob" in o.name][0]
# (変換後のモデルは標準演算の opset が古いので、Slice ではなく opset 1 からある Gather を使う)
g.initializer.append(numpy_helper.from_array(np.array([1], np.int64), "col1"))
g.node.append(helper.make_node("Gather", [prob, "col1"], ["p"], axis=1))
del g.output[:]
g.output.append(helper.make_tensor_value_info("p", TensorProto.FLOAT, [None, 1]))
onnx.checker.check_model(lgbm)
onnx.save(lgbm, HERE / "lgbm.onnx")
ref = clf.predict_proba(X[:rows])[:, 1].astype("<f4")
ref.tofile(HERE / "ref_lgbm.f32")

# ---- 2. 自前 MLP (JSON) → ONNX
w = json.loads((HERE / "../../templates/rust/out/mlp16.json").read_text())
sizes = w["sizes"]
nodes, inits, cur = [], [], "x"
for l in range(len(sizes) - 1):
    W = np.array(w["W"][l], np.float32).reshape(sizes[l + 1], sizes[l]).T  # (in, out)
    b = np.array(w["b"][l], np.float32)
    inits += [numpy_helper.from_array(W, f"W{l}"), numpy_helper.from_array(b, f"b{l}")]
    nodes += [helper.make_node("MatMul", [cur, f"W{l}"], [f"m{l}"]), helper.make_node("Add", [f"m{l}", f"b{l}"], [f"z{l}"])]
    cur = f"z{l}"
    if l < len(sizes) - 2:
        nodes.append(helper.make_node("Relu", [cur], [f"a{l}"]))
        cur = f"a{l}"
nodes.append(helper.make_node("Sigmoid", [cur], ["p"]))
graph = helper.make_graph(nodes, "mlp16", [helper.make_tensor_value_info("x", TensorProto.FLOAT, [None, d])],
                          [helper.make_tensor_value_info("p", TensorProto.FLOAT, [None, 1])], inits)
mlp = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 15)])
mlp.ir_version = 8
onnx.checker.check_model(mlp)
onnx.save(mlp, HERE / "mlp16.onnx")
h = np.maximum(X[:rows].astype(np.float64) @ np.array(w["W"][0]).reshape(16, d).T + np.array(w["b"][0]), 0)
ref_mlp = (1 / (1 + np.exp(-(h @ np.array(w["W"][1]) + w["b"][1][0])))).astype("<f4")
ref_mlp.tofile(HERE / "ref_mlp16.f32")

# ---- Python の onnxruntime で、変換が合っているかと速さを確かめる
for name, ref_ in [("lgbm", ref), ("mlp16", ref_mlp)]:
    so = ort.SessionOptions()
    so.intra_op_num_threads = 1  # TS・Rust と同じ 1 スレッド
    sess = ort.InferenceSession(str(HERE / f"{name}.onnx"), so, providers=["CPUExecutionProvider"])
    xb = X[:rows]
    out = sess.run(["p"], {"x": xb})[0][:, 0]
    t0 = time.perf_counter()
    out = sess.run(["p"], {"x": xb})[0][:, 0]
    batch_ns = (time.perf_counter() - t0) * 1e9 / rows
    k = 20_000
    t0 = time.perf_counter()
    for i in range(k):
        sess.run(["p"], {"x": xb[i : i + 1]})
    one_ns = (time.perf_counter() - t0) * 1e9 / k
    print(f"python onnxruntime {ort.__version__} {name}: 1-row {one_ns:.0f} ns, batch {batch_ns:.0f} ns/row, "
          f"max |diff| vs python {np.abs(out - ref_).max():.1e}")
    print(f"bench lang=python-ort task=onnx model={name} one_row_ns={one_ns:.0f} batch_ns={batch_ns:.0f}")
print("wrote lgbm.onnx, mlp16.onnx, ref_lgbm.f32, ref_mlp16.f32")
