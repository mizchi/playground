# 調査メモ: blaze / relax / wild

exrustc の設計に使うため、3 つのプロジェクトを読んで整理した（2026-11-14 時点の main をソースで読んだだけで、ビルドはしていない。wild だけは crates.io の 0.10.0 を入れて実際に使った）。

- blaze: https://github.com/indexable-inc/blaze
- relax: https://github.com/makepad/relax
- wild: https://github.com/wild-linker/wild

## 一覧

| | blaze | relax | wild |
|---|---|---|---|
| 何か | cargo の代わり。常駐 daemon が rustc を使わずにビルドする | Rust の JIT とライブコンパイラ | 高速な ELF リンカ |
| フロントエンド | rust-analyzer の fork（rowan、hir-def/hir-ty）。trait solver は rustc から vendor | 全部自前（lexer、Pratt parser、rustc から移植した typeck と next-solver）。依存クレートなし | - |
| IR | HIR → MIR → (CLIF / LIR) | typeck::Body → RIR（非 SSA の vreg、CFG） | - |
| バックエンド | 今は MIR → CLIF。v2 では開発ビルドを copy-and-patch stencil、リリースを LLVM にする方針で、ネイティブコード生成に Cranelift は使わない | 自前（interp → O1 → O3、linear scan regalloc） | - |
| コンパイル単位 | 単相化した関数インスタンス 1 個 | 関数インスタンス 1 個（初回呼び出しで lazy に） | 入力セクション |
| キャッシュキー | BLAKE3 で、本体を α 正規化しスパンは入れない。callee は interface hash だけ | 128bit。compiler ID + インスタンス + 依存の interface 指紋 + 本体 | - |
| リンク | 自前リンカ（約 89k 行）。in-place patch とジャンプテーブル | リンカなし（JIT）。startup image あり | 並列。`--gc-sections` は既定で有効、ICF なし、LTO はプラグイン経由のみ |

## blaze から真似できること

1. **関数単位キャッシュ + callee は interface hash だけ**（`crates/blaze-codegen/src/template.rs`）。
   コンパイル済みコードは callee を「参照リストの位置」で持つので、callee の本体を編集しても caller は再コンパイルされない。リロケーションの名前が付け替わるだけ。
   → exrustc でそのまま採用した（`Blob` のリロケーションは関数 ID か名前で持つ）。
2. **本体ハッシュに空白・コメント・スパンを入れない**（`docs/v2/plan/design/item-hash.md`）。
   blaze はさらにローカル変数を de Bruijn 風の index に α 正規化するので、ローカル変数のリネームでも無効化されない。
   → exrustc は lexer レベルの正規化まで。α 正規化はまだ。
3. **到達可能性**：main、テスト、export から到達するものだけをコンパイルする。
4. **ICF とポリモーフ化はリンカではなくコンパイラでやる**：
   - レイアウト（size/align/ABI）でキーを作るので `Vec<A>` と `Vec<B>` が共有できる。
   - コード + リロケーション先のハッシュで同一関数を畳む（`dedupe.rs`）。
   - 定数だけが違う関数をパラメータ化する（`fold.rs`）。
5. **開発ビルドではシンボル名を短いハッシュにし、line table だけにする**。
6. **増分リンク**：`.text` に余白を残しておき、変更された関数は末尾に追記し、ジャンプテーブル経由で呼ぶ。リンクが支配的になってから検討すれば十分。
7. **やらなくていいもの**：daemon、DBSP、stencil。モノレポ規模の仕組みで、個人用には過剰。

## relax から真似できること

1. **demand-driven**：名前解決も型検査も、到達したものだけ行う（`front/src/lazy.rs`、`demand.rs`）。AOT なら「main からの到達集合だけをコンパイル」になる。
2. **キャッシュ形式**（`compiler/src/cache.rs`、`store.rs`）：
   - 1 クレート = 1 ファイル。ヘッダの後にソート済みの index、その後に append-only の payload。A/B の index スロットを切り替えて crash-safe にしている。
   - リロケーションは安定な ID（定義キー、文字列のバイト列など）で保存する。
   - インライン展開したときは、取り込んだ関数のキーも記録する。
   → exrustc も 1 ファイルにした。index + payload で、デコードはヒットしたときだけ。
3. **自前の小さな std**（約 120k 行）。rust-src をコンパイルしないので、ビルド時間もバイナリも小さくなる。
4. **最適化 tier の要点**（LLVM 比 1.12x）：
   - インライン展開（rustc のコストモデル）。
   - スタックスロットの SROA 系。Cranelift には SROA がないので、ここは自前で書く必要がある。
   - LICM、値域解析による境界チェック削除。
5. **パーサはエラー回復なしで最初のエラーで止まる**。個人用ならそれで十分。

## wild から真似できること / exrustc 側でやるべきこと

1. **オブジェクトの出し方**：
   - 関数ごとにセクションを分ける（`ObjectBuilder::per_function_section(true)`）。これがないと `--gc-sections` で何も消えない。
   - export しないものは Local/hidden にする。
   - `-no-pie` で `R_*_RELATIVE` を出さない。
   - 文字列は `SHF_MERGE|SHF_STRINGS` セクションに置く。ただし merge セクションは GC されないので、死んだデータを入れないこと。
2. **LTO は linker plugin（LLVM bitcode / GIMPLE）だけ**。Cranelift には該当する IR がないので、wild はただのネイティブリンカになる。
   **インライン展開、DCE、単相化の重複除去、ICF はコンパイラ側で whole-program にやるしかない。**
   → exrustc は 1 プログラム = 1 オブジェクトにして、到達可能性による DCE をコンパイラ側でやっている。
3. **cc ドライバを経由しない**：
   - 実測で、cc + lld は 21ms、wild の直接起動は 5ms だった。
   - `libwild` クレートを使えばライブラリとして呼べるし、`FileSystem` trait でオブジェクトをメモリから直接渡せる。`.o` を書く必要がなくなる。
4. **増分リンクは wild の最終目標だが、まだ実装されていない**。
5. 実装テクニック：
   - mmap して `'data` で借用し、コピーしない。
   - `u32` の dense ID と Vec の表を使う。
   - 名前のハッシュは 1 回だけ計算して使い回す。
   - 出力を先にサイズ決めしてから、互いに素な `&mut` スライスに分けて並列に書く。
   - fork して後片付けを裏に回す。

## exrustc で採用したもの（優先度順）

| アイデア | 出典 | 状態 |
|---|---|---|
| 関数単位の機械語キャッシュ。callee はシグネチャだけをキーに入れる | blaze template / relax cache | 実装済み |
| 空白・コメントを無視したアイテムハッシュ（自前 lexer） | blaze item-hash | 実装済み |
| 変更したアイテムだけ syn で parse（要約キャッシュ） | relax demand / blaze early cutoff | 実装済み |
| main からの到達可能性。到達しない関数は parse もしない | blaze / relax | 実装済み |
| 1 ファイルキャッシュ。今回使ったエントリだけ書き戻すので自動 GC になる | relax store | 実装済み |
| 関数ごとのセクション、Local シンボル、`-no-pie`、`--gc-sections`、strip | wild | 実装済み |
| リンク行をキャッシュして、リンカ（wild）を直接起動する | wild 実測 | 実装済み |
| オブジェクトが同一ならリンクを省略 | - | 実装済み |
| ミスの多いとき（cold）だけスレッド並列。各スレッドは担当アイテムだけを parse | relax compile_split | 実装済み |
| α 正規化ハッシュ（ローカル変数のリネームで無効化しない） | blaze | 未実装 |
| ICF（コード + リロケーション先のハッシュ） | blaze dedupe | 未実装 |
| レイアウトをキーにした単相化 | blaze polymorph | 未実装（ジェネリクスがまだない） |
| 自前インライン + slot forwarding（-O 時） | relax | 未実装 |
| libwild の埋め込み、メモリ上のオブジェクト | wild | 未実装 |
| 増分リンク（余白 + ジャンプテーブル） | blaze patch-in-place | 未実装 |
