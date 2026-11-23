# tiny-ml-for-systems

フレームワーク (PyTorch / TF / onnxruntime) を持ち出さず、**数百行の Rust / TS で書いた小さな ML** を
キャッシュ・フィルタ・予測器などの「システムの判断部品」として使う練習場。

- いつ使うべきか / 使うべきでないか
- 実際に効果が出た事例
- 誤差逆伝搬 MLP をゼロから書く最小コード (TS / Rust、依存ゼロ)
- 3 つの実験: **learned Bloom filter**、**キャッシュ admission**、**パーセプトロン分岐予測**

```
ts/    nn.ts (MLP+backprop+Adam, ~200行)  bloom.ts  urls.ts
       gradcheck.ts  learned-bloom.ts  cache-admission.ts
rust/  src/lib.rs (Mlp, int8 QuantMlp, Bloom, n-gram hashing)  src/urls.rs
       examples/learned_bloom.rs  examples/branch_predictor.rs
explainer/  mizchi/explainer スキルで書いた速習資料 (図つき、出力は再実行して照合)
bench/      Rust (自前・perpetual) と Python (scikit-learn・LightGBM) の比較
GUIDE.md    いつ・どこで・どの入力量で・どの出力に、何を使うかの整理
```

図つきの解説は [explainer/README.md](explainer/README.md)。「入れてよいかをどう判定するか」に絞って、このページの実験を読み直している。

## 動かし方

```bash
cd ts
node src/gradcheck.ts        # 逆伝搬の数値微分チェック + XOR
node src/learned-bloom.ts    # ~30s
node src/cache-admission.ts  # ~6s
```

```bash
cd rust
cargo test --release
cargo run --release --example learned_bloom
cargo run --release --example branch_predictor
```

Node は 22.18 以上 (型ストリップで `.ts` を直接実行)。依存パッケージは無い。

---

## 1. いつ使うか: 判断基準

システムの中の ML がうまくいく典型は、
**「手書きのヒューリスティック (if 文と閾値) で判断しているところを、学習した関数に置き換える」** 形。
次の条件がそろうほど向いている。

| 条件 | 理由 | 例 |
|---|---|---|
| **ラベルが自動で取れる** | 一番大事。人手のアノテーション無しでログから正解が作れる | キャッシュ: 「後でまた来たか」、分岐: 「実際に taken だったか」、I/O: 「遅かったか」 |
| **間違えても正しさが壊れない** (フォールバックがある) | ML は必ず外れる。外れたときに遅くなるだけ / 古典的構造が拾う設計にする | learned Bloom の backup filter、キャッシュミスは遅いだけ、分岐予測ミスはパイプラインのやり直しだけ |
| **既存ヒューリスティックの係数を手で調整している** | 「閾値 0.7 と重み 3 はなぜ?」に答えられないなら、それは学習すべきパラメータ | スパムスコアの重み、admission の閾値、優先度スコア |
| **入力の特徴が複数あって、組み合わせが効く** | 1 特徴なら閾値 1 個で十分。複数の弱い手がかりを足し合わせるのが ML の得意分野 | サイズ × 種別 × 参照間隔 × クライアント |
| **推論コスト ≪ 判断で節約できるコスト** | 1µs の推論で 1ms のディスク I/O を避けられるなら十分元が取れる | フラッシュキャッシュの admission、CDN、コンパイラ最適化 |
| **データの分布が (ほぼ) 定常、または再学習できる** | 学習時と運用時で分布が違うと壊れる | — |

### 使うべきでないとき

- **攻撃者が入力を選べる** (後述の実験で FPR が 1% → 84% に跳ね上がる)。セキュリティ境界の判断を学習モデル単体に任せない。
- **フォールバックが無く、間違いが正しさのバグになる**。
- **単純なヒューリスティックでほぼ足りている**。キャッシュ実験で見るように「2 回目で admit」だけで改善の大半が取れることが多い。必ずそれをベースラインにする。
- **ナノ秒単位のホットパス**で、推論が節約分より高くつく (Bloom filter の 70ns に対して特徴量抽出 + 推論で 1.5µs かかる)。
- **ラベルが取れない / 評価できない**。評価できない ML は入れてはいけない。

### モデルの段階 (小さいほうから試す)

1. **定数閾値 / 手書きルール**: これがベースライン
2. **ロジスティック回帰** (= 隠れ層ゼロの MLP): 30 行。重みが読めるので説明もできる。**大抵ここで十分**
3. **小さい MLP** (隠れ層 1 つ、8〜32 ユニット): 特徴の相互作用 (XOR 的なもの) が必要なとき
4. **勾配ブースティング木 (GBDT)**: 表形式データでは MLP より強いことが多い。LRB や Baleen は GBDT を使っている。ただし手書きは MLP より重い
5. それ以上 (大きい NN、RNN) は、もうフレームワークを使う領域

実験でもロジスティック回帰が最もコスパがよく、大きい MLP は精度がわずかに上がる代わりにサイズで負けた。

---

## 2. 実際に効果が出た事例

| 事例 | 何を学習したか | モデル | 報告されている効果 |
|---|---|---|---|
| **パーセプトロン分岐予測** (Jiménez & Lin, HPCA 2001) | 分岐の taken/not-taken をグローバル分岐履歴から予測 | 1 層パーセプトロン、整数重み、オンライン学習 | AMD Zen 系 CPU などに "neural net prediction" として実装されている。**最も普及した組み込み ML** |
| **Learned Index / Learned Bloom Filter** (Kraska et al., SIGMOD 2018) | キーの位置 / キー集合の所属 | 小さい NN、線形モデルの階層 | B-Tree や Bloom filter よりメモリが大幅に小さくなるケースを示した。ただし後続研究で、よく調整された古典構造との差は条件次第とされている |
| **LRB: Learning Relaxed Belady** (Song et al., NSDI 2020) | CDN キャッシュで「どのオブジェクトがしばらく来ないか」 | GBDT | 本番 CDN トレースで、LRU 中心の典型的な CDN 構成に比べ WAN トラフィックを 4〜25% 削減。Apache Traffic Server 上の実装で、スループットは非改変版と同等 |
| **Baleen** (Wong et al., FAST 2024, Meta) | フラッシュキャッシュに入れるべきか (admission) + prefetch | GBDT | Meta の 7 クラスタのトレースで、ピーク時のディスクヘッド時間 (≒必要な HDD 台数) を最新手法比 12% 削減 |
| **LinnOS** (Hao et al., OSDI 2020) | 各 I/O が SSD で「速い / 遅い」か | 3 層の軽量 NN を**カーネル内**で実行。重みは量子化 | 遅いと予測した I/O を別レプリカに回してテールレイテンシを改善。回帰ではなく二値分類に落としたのが勝因 |
| **MLGO** (Google, LLVM) | インライン化するかどうか、レジスタ割り当ての追い出し | 小さい NN (強化学習で訓練し、AOT コンパイルして LLVM に埋め込み) | `-Oz` 比でバイナリサイズを 3〜7% 削減。Fuchsia で 6.3% |
| **ベイジアンスパムフィルタ** (Paul Graham "A Plan for Spam", 2002) | メールがスパムか | ナイーブベイズ (単語ごとの確率表) | 手書きルールのスパムフィルタを置き換え、今もメールクライアントに広く入っている |

共通点:

- **出力は二値か、スコアによる順位付け** (回帰より学習しやすい: LinnOS の教訓)
- **正解は後から自動で分かる** (キャッシュ / 分岐 / I/O / コンパイル結果のサイズ)
- **外れても既存の仕組みが正しさを保つ** (キャッシュミス、分岐予測ミス、backup filter)
- **モデルは小さく、推論は自前コードか AOT 生成コード**。ランタイムにフレームワークを持ち込んでいない

---

## 3. 誤差逆伝搬を最小限で書く

`ts/src/nn.ts` と `rust/src/lib.rs` の `Mlp` が実装。数式は 3 行で済む。

出力を sigmoid、損失を binary cross entropy にすると、出力層の誤差は単純な形になる:

```
δ_L      = p − y                      (予測確率 − 正解)
∂L/∂W_l  = δ_l ⊗ a_l                  (誤差と、その層への入力の外積)
δ_{l−1}  = (W_lᵀ δ_l) ⊙ [a_{l−1} > 0]  (ReLU の微分は 0 か 1)
```

あとは Adam (勾配の移動平均で学習率を自動調整する SGD) で更新するだけ。

**自作するときの必須事項:**

- **数値微分で勾配チェックする** (`gradcheck.ts` / `cargo test`)。逆伝搬のバグは学習が「なんとなく遅い」という形でしか現れないので、テストしないと気づけない。この実装では相対誤差 3e-9。
- **乱数は自前の seed 付き PRNG にする**。再現できないと比較できない。
- **特徴量は 0〜1 程度にそろえる**: サイズや時間間隔は `log` を取る、カテゴリは one-hot、文字列は feature hashing (n-gram を固定長ベクトルにハッシュする。辞書が要らない)。
- **時系列で分ける**: 過去で学習して未来で評価する (cache 実験は先頭 30% で学習、残り 70% で評価)。ランダム分割すると未来の情報が漏れる。
- **デプロイは重みの配列を埋め込むだけ**。int8 量子化すれば重み 1 個 1 byte (Rust 版 `QuantMlp`)。

---

## 4. 実験結果

### 4.1 Learned Bloom filter (`learned-bloom.ts` / `learned_bloom.rs`)

設定: ブロックリスト (怪しい URL) 3 万件を鍵集合とし、問い合わせは主に普通の URL。
鍵の 15% は「乗っ取られた普通のサイト」で、見た目では判別できないようにしてある。

```
query(x) = model(x) >= τ  ||  backup_bloom.has(x)
```

backup にはモデルが取りこぼした鍵だけを入れるので、**偽陰性ゼロは構造的に保証される**。
τ は総ビット数が最小になるよう、検証用の負例で選ぶ。

目標 FPR 1%、TS 版:

| 構成 | 総サイズ | テスト FPR | **分布シフト FPR** | 問い合わせ |
|---|---|---|---|---|
| 普通の Bloom filter | 35.1 KB | 1.03% | 1.07% | 250 ns |
| ロジスティック回帰 (257 params) + backup | **7.1 KB** | 1.06% | **84%** | 4.2 µs |
| MLP 256-8-1 (2065 params) + backup | 13.9 KB | 1.01% | 85% | 6.6 µs |
| MLP 256-32-1 (8257 params) + backup | 38.0 KB | 1.04% | 85% | 12.8 µs |

目標 FPR 0.1% では、普通の Bloom が 52.7 KB、ロジスティック回帰版が 11.6 KB。

Rust 版 (int8 量子化つき):

| 構成 | 総サイズ | テスト FPR | 問い合わせ |
|---|---|---|---|
| 普通の Bloom filter | 35.1 KB | 0.99% | 69 ns |
| ロジスティック回帰 f32 | 6.9 KB | 0.99% | 1.5 µs |
| ロジスティック回帰 int8 | **6.2 KB** | 1.04% | 1.6 µs |
| MLP 256-8-1 f32 → int8 | 14.1 → 8.1 KB | 1.03% | 2.2〜2.5 µs |

読み取れること:

- **メモリは約 1/5**。鍵集合に「形」があれば、モデルがその形を覚え、backup は判別できない 15% (約 4.8k 件) だけを持てばよい。
- **一番小さいモデルが勝つ**。MLP にしても AUC は 0.982 → 0.987 と少ししか上がらず、モデルのサイズ増でそれ以上に損をする。int8 量子化は FPR をほとんど変えずにモデルを 1/4 にする。
- **分布シフトに極端に弱い**。「怪しい見た目だが鍵集合に無い URL」を問い合わせると FPR が 84% になる。普通の Bloom filter の FPR はどんな問い合わせでも保証されるが、learned 版の FPR は「問い合わせ分布が学習時と同じなら」という条件付き。攻撃者がクエリを作れる場面では使えない。
- **問い合わせは 20 倍以上遅い** (主に n-gram の特徴量抽出)。メモリ (L2/L3 に載るか、ネットワークで配布するか) が問題で、レイテンシには余裕がある場面向け。

### 4.2 キャッシュ admission (`cache-admission.ts`)

合成トレース 60 万リクエスト: Zipf 人気のカタログ 50%、二度と来ない one-hit wonder 32%、短期間だけ参照されるバースト 18%。
特徴量は 11 次元 (種別・クライアント種別・log サイズ・これまでの参照回数・前回参照からの間隔)。
ラベルは「H リクエスト以内に再来するか」で、トレースから自動で付く。
モデルは MLP 11-16-1 (209 params)、先頭 30% で学習 (0.7 秒)。

| ポリシー | OHR (cache 0.5%) | BHR (0.5%) | OHR (cache 2%) | BHR (2%) |
|---|---|---|---|---|
| LRU (全部入れる) | 14.7% | 8.1% | 27.5% | 19.1% |
| LRU + doorkeeper (2 回目で入れる) | 20.2% | 12.9% | 39.1% | 29.4% |
| **LRU + MLP admission** | **24.6%** | **16.8%** | **40.1%** | **30.5%** |
| LRU + oracle admission (未来を見る上限) | 27.1% | 19.1% | 44.1% | 33.9% |

OHR はオブジェクト単位のヒット率、BHR はバイト単位のヒット率。

読み取れること:

- **単純なヒューリスティック (doorkeeper) が改善の大半を取る**。キャッシュが大きい (2%) と、MLP の上乗せは +1pt 程度しかない。
- **キャッシュが小さく判断が難しいほど ML が効く** (0.5% では doorkeeper +5.6pt に対して MLP +10pt で、oracle との差の大半を埋める)。
- 注意: これは合成データで、特徴量が効くように作ってある。実データでは「どの特徴量が効くか」を探すのが仕事の大半になる。

### 4.3 パーセプトロン分岐予測 (`branch_predictor.rs`)

誤差逆伝搬すら使わない、整数だけのオンライン学習。
外れたときに `w_i += 正解(±1) × 履歴_i(±1)` とするだけ。

| 分岐の性質 | bimodal 1KB | gshare 12bit 1KB | gshare 16bit 16KB | **perceptron 8KB** |
|---|---|---|---|---|
| inner loop (TTTTTTTN) | 12.5% | 9.6% | 9.7% | **0.0%** |
| biased 90% | 11.0% | 11.0% | 11.0% | 10.1% |
| random 50% (R, S) | 50% | 50% | 50% | 50% (予測不能。正しい) |
| = R (直後) | 49.9% | 1.0% | 1.3% | **0.0%** |
| = R xor S (直後) | 49.9% | **5.3%** | **3.7%** | 49.5% ← 線形分離できない |
| = R (約 20 分岐後) | 49.9% | 49.9% | 49.8% | **0.0%** ← 長い履歴 |
| **合計** | 14.8% | 11.0% | 11.0% | **8.0%** |

読み取れること:

- 線形モデルの強み: **長い履歴の中から相関している 1 ビットを選べる**。gshare は表のサイズが履歴長に対して指数的に増えるが、パーセプトロンは線形にしか増えない。
- 線形モデルの限界: **XOR は学習できない**。これが隠れ層 (MLP) が必要になる理由そのもの。実際の CPU は複数の予測器を組み合わせて、この弱点を補っている。

---

## 5. まとめ: 自分のコードに入れるときの型

```
            ┌───────────── 特徴量 (log, one-hot, hashing) ──────────┐
request ──▶ │ 小さいモデル (LR / MLP / perceptron) → スコア → 閾値 τ │ ──▶ 判断
            └──────────────────────────────────────────────────────┘
                     ▲ オフライン or オンライン学習          │ 外れても
   ログ ── 後から分かる正解ラベル ─┘                          ▼ 正しさは古典構造が担保
```

1. まず **ヒューリスティックのベースラインと、未来を見た oracle (上限)** を測る。差が小さければ ML は不要。
2. **ラベルがログから自動で付く問題設定**にする (回帰より二値分類)。
3. **ロジスティック回帰から始める**。MLP は、相互作用が必要だと分かってから。
4. **フォールバックで正しさを担保する**。モデル単体に正しさを依存させない。
5. **分布シフトと敵対的入力を別枠でテストする**。

## 参考

- Jiménez & Lin, "Dynamic Branch Prediction with Perceptrons", HPCA 2001
- Kraska et al., "The Case for Learned Index Structures", SIGMOD 2018
- Mitzenmacher, "A Model for Learned Bloom Filters and Optimizing by Sandwiching", NeurIPS 2018
- Song et al., [Learning Relaxed Belady for Content Distribution Network Caching](https://www.usenix.org/conference/nsdi20/presentation/song), NSDI 2020
- Wong et al., [Baleen: ML Admission & Prefetching for Flash Caches](https://www.usenix.org/conference/fast24/presentation/wong), FAST 2024
- Hao et al., [LinnOS: Predictability on Unpredictable Flash Storage with a Light Neural Network](https://www.usenix.org/conference/osdi20/presentation/hao), OSDI 2020
- Trofin et al., [MLGO: a Machine Learning Guided Compiler Optimizations Framework](https://arxiv.org/abs/2101.04808), 2021 / [Google Research blog](https://research.google/blog/mlgo-a-machine-learning-framework-for-compiler-optimization/)
- Paul Graham, "A Plan for Spam", 2002
