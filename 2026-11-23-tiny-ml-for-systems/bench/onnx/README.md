# onnx：他所で学習したモデルを ONNX で持ち出す経路

`GUIDE.md` で「未計測」のまま残っていた経路（LightGBM → ONNX → JS・Rust で推論）を測った。
比べるために、自前の MLP（11-16-1）も ONNX にして、同じランタイムで動かした。

```bash
../python/.venv/bin/python export.py      # lgbm.onnx・mlp16.onnx と、Python の予測 (ref_*.f32) を書く
cd ts && npm install && node bench.ts      # onnxruntime-node
cd ../rust && cargo run --release          # tract (純 Rust)
ORT_DYLIB_PATH=../ts/node_modules/onnxruntime-node/bin/napi-v6/linux/x64/libonnxruntime.so.1 \
  cargo run --release --features ort       # ort (ONNX Runtime の Rust ラッパー)
```

`export.py` は `../python/.venv` に `onnx onnxmltools onnxruntime` を足して動かす。
`mlp16.onnx` は `../../templates/rust/out/mlp16.json` から作るので、先に `templates/rust` の `train_cache` を回しておく。

## 結果（1 スレッド、各 1 回の実行）

| モデル | 実行方法 | 1 行ずつ | まとめて（1 行あたり） | Python の予測との差 |
|---|---|---|---|---|
| MLP 11-16-1 | TS 自前（`templates/ts`） | 427 ns | 427 ns | 0 |
| MLP 11-16-1 | Rust 自前（`templates/rust`） | 105〜185 ns | 105〜185 ns | 2.4e-7 |
| MLP 11-16-1 | onnxruntime-node 1.31.1（TS、`await`） | 9.2 µs | 53 ns | 1.8e-7 |
| MLP 11-16-1 | `ort` 2.0.0-rc.13（Rust、ONNX Runtime 1.31.1） | 4.6 µs | 26 ns | 1.8e-7 |
| MLP 11-16-1 | `tract` 0.23（Rust） | 4.1〜5.1 µs | 34〜45 ns | 1.8e-7 |
| MLP 11-16-1 | onnxruntime 1.32.0（Python） | 5.4 µs | 49 ns | 1.8e-7 |
| LightGBM 100 本 | onnxruntime-node 1.31.1（TS、`await`） | 12.1 µs | 2.6 µs | 2.1e-7 |
| LightGBM 100 本 | `ort`（Rust） | 9.8 µs | 3.0 µs | 2.1e-7 |
| LightGBM 100 本 | `tract`（Rust） | 1.1 ms | 1.07 ms | 2.1e-7 |
| LightGBM 100 本 | onnxruntime（Python） | 7.7 µs | 2.5 µs | 2.1e-7 |

自前の MLP の値は `../../templates/RESULT.md` から。
LightGBM を Python のまま推論すると、1 行ずつで 22.7 µs、まとめて 679 ns（4 スレッド、`../README.md`）。

## 読み取れること

- **ONNX ランタイムは、1 回の呼び出しに 4.6〜9.2 µs かかる。** 小さい MLP を 1 行ずつ判断する使い方では、自前の推論のほうが 25〜90 倍速い。まとめて推論するなら、ランタイムのほうが速い（26〜53 ns と、自前の 105〜427 ns）。
- **onnxruntime-node の `run()` は非同期**（Promise を返す）。キャッシュの `get` のような同期の判断の中では呼べない。
- **LightGBM を JS・Rust で動かすなら、ONNX Runtime（onnxruntime-node・`ort`）で 1 行 10〜12 µs。** ms 単位の予算がある判断なら使える。予測は Python と 2.1e-7 以内で一致した。
- **`tract` は、木のモデル（TreeEnsembleClassifier）が 1 行 1 ms と、実用にならない遅さだった。** NN（MLP）なら `ort` と同じくらい速い。
- **`ort` は、既定ではビルド時に `cdn.pyke.io` から ONNX Runtime を取得しようとし、この環境のネットワーク設定で塞がれた。** `load-dynamic` にして、npm の onnxruntime-node に入っている `libonnxruntime.so.1` を実行時に読み込ませると動いた。
- 変換は `onnxmltools` で 1 行。ただし変換後のモデルは標準演算の opset が古く、出力を整えるのに `Slice` が使えず `Gather` を使った。
