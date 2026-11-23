# bench: Rust と Python の比較

「Rust で Python に迫る性能が出せるか」を、この実験と同じ 2 つのデータで測った。

- url：URL の文字 n-gram を 256 次元にハッシュした特徴量。学習 6 万行、テスト 4 万行（学習に使っていない URL）
- cache：キャッシュ admission の 11 次元の特徴量。60 万リクエストのうち、先頭 30% で学習し、残り 70% で評価（時系列で分割）

## 動かし方

```bash
cd ../ && DUMP_DIR=bench/data node ts/src/cache-admission.ts    # cache のデータ
cd bench/rust && RUSTC_BOOTSTRAP=1 cargo run --release -- dump-bloom   # url のデータ
RUSTC_BOOTSTRAP=1 cargo run --release -- run                           # Rust 側 (perpetual を含む。5 分ほど)
RUSTC_BOOTSTRAP=1 cargo run --release -- run-own                       # Rust 側の自前の実装だけ
cd ../.. && node bench/ts/bench.ts                                     # TS 側の自前の実装
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

```
node 22.23.5
url    own logreg (1 thread)        train    1.01s  AUC 0.9824  1-row     1.1µs  batch     1.1µs
url    own MLP [8] (1 thread)       train    3.25s  AUC 0.9849  1-row     3.4µs  batch     3.4µs
cache  own logreg (1 thread)        train    0.17s  AUC 0.9321  1-row      86ns  batch      86ns
cache  own MLP [16] (1 thread)      train    0.68s  AUC 0.9341  1-row     366ns  batch     366ns
```

```
url    own logreg (1 thread)        train    0.29s  AUC 0.9824  1-row     306ns  batch     306ns
url    own MLP [8] (1 thread)       train    0.94s  AUC 0.9842  1-row     1.3µs  batch     1.3µs
cache  own logreg (1 thread)        train    0.10s  AUC 0.9324  1-row      30ns  batch      30ns
cache  own MLP [16] (1 thread)      train    0.27s  AUC 0.9338  1-row     110ns  batch     110ns
```

上は TS（`bench/ts/bench.ts`）、下は Rust の `run-own`。

1-row は 1 行ずつ推論したときの 1 行あたりの時間で、システムの判断部品はこの使い方になる。
batch は全テスト行をまとめて推論したときの 1 行あたりの時間。

## サンプルごとの比較

### セットアップの量

| 手段 | 依存 | 入れる手順 | 大きさ・時間 | 注意 |
|---|---|---|---|---|
| TS 自前 | 0 | なし（Node 22.18 以上） | 0 | `nn.ts` 201 行を自分で持つ |
| Rust 自前 | 0 crate | なし | クリーンビルド 0.7 秒 | `lib.rs` の RNG 32 行 + MLP 132 行 |
| `perpetual` | 36 crate | `cargo add perpetual` | クリーンビルド 18.8 秒、target 95MB、バイナリ 1.7MB | nightly が必要（`RUSTC_BOOTSTRAP=1` で回避） |
| Python（scikit-learn + LightGBM） | 8 パッケージ | `uv venv` と `uv pip install numpy scikit-learn lightgbm` | venv 232MB | 推論を TS・Rust に持ち出すには別の経路が要る（ONNX など） |

### 解くまでのコード（cache の例。読み込みと時間計測を除く）

```ts
// TS 自前（nn.ts を手元に置く）
const m = new MLP([11, 16, 1], mulberry32(1));
m.fit(X, y, { epochs: 4, batchSize: 128, lr: 3e-3, rng: mulberry32(2) });
const p = m.predict(x);
```

```rust
// Rust 自前（lib.rs を手元に置く）
let mut m = Mlp::new(&[11, 16, 1], &mut Rng::new(1));
m.fit(&x, &y, 4, 128, 3e-3, &mut Rng::new(2));
let p = m.predict(&row);
```

```rust
// perpetual（列優先の f64 に並べ替える必要がある）
let xcol = col_major(&x, n, d);
let mut m = PerpetualBooster::default().set_objective(Objective::LogLoss).set_budget(0.5);
m.fit(&Matrix::new(&xcol, n, d), &y64, None, None)?;
let p = m.predict_proba(&Matrix::new(&row64, 1, d), false, false)[0];
```

```python
# Python
m = LogisticRegression(max_iter=200).fit(X, y); p = m.predict_proba(X_test)[:, 1]
b = lgb.train({"objective": "binary"}, lgb.Dataset(X, y), num_boost_round=100); p = b.predict(X_test)
```

### url（6 万行 × 256 次元のハッシュ特徴、テスト 4 万行）

| 手段 | 解けた量（AUC） | 学習 | 1 行推論 | まとめて推論（1 行あたり） |
|---|---|---|---|---|
| TS 自前 LR | 0.9824 | 1.01 秒 | 1.1µs | 1.1µs |
| TS 自前 MLP [8] | 0.9849 | 3.25 秒 | 3.4µs | 3.4µs |
| Rust 自前 LR | 0.9824 | 0.29 秒 | 306ns | 306ns |
| Rust 自前 MLP [8] | 0.9842 | 0.94 秒 | 1.3µs | 1.3µs |
| `perpetual`（budget 0.5） | 0.9951 | 304 秒 | 142.7µs | 24.9µs |
| scikit-learn LR | 0.9842 | 0.60 秒 | 70.9µs | 215ns |
| scikit-learn MLP (8,) | 0.9841 | 1.34 秒 | 63.0µs | 371ns |
| LightGBM（100 本） | 0.9944 | 2.01 秒 | 22.7µs | 1.7µs |
| scikit-learn HistGB | 0.9943 | 3.29 秒 | 871.9µs | 4.0µs |

### cache（18 万行 × 11 次元で学習、42 万行で評価）

| 手段 | 解けた量（AUC） | 学習 | 1 行推論 | まとめて推論（1 行あたり） |
|---|---|---|---|---|
| TS 自前 LR | 0.9321 | 0.17 秒 | 86ns | 86ns |
| TS 自前 MLP [16] | 0.9341 | 0.68 秒 | 366ns | 366ns |
| Rust 自前 LR | 0.9324 | 0.10 秒 | 30ns | 30ns |
| Rust 自前 MLP [16] | 0.9338 | 0.27 秒 | 110ns | 110ns |
| `perpetual`（budget 0.5） | 0.9316 | 1.38 秒 | 4.9µs | 1.1µs |
| `perpetual`（budget 1.0） | 0.9327 | 2.48 秒 | 13.5µs | 2.7µs |
| scikit-learn LR | 0.9311 | 0.45 秒 | 90.3µs | 26ns |
| scikit-learn MLP (16,) | 0.9332 | 1.48 秒 | 69.1µs | 115ns |
| LightGBM（100 本） | 0.9332 | 0.37 秒 | 22.7µs | 679ns |
| scikit-learn HistGB | 0.9328 | 0.82 秒 | 922.5µs | 1.1µs |

## 読み取れること

| 種類 | Rust | Python | 判定 |
|---|---|---|---|
| LR・小さい MLP の学習 | 自前 0.26 s / 0.79 s | sklearn 0.60 s / 1.34 s | Rust が同等以上。AUC も同じ水準 |
| LR・小さい MLP の 1 行推論 | 91 ns〜1.0 µs | 63〜90 µs | Rust が 60〜760 倍速い。Python は 1 回の呼び出しの手間が大きい |
| まとめて推論 | 91 ns〜1.0 µs/行 | 26〜371 ns/行 | ほぼ同じ。Python も中身は BLAS |
| GBDT の学習 | perpetual 1.4〜304 s | LightGBM 0.4〜2.0 s | LightGBM（C++）が 4〜150 倍速い。256 次元では perpetual が遅すぎる |
| GBDT の精度 | AUC 0.9951 / 0.9327 | AUC 0.9944 / 0.9332 | ほぼ同じ |

この 2 つのデータでは、GBDT の AUC の上乗せは小さかった（url で 0.984 → 0.995、cache ではほぼ 0）。

## 同じ問題を解く：キャッシュのヒット率（容量 0.5%）

AUC ではなく、キャッシュのヒット率そのもので比べた。
各手段のモデルを「LRU に入れるかどうか」の判断に差し込み、ML を使わない定番のキャッシュ（`moka`、`quick_cache`）とも比べる。

特徴量はキャッシュの中身に依存しない（参照回数・前回からの間隔・種別・サイズ）。
そこで各手段は全 60 万リクエストのスコアを `data/scores/` に書き、Rust の 1 つのシミュレータが同じ条件で評価する。
LRU・doorkeeper・oracle の値は、TS の実験（`ts/src/cache-admission.ts`）の値と一致した。

```bash
cd ../ && DUMP_DIR=bench/data node ts/src/cache-admission.ts && node bench/ts/bench.ts --scores
cd bench/python && .venv/bin/python bench.py --scores
cd ../rust && RUSTC_BOOTSTRAP=1 cargo run --release -- scores && RUSTC_BOOTSTRAP=1 cargo run --release -- sim
```

（この節の出力は、TTL の行を足す前のもの。最新の全体は次の節にある）

```
cache 282 MB, eval on last 70% of 600000 requests
policy                                       OHR     BHR
LRU (admit all)                           14.66%   8.08%
LRU + doorkeeper (2nd hit)                20.23%  12.86%
moka 0.12 (TinyLFU)                       35.75%  11.69%
quick_cache 0.7 (S3-FIFO)                 22.52%  15.07%
LRU + py-lightgbm p>0.5                   24.64%  16.93%
LRU + py-lightgbm p>0.2 (tuned)           25.37%  17.31%
LRU + py-sklearn-histgb p>0.5             24.91%  16.20%
LRU + py-sklearn-histgb p>0.2 (tuned)     25.39%  17.33%
LRU + py-sklearn-logreg p>0.5             25.39%  16.88%
LRU + py-sklearn-logreg p>0.1 (tuned)     24.64%  16.32%
LRU + py-sklearn-mlp16 p>0.5              26.61%  17.20%
LRU + py-sklearn-mlp16 p>0.2 (tuned)      26.01%  17.21%
LRU + rust-own-logreg p>0.5               23.85%  14.75%
LRU + rust-own-logreg p>0.1 (tuned)       23.51%  15.25%
LRU + rust-own-mlp16 p>0.5                25.41%  17.52%
LRU + rust-own-mlp16 p>0.2 (tuned)        25.54%  17.25%
LRU + rust-perpetual-b0.5 p>0.5           24.74%  16.89%
LRU + rust-perpetual-b0.5 p>0.1 (tuned)   23.75%  15.86%
LRU + rust-perpetual-b1 p>0.5             24.43%  16.51%
LRU + rust-perpetual-b1 p>0.2 (tuned)     25.80%  17.42%
LRU + ts-own-logreg p>0.5                 23.43%  14.78%
LRU + ts-own-logreg p>0.2 (tuned)         25.92%  17.03%
LRU + ts-own-mlp16 p>0.5                  25.03%  17.39%
LRU + ts-own-mlp16 p>0.2 (tuned)          24.97%  17.30%
LRU + oracle (future)                     27.13%  19.14%
```

OHR はリクエスト数で数えたヒット率（応答の速さに効く）、BHR はバイト数で数えたヒット率（裏側への転送量に効く）。
`p>0.5` は閾値を 0.5 に固定した版、`(tuned)` は学習区間（先頭 30%）の中で OHR が最大になる閾値を選び直した版。
oracle は「未来を見て、H リクエスト以内にまた来るものだけを LRU に入れる」で、admission だけの上限。追い出し方まで含めた上限ではない。
`moka` は実行ごとに少し揺れた（3 回で OHR 35.75〜36.11%）。ほかの行は毎回同じ値になる。

### 読み取れること

| 改善したいもの | 一番良かったもの | 値 | ML の admission との差 |
|---|---|---|---|
| OHR（リクエスト数） | `moka`（TinyLFU、ML なし） | 約 36% | ML の admission の最高（26.6%）より 9 ポイント高い |
| BHR（バイト数） | LRU + MLP の admission | 17.2〜17.5% | `quick_cache`（15.1%）より 2 ポイント高い。`moka` は 11.7% |

- ML の admission は、どの言語・ライブラリで作っても OHR 23.4〜26.6%、BHR 14.8〜17.5% に収まった。手段による差は、閾値とキャリブレーション（確率の出方）による差と同じくらいの大きさしかない。
- AUC がほぼ同じでも（0.931〜0.934）、閾値 0.5 で入れる量が変わり、ヒット率は 2〜3 ポイント動く。学習区間で閾値を選び直しても、一貫しては良くならなかった（区間によって分布が違うため）。
- `moka` が admission の上限（27.1%）を超えたのは、追い出し方も変えているため。頻度で追い出す相手を選び、小さくて人気のあるものを多く残すので、OHR は高いが BHR は低い。
- 結論：リクエスト数を減らしたいなら、まず `moka` を使う（ML は要らない）。転送量を減らしたいなら、LRU + 小さい MLP の admission が一番良く、作る言語はどれでもよい。

## 微分可能プログラミング：ヒット率そのものを最適化する（JAX と Rust の std::autodiff）

ここまでの ML は「H リクエスト以内にまた来るか」という近似のラベルを学習していた。
微分可能プログラミングを使うと、最適化したい量（容量の制約の下でのヒット）を直接、損失として書ける。

キャッシュを TTL（保持期間）で書き直すと、全体が微分できる式になる。
各リクエスト s に TTL_s = exp(f_θ(x_s)) を割り当て（f_θ は 11-16-1 の MLP）、次のアクセスまでの間隔を g_s とする。

| 量 | 式 | 微分できる形 |
|---|---|---|
| ヒット | g_s ≤ TTL_s なら 1 | sigmoid((log TTL_s − log g_s) / 0.25) |
| 平均占有量 | Σ size_s × min(g_s, TTL_s) / 行数 | そのまま |
| 損失 | − ヒット + 10 × max(0, 占有量 / 容量 − 1)² | そのまま |

ヒットは OHR 用（リクエスト数で数える）と BHR 用（バイトで重みづけ）の 2 通りを学習した。
学習は先頭 30% だけを使い、学習区間の外に出る間隔は「来なかった」として扱う（未来を見ない）。
学習の後、バイアスを二分探索でずらし、学習区間の平均占有量をちょうど容量に合わせた。
評価は、容量を厳密に守る TTL キャッシュ（期限の早いものから追い出す）で行った。

```bash
python/.venv/bin/python jax/ttl.py bhr && python/.venv/bin/python jax/ttl.py ohr
rustup toolchain install nightly && rustup +nightly component add enzyme
cd autodiff && RUSTFLAGS="-Zautodiff=Enable" cargo +nightly run --release -- bhr   # ohr も同様
cd ../rust && RUSTC_BOOTSTRAP=1 cargo run --release -- sim
```

JAX（jax 0.12.1）と Rust（nightly 1.102 の `std::autodiff`）で同じ損失を書き、同じ決定的な初期値から 500 ステップ学習した。

| | JAX | Rust `std::autodiff` |
|---|---|---|
| 損失を書く部分 | `terms()` と `loss()` で約 15 行（配列の演算） | `log_ttl()`・`terms()`・`loss()` で約 35 行（普通の for ループ） |
| 微分の書き方 | `jax.value_and_grad(loss)` | `#[autodiff_reverse(d_loss, Duplicated, Const, ..., Active)]` |
| ステップ 1 の損失 | −0.1367 | −0.1367 |
| 学習後（補正後）の soft BHR / OHR | 0.2297 / 0.4114 | 0.2299 / 0.4112 |
| 学習時間（500 ステップ × 18 万行） | 14.6 秒（jit、XLA） | 101〜107 秒（1 スレッド） |
| セットアップ | `uv pip install jax` | nightly + enzyme コンポーネント、`lto = "fat"`、`RUSTFLAGS="-Zautodiff=Enable"` |

キャッシュのシミュレーション（上の表と同じ条件）:

```
cache 282 MB, eval on last 70% of 600000 requests
policy                                       OHR     BHR
LRU (admit all)                           14.66%   8.08%
LRU + doorkeeper (2nd hit)                20.23%  12.86%
moka 0.12 (TinyLFU)                       35.83%  12.56%
quick_cache 0.7 (S3-FIFO)                 22.52%  15.07%
LRU + py-lightgbm p>0.5                   24.64%  16.93%
LRU + py-lightgbm p>0.2 (tuned)           25.37%  17.31%
LRU + py-sklearn-histgb p>0.5             24.91%  16.20%
LRU + py-sklearn-histgb p>0.2 (tuned)     25.39%  17.33%
LRU + py-sklearn-logreg p>0.5             25.39%  16.88%
LRU + py-sklearn-logreg p>0.1 (tuned)     24.64%  16.32%
LRU + py-sklearn-mlp16 p>0.5              26.61%  17.20%
LRU + py-sklearn-mlp16 p>0.2 (tuned)      26.01%  17.21%
LRU + rust-own-logreg p>0.5               23.85%  14.75%
LRU + rust-own-logreg p>0.1 (tuned)       23.51%  15.25%
LRU + rust-own-mlp16 p>0.5                25.41%  17.52%
LRU + rust-own-mlp16 p>0.2 (tuned)        25.54%  17.25%
LRU + rust-perpetual-b0.5 p>0.5           24.74%  16.89%
LRU + rust-perpetual-b0.5 p>0.1 (tuned)   23.75%  15.86%
LRU + rust-perpetual-b1 p>0.5             24.43%  16.51%
LRU + rust-perpetual-b1 p>0.2 (tuned)     25.80%  17.42%
LRU + ts-own-logreg p>0.5                 23.43%  14.78%
LRU + ts-own-logreg p>0.2 (tuned)         25.92%  17.03%
LRU + ts-own-mlp16 p>0.5                  25.03%  17.39%
LRU + ts-own-mlp16 p>0.2 (tuned)          24.97%  17.30%
LRU + oracle (future)                     27.13%  19.14%
TTL cache, constant TTL=1546              14.22%   7.72%
TTL cache, learned jax-bhr                24.14%  16.84%
TTL cache, learned jax-ohr                40.00%   8.76%
TTL cache, learned rust-autodiff-bhr      24.16%  16.89%
TTL cache, learned rust-autodiff-ohr      39.99%   8.74%
```

### 読み取れること

- OHR を直接最適化した TTL は 40.0% で、`moka`（約 36%）も、これまでのどの ML の admission（最高 26.6%）も、admission の上限（27.1%）も上回った。admission だけでなく「どれだけ長く残すか」まで学習しているため。
- BHR を直接最適化した TTL は 16.8〜16.9% で、近似のラベルで学習した MLP の admission（17.2〜17.5%）と同じ水準にとどまった。BHR では、目的を直接書く利点は出なかった。
- JAX と Rust は、同じ損失・同じ初期値から、ほぼ同じ結果に収束した（実キャッシュの OHR 40.00% と 39.99%）。
- JAX のほうが 7 倍速かった。Rust 版は 1 スレッドのスカラーのループで、並列化もベクトル化もしていない。
- 定数の TTL（1546）は LRU とほぼ同じ（14.2%）。差はすべて、特徴量から TTL を決めたことによる。
- `moka` は実行ごとに BHR も揺れた（11.6〜12.6%）。

## 測っていないこと

- PyTorch。download.pytorch.org がこの環境のネットワーク設定で塞がれていて、PyPI の wheel は CUDA の依存で大きすぎた。
- Rust から LightGBM の C ライブラリを呼ぶ場合（`lightgbm3` など）。中身は Python 版と同じ C++ なので学習速度は同等と予想するが、未計測。
- GPU。
- 実行のばらつき。各 1 回の計測。
