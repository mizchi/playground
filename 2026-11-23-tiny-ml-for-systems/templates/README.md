# templates：小さいモデルを自前で作る雛形

キャッシュやフィルタの「判断」を、フレームワーク無しの小さいモデルで作るための雛形。
TS・Rust・Python・Julia の 4 言語で同じ構造にしてあり、どの言語で学習した重みも、TS と Rust の製品コードにそのまま読み込める。

使うときは、言語のディレクトリを自分のプロジェクトにコピーして書き換える（このディレクトリを参照しない）。

## 置き場所

```
templates/
  ts/              TS。製品の中での推論の第一候補。学習もできる
    tinyml.ts        MLP・逆伝搬・Adam・AUC・JSON の読み書き（依存ゼロ）
    test.ts          勾配チェック・XOR・JSON の往復
    train_cache.ts   学習 → 評価 → 重み（out/*.json）とスコアの書き出し
    infer.ts         どの言語の重みでも読み込んで、1 行ずつ推論する（製品での使い方）
  rust/            Rust。製品の中での推論の第一候補。学習もできる
    src/lib.rs       MLP・逆伝搬・Adam・int8 量子化・AUC・JSON の読み書き・テスト（依存ゼロ）
    examples/train_cache.rs, examples/infer.rs   TS 版と同じ役割
  python/          Python。学習と、目的関数を試す場所。速度を比べる相手にもする
    tinyml.py, test_tinyml.py, train_cache.py   numpy だけの自前実装
    infer.py         どの言語の重みでも読み込んで推論する（1 行ずつと、まとめて）
    ttl_jax.py       微分可能プログラミング（JAX）
  julia/           Julia。学習と、目的関数を試す場所
    TinyML.jl, test.jl, train_cache.jl   標準ライブラリだけの自前実装
    infer.jl         どの言語の重みでも読み込んで推論する（1 行ずつと、まとめて）
    diff/ttl_enzyme.jl   微分可能プログラミング（Enzyme.jl）
  rust-autodiff/   微分可能プログラミング（Rust std::autodiff、nightly）
  compare.py       4 言語を同じ条件で動かし、速度を表にする（標準ライブラリだけ）
  RESULT.md        compare.py の最新の結果
```

## どれを使うか

| やりたいこと | 使う雛形 |
|---|---|
| 製品（JS）の中で判断する | `ts/tinyml.ts` の推論（`MLP.fromJSON` → `predict`） |
| 製品（Rust）の中で判断する | `rust/src/lib.rs` の推論（`Mlp::from_json` → `predict`）。int8 にするなら `QuantMlp` |
| ラベル（「また来るか」など）から学習する | どの言語の `train_cache.*` でもよい。製品と同じ言語にすると、特徴量の作り方を共有できる |
| ヒット率などを直接最適化する（目的関数を試す） | `python/ttl_jax.py`（速い）か `julia/diff/ttl_enzyme.jl`（ループで書ける）。決まったら `rust-autodiff/` に移せる |

## 共通の流れ

どの言語でも、次の 5 段階で使う。

| 段階 | やること | 雛形の場所 |
|---|---|---|
| 1. 特徴量 | 判断の時点で分かる値を、0〜1 程度にそろえた数値の列にする（log、one-hot、ハッシュ） | 例は `../ts/src/cache-admission.ts` の `features()` と `../ts/src/bloom.ts` の `hashedNgrams()` |
| 2. ラベルか目的 | ログから後で分かる正解（「H 以内にまた来たか」）を付ける。目的を直接書くなら、微分できる形の損失にする | `train_cache.*`、`ttl_*` |
| 3. 学習と確認 | 逆伝搬を書き換えたら、勾配チェックを回す。評価は時系列で分ける（過去で学習、未来で評価） | `test.*`、`train_cache.*` |
| 4. 書き出し | 重みを共通の JSON にする | `to_json` / `toJSON` |
| 5. 製品で使う | JSON を読み、`predict(x) > τ` を判断の 1 行にする。TTL モデルは `exp(predictLogit(x))` | `infer.*` |

重みの JSON は `{"sizes":[11,16,1],"W":[[...],[...]],"b":[[...],[...]]}` で、`W[l]` は（出力, 入力）を row-major で平らにしたもの。

## 書き換える場所

| 変えたいもの | 書き換える場所 |
|---|---|
| 特徴量の次元 | `sizes` の先頭と、データの読み込み（例では 11） |
| モデルの大きさ | `sizes`。`[d, 1]` ならロジスティック回帰、`[d, 16, 1]` なら隠れ層 16 |
| ラベル | `train_cache.*` が読む `y`。例は `../bench/data/cache_y.f32` |
| 判断の閾値 | `infer.*` の `> 0.5` |
| 最適化したい量 | `ttl_*` の `terms()`（ヒットの重み `w` をバイトにするか件数にするか、容量の制約） |
| 出力の形 | 回帰なら sigmoid を外して二乗誤差、多クラスなら softmax（出力の誤差はどちらも `予測 − 正解` の形） |

## 動かし方

データは `../bench/data`（作り方は `../bench/README.md`）。

```bash
cd ts && node test.ts && node train_cache.ts
node infer.ts ../python/out/mlp16.json ../../bench/data/scores/py-template-mlp16.f32
node infer.ts --ttl ../python/out/ttl-jax-ohr.json ../../bench/data/ttl/py-template-jax-ohr.f32
```

```bash
cd rust && cargo test --release
cargo run --release --example train_cache
cargo run --release --example infer -- ../julia/out/mlp16.json ../../bench/data/scores/jl-template-mlp16.f32
```

```bash
cd python && uv venv .venv && uv pip install --python .venv/bin/python numpy jax
.venv/bin/python test_tinyml.py && .venv/bin/python train_cache.py && .venv/bin/python ttl_jax.py ohr
```

```bash
cd julia && julia test.jl && julia train_cache.jl
julia --project=diff -e 'using Pkg; Pkg.instantiate()' && julia --project=diff diff/ttl_enzyme.jl ohr
```

```bash
cd rust-autodiff   # rustup toolchain install nightly && rustup +nightly component add enzyme
RUSTFLAGS="-Zautodiff=Enable" cargo +nightly run --release -- ohr
```

ヒット率は `cd ../bench/rust && RUSTC_BOOTSTRAP=1 cargo run --release -- sim` で、ほかの手段と並べて測れる。

## 速度を比べる

```bash
python3 compare.py                  # 学習・推論・TTL（JAX と Enzyme.jl）
python3 compare.py --with-autodiff  # Rust std::autodiff の TTL も（nightly が必要）
python3 compare.py --out RESULT.md  # 表を書き出す
```

各スクリプトは `bench lang=... task=... key=value` の行を出し、`compare.py` はその行だけを読んで表にする。
道具が入っていない言語は「未実行」になる。コマンドは環境変数 `NODE`・`CARGO`・`PYTHON`・`JULIA` で差し替えられる。
雛形を書き換えたら、もう一度回して `RESULT.md` を更新する。

最新の結果は [RESULT.md](RESULT.md)。要点は次のとおり（4 コアの Linux、各 1 回の実行）。

| | TS | Rust | Python | Julia |
|---|---|---|---|---|
| MLP の学習（18 万行 × 4 エポック） | 0.67 秒 | 0.26 秒 | 0.55 秒 | 0.22 秒 |
| 1 行ずつの推論 | 474 ns | 105 ns | 3132 ns | 453 ns |
| まとめての推論（1 行あたり） | 474 ns | 105 ns | 42 ns | 162 ns |
| TTL の学習（微分可能プログラミング） | — | 83.6 秒（std::autodiff） | 11.7 秒（JAX） | 15.9 秒（Enzyme.jl） |

製品の中の判断（1 行ずつ）は Rust が最速で、TS と Julia が同じくらい、Python は 30 倍遅い。
まとめて推論するなら Python（numpy）が最速になる。学習はどの言語でも 1 秒かからない。
目的関数を試す微分可能プログラミングは、JAX と Enzyme.jl が Rust の 5〜7 倍速い。

## 確かめたこと

TS は Node 22.23、Rust は 1.97（`rust-autodiff` は nightly 1.102）、Python は 3.13（numpy 2.5.5、jax 0.12.1）、Julia は 1.13.4（Enzyme.jl 0.13.221）で実行した。

### 学習

| | TS | Rust | Python | Julia |
|---|---|---|---|---|
| 勾配チェック（最大相対誤差） | 3.07e-9 | 合格（`cargo test`） | 1.11e-8 | 1.74e-8 |
| XOR | 合格 | 合格 | 合格 | 合格 |
| キャッシュの AUC（LR / MLP） | 0.9321 / 0.9341 | 0.9324 / 0.9338 | 0.9323 / 0.9314 | 0.9325 / 0.9329 |
| キャッシュの学習時間（LR / MLP） | 0.28 / 0.89 秒 | 0.08 / 0.28 秒 | 0.34 / 0.87 秒 | 2.69（初回のコンパイルを含む）/ 0.22 秒 |
| ヒット率（MLP、p>0.5、OHR / BHR） | 25.03% / 17.39% | 25.41% / 17.52% | 25.54% / 17.10% | 24.65% / 17.00% |

### 重みの受け渡し（学習した言語 → 推論する言語）

MLP（11-16-1）の重みを JSON で渡し、先頭 10 万行で、学習側の予測との最大の差を測った。

| 学習した言語 | TS で推論 | Rust で推論 |
|---|---|---|
| TS | 0（599 ns/行） | 2.4e-7（115 ns/行） |
| Rust | 2.1e-7（537 ns/行） | 0（204 ns/行） |
| Python | 0（507 ns/行） | 1.8e-7（194 ns/行） |
| Julia | 0（531 ns/行） | 2.4e-7（116 ns/行） |

Rust で推論したときの差（約 2e-7）は、Rust が f32 で計算し、ほかが f64 で計算しているため。判断（入れる割合）は学習側と同じになった。
ns/行は 1 行ずつ推論したときの値で、TS 版は特徴量を 1 行ぶん複写する時間を含む。

### 微分可能プログラミング（TTL、OHR を直接最適化）

| | Python（JAX） | Julia（Enzyme.jl） | Rust（std::autodiff） |
|---|---|---|---|
| 補正後の soft OHR | 0.4114 | 0.4112 | 0.4112 |
| 実キャッシュの OHR / BHR | 40.00% / 8.76% | 39.99% / 8.74% | 39.99% / 8.74% |
| 学習時間（500 ステップ × 18 万行） | 14.0 秒（jit） | 22〜24 秒（1 スレッド、初回のコンパイルを含む） | 101〜111 秒（1 スレッド） |
| 書き出した重みを TS で推論したときの TTL の差 | 相対 0 | 相対 0 | 相対 0 |

3 つとも、同じ損失・同じ初期値から、ステップ 1 の損失 −0.1367 で一致した。

Enzyme.jl は、`Pkg.instantiate()` に約 5 分かかった（LLVM・GPUCompiler・Enzyme などのプリコンパイル）。
同じ Enzyme を使う Rust の `std::autodiff` より 4 倍以上速かった。
理由は確かめていないが、Julia 版は `@inbounds` で添字の範囲検査を外していて、Rust 版は外していないことが一因と考えられる。
