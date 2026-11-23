# bench: Rust と Python の比較

「Rust で Python に迫る性能が出せるか」を、この実験と同じ 2 つのデータで測った。

- url：URL の文字 n-gram を 256 次元にハッシュした特徴量。学習 6 万行、テスト 4 万行（学習に使っていない URL）
- cache：キャッシュ admission の 11 次元の特徴量。60 万リクエストのうち、先頭 30% で学習し、残り 70% で評価（時系列で分割）

## 動かし方

```bash
cd ../ && DUMP_DIR=bench/data node ts/src/cache-admission.ts    # cache のデータ
cd bench/rust && RUSTC_BOOTSTRAP=1 cargo run --release -- dump-bloom   # url のデータ
RUSTC_BOOTSTRAP=1 cargo run --release -- run                           # Rust 側
cd ../python && uv venv .venv && uv pip install --python .venv/bin/python numpy scikit-learn lightgbm
.venv/bin/python bench.py                                              # Python 側
```

`perpetual` 2.5.0 は `#![feature(array_ptr_get)]` を使っており、stable の rustc ではビルドできない。
`RUSTC_BOOTSTRAP=1` で stable のコンパイラに nightly の機能を許可して測った。

## 結果（4 コア、Linux）

```
threads: own = 1, perpetual = rayon default (4 cores)
url    own logreg (1 thread)        train    0.26s  AUC 0.9824  1-row     282ns  batch     282ns
url    own MLP [8] (1 thread)       train    0.79s  AUC 0.9842  1-row     1.0µs  batch     1.0µs
url    perpetual budget=0.5 (123 trees) train  304.41s  AUC 0.9951  1-row   142.7µs  batch    24.9µs
cache  own MLP [16] (1 thread)      train    0.25s  AUC 0.9338  1-row      91ns  batch      91ns
cache  perpetual budget=0.5 (38 trees) train    1.38s  AUC 0.9316  1-row     4.9µs  batch     1.1µs
cache  perpetual budget=1 (72 trees) train    2.48s  AUC 0.9327  1-row    13.5µs  batch     2.7µs
```

```
python 3.13.18, numpy 2.5.5, scikit-learn 1.9.1, lightgbm 4.7.0, cores 4
url    sklearn LogisticRegression   train    0.60s  AUC 0.9842  1-row    70.9µs  batch     215ns
url    sklearn MLP (8,)             train    1.34s  AUC 0.9841  1-row    63.0µs  batch     371ns
url    lightgbm 100 trees           train    2.01s  AUC 0.9944  1-row    22.7µs  batch     1.7µs
url    sklearn HistGB               train    3.29s  AUC 0.9943  1-row   871.9µs  batch     4.0µs
cache  sklearn LogisticRegression   train    0.45s  AUC 0.9311  1-row    90.3µs  batch      26ns
cache  sklearn MLP (16,)            train    1.48s  AUC 0.9332  1-row    69.1µs  batch     115ns
cache  lightgbm 100 trees           train    0.37s  AUC 0.9332  1-row    22.7µs  batch     679ns
cache  sklearn HistGB               train    0.82s  AUC 0.9328  1-row   922.5µs  batch     1.1µs
```

1-row は 1 行ずつ推論したときの 1 行あたりの時間で、システムの判断部品はこの使い方になる。
batch は全テスト行をまとめて推論したときの 1 行あたりの時間。

## 読み取れること

| 種類 | Rust | Python | 判定 |
|---|---|---|---|
| LR・小さい MLP の学習 | 自前 0.26 s / 0.79 s | sklearn 0.60 s / 1.34 s | Rust が同等以上。AUC も同じ水準 |
| LR・小さい MLP の 1 行推論 | 91 ns〜1.0 µs | 63〜90 µs | Rust が 60〜760 倍速い。Python は 1 回の呼び出しの手間が大きい |
| まとめて推論 | 91 ns〜1.0 µs/行 | 26〜371 ns/行 | ほぼ同じ。Python も中身は BLAS |
| GBDT の学習 | perpetual 1.4〜304 s | LightGBM 0.4〜2.0 s | LightGBM（C++）が 4〜150 倍速い。256 次元では perpetual が遅すぎる |
| GBDT の精度 | AUC 0.9951 / 0.9327 | AUC 0.9944 / 0.9332 | ほぼ同じ |

この 2 つのデータでは、GBDT の AUC の上乗せは小さかった（url で 0.984 → 0.995、cache ではほぼ 0）。

## 測っていないこと

- PyTorch。download.pytorch.org がこの環境のネットワーク設定で塞がれていて、PyPI の wheel は CUDA の依存で大きすぎた。
- Rust から LightGBM の C ライブラリを呼ぶ場合（`lightgbm3` など）。中身は Python 版と同じ C++ なので学習速度は同等と予想するが、未計測。
- GPU。
- 実行のばらつき。各 1 回の計測。
