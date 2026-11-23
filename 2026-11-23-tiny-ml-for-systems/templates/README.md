# templates：小さいモデルを自前で作る雛形（Python・Julia）

TS（`../ts/src/nn.ts`）・Rust（`../rust/src/lib.rs`）と同じパターンを、Python と Julia でも書けるようにした雛形。
どの言語でも、次の 6 つを同じ形で持つ。

| 部品 | 中身 |
|---|---|
| モデル | 隠れ層 ReLU、出力 1 ユニット + sigmoid、交差エントロピー。`sizes=[d, 1]` ならロジスティック回帰 |
| 学習 | 手で書いた逆伝搬（δ = p − y）と Adam、ミニバッチ |
| 確かめ方 | 数値微分による勾配チェックと XOR |
| 評価 | 時系列で分ける（先頭 30% で学習）、AUC |
| 書き出し | 重みを JSON（TS の `MLP.toJSON()` と同じ形）と、全行のスコア（`bench/rust` の `sim` がヒット率を測る） |
| 微分可能プログラミング | TTL を「容量の制約の下でのヒット」に対して直接最適化する（Python は JAX、Julia は Enzyme.jl） |

重みの JSON は `{"sizes":[11,16,1],"W":[[...]],"b":[[...]]}` で、`W[l]` は（出力, 入力）を row-major で平らにしたもの。
どの言語で学習しても、製品の中では TS か Rust の自前推論に読ませる。

## ファイル

| | Python | Julia |
|---|---|---|
| モデル・逆伝搬・Adam・勾配チェック・AUC・JSON | `python/tinyml.py`（依存は numpy） | `julia/TinyML.jl`（依存は標準ライブラリだけ） |
| テスト | `python/test_tinyml.py` | `julia/test.jl` |
| キャッシュ admission の例 | `python/train_cache.py` | `julia/train_cache.jl` |
| 微分可能プログラミング（TTL） | `python/ttl_jax.py`（jax） | `julia/diff/ttl_enzyme.jl`（Enzyme.jl。Rust の `std::autodiff` と同じ Enzyme） |

データは `../bench/data`（作り方は `../bench/README.md`）。

## 動かし方

```bash
cd python
uv venv .venv && uv pip install --python .venv/bin/python numpy jax
.venv/bin/python test_tinyml.py
.venv/bin/python train_cache.py
.venv/bin/python ttl_jax.py ohr
```

```bash
cd julia
julia test.jl
julia train_cache.jl
julia --project=diff -e 'using Pkg; Pkg.instantiate()'
julia --project=diff diff/ttl_enzyme.jl ohr
```

ヒット率は `cd ../bench/rust && RUSTC_BOOTSTRAP=1 cargo run --release -- sim` で、ほかの手段と並べて測れる。

## 確かめたこと

| | Python | Julia |
|---|---|---|
| 勾配チェック | 最大相対誤差 1.11e-8 | 未実行 |
| XOR | 0.001 1.000 1.000 0.000 | 未実行 |
| キャッシュ（AUC） | LR 0.9323、MLP 0.9314 | 未実行 |
| キャッシュ（ヒット率、p>0.5） | LR：OHR 23.80%・BHR 15.35%、MLP：OHR 25.54%・BHR 17.10% | 未実行 |
| TTL（OHR を直接最適化） | OHR 40.00%・BHR 8.76%（`bench/jax` と同じ） | 未実行 |

Julia の雛形は、この環境で Julia の本体を実行できなかったので、まだ動かしていない。
Python の雛形の値は、TS・Rust の自前実装（MLP の BHR 17.39%・17.52%）と同じ水準になった。
