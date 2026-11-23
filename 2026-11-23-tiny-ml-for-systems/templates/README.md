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

Python 3.13（numpy 2.5.5、jax 0.12.1）と Julia 1.13.4（Enzyme.jl 0.13.221）で実行した。

| | Python | Julia |
|---|---|---|
| 勾配チェック | 最大相対誤差 1.11e-8 | 最大相対誤差 1.74e-8 |
| XOR | 0.001 1.000 1.000 0.000 | 0.001 1.0 1.0 0.0 |
| キャッシュの AUC | LR 0.9323、MLP 0.9314 | LR 0.9325、MLP 0.9329 |
| キャッシュの学習時間 | LR 0.34 秒、MLP 0.87 秒 | LR 2.69 秒（初回のコンパイルを含む）、MLP 0.22 秒 |
| ヒット率（LR、p>0.5） | OHR 23.80%・BHR 15.35% | OHR 23.35%・BHR 15.08% |
| ヒット率（MLP、p>0.5） | OHR 25.54%・BHR 17.10% | OHR 24.65%・BHR 17.00% |
| TTL（OHR を直接最適化）の実キャッシュ | OHR 40.00%・BHR 8.76% | OHR 39.99%・BHR 8.74% |
| TTL の学習（500 ステップ × 18 万行） | 14.0 秒（JAX、jit） | 22〜24 秒（Enzyme.jl、1 スレッド、初回のコンパイルを含む） |

どの値も、TS・Rust の自前実装（MLP の BHR 17.39%・17.52%）や、JAX・Rust `std::autodiff` の TTL（OHR 40.00%・39.99%）と同じ水準になった。
TTL の損失は、JAX・Rust・Julia のどれでもステップ 1 で −0.1367 と一致し、補正後の soft OHR は 0.4112〜0.4114 になった。

Enzyme.jl のセットアップには、`Pkg.instantiate()` で約 5 分かかった（LLVM・GPUCompiler・Enzyme などのプリコンパイル）。
同じ Enzyme を使う Rust の `std::autodiff`（101〜107 秒）より 4 倍以上速かった。
理由は確かめていないが、Julia 版は `@inbounds` で添字の範囲検査を外していて、Rust 版は外していないことが一因と考えられる。
