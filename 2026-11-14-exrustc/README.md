# exrustc

自分用の Rust (サブセット) コンパイラ。Rust で書き、バックエンドは Cranelift。
**インクリメンタルな開発ビルド**に特化して、次の 4 つを小さくすることを狙う。

- 再ビルド時間
- ピークメモリ
- キャッシュのディスク消費
- 出力バイナリのサイズ

設計の元にした blaze / relax / wild の調査は [NOTES.md](NOTES.md) にまとめた。

## 動かし方

```bash
cargo build --release
./target/release/exrustc examples/hello.rs -o out/hello --time
./out/hello
```

| オプション | 意味 |
|---|---|
| `-o PATH` | 出力パス。同じ場所に `PATH.o`、`PATH.exrc`（キャッシュ）、`PATH.link`（リンク行）を置く |
| `-O` | Cranelift の `speed_and_size` と backtracking regalloc を使う。既定は `none` + `single_pass` |
| `--linker=wild\|mold\|lld\|bfd\|PATH` | リンカを選ぶ。既定は PATH にある順に wild > mold > ld.lld > ld |
| `--time` | フェーズごとの時間とピーク RSS を表示する |
| `-j N` | コード生成のスレッド数。ミスが 32 件未満ならスレッドを立てない |
| `--no-cache` | キャッシュを読まず、書かない |

```bash
cargo test --release
```

lexer の単体テストが走る。

```bash
tests/compare.sh
```

`examples/*.rs` を exrustc と rustc の両方でビルドし、実行結果が一致するかを確かめる。

```bash
python3 bench/run.py 1000 5000
```

rustc との比較ベンチ。`bench/gen.py` が N 関数のソースを生成し、cold、no-op、1 関数編集の各ケースを測る。

## 対応している言語サブセット

rustc でもそのままコンパイルできる範囲に限っている。そのため、出力を rustc の結果と比較して検証できる（`tests/compare.sh`）。

- アイテム：
  - `fn`
  - `unsafe extern "C" { fn ...; }`
  - 名前付きフィールドの `struct`
  - ライフタイム引数だけのジェネリクス（`<'a>`）は可。ライフタイムは消去する
- 型：
  - `i8`〜`i64`、`u8`〜`u64`、`isize`、`usize`、`bool`、`()`、`!`
  - `&T`、`&mut T`
  - struct（入れ子可。フィールドに `&'a T` も置ける）
- 式：
  - 算術、ビット演算、シフト、比較、`&&` `||`（短絡評価）、単項の `-` と `!`、`as`（整数と bool の間）
  - `let [mut]`、代入、複合代入（左辺はローカル変数・フィールド・`*r` を使える）
  - `if`/`else`（式として使える。struct も返せる）、`while`、`loop`、`break`、`continue`、`return`
  - 関数呼び出し（struct の値渡し・値返しを含む）、ブロック、`unsafe {}`
  - struct リテラル（`S { x, y: e }`）、フィールドアクセス（参照越しは自動 deref）
  - `&e` / `&mut e`（一時値への参照も可）、`*e`
- 型推論は関数単位の単一化（`src/infer.rs`）。
  - 整数リテラルの型は、後ろの使用箇所から決める（`let mut i = 0; bump(&mut i)` のような参照越しの使用も含む）。
  - 決まらなければ `i32` にする。
- 借用検査・可変性検査・ムーブ検査はしない。
  rustc を通るプログラムなら意味は変わらない前提で、struct は常にコピーしている。

### 値の表現と ABI（内部用）

- **スカラー**（整数・bool・参照）は SSA 値で表す。参照はポインタ。
- **ローカル変数**
  - アドレスを取られないスカラーは Cranelift の `Variable`（レジスタ）に置く。
  - `&x` で参照されるものと struct はスタックスロットに置く。
- **struct**
  - 値は常にメモリ上にあり、式の評価結果は「そのアドレス」になる。
  - let・代入・引数・return の時点でコピーする（サイズは静的に分かるので、load/store に展開する）。
  - 引数：呼び出し側が一時領域にコピーし、そのアドレスを渡す。
  - 戻り値：呼び出し側が確保した領域のアドレスを、隠れ第 1 引数（sret）で渡す。
  - extern "C" 関数との間で struct を値で受け渡すことはできない（エラーになる）。
- **レイアウト**：宣言順に自然なアラインメントで並べる（repr(C) 相当）。

## アーキテクチャ

```
src.rs ──lexer(自前)──▶ アイテム列 + 正規化ハッシュ
                          │ (空白・コメントは無視)
                          ▼
           要約キャッシュ? ── hit ──▶ シグネチャ
                 │ miss
                 ▼
          syn でそのアイテムだけ parse → シグネチャ
                          ▼
     識別子集合から参照グラフ → main からの到達集合
                          ▼
 関数キー = H(コンパイラ, 設定, 本体ハッシュ, 自分と参照先の「シグネチャ」,
               名前で現れる struct の deep hash)
                          ▼
           機械語キャッシュ? ── hit ──▶ Blob（機械語 + 名前付きリロケーション）
                 │ miss
                 ▼
  syn parse → infer → syn AST から直接 Cranelift IR → 機械語 → Blob
                          ▼
   cranelift-object で 1 個の .o にまとめる（関数ごとのセクション、Local シンボル）
                          ▼
   .o が前回と同一ならリンクを省略。違えば、保存したリンク行で wild を直接起動
```

| ファイル | 役割 |
|---|---|
| `src/lexer.rs` | トップレベルのアイテム分割と、正規化トークン列のハッシュ。syn より 1 桁以上速い |
| `src/front.rs` | 型（intern）、struct レイアウト、シグネチャ、アイテム要約とそのシリアライズ、参照グラフ、到達可能性 |
| `src/infer.rs` | 関数単位の型推論（union-find。参照の形も単一化する） |
| `src/codegen.rs` | syn AST から Cranelift IR への 1 パス lowering と型検査（place / スタックスロット / sret）。出力は `Blob` |
| `src/cache.rs` | 1 ファイルキャッシュ（index + payload、tmp + rename で書き込む） |
| `src/link.rs` | `cc -###` からリンク行を抽出して保存し、リンカを直接起動する |
| `src/main.rs` | ドライバ、フェーズ計測、並列コード生成 |

### インクリメンタル性の要点

1. **callee の本体を変えても caller は再コンパイルされない。**
   キーに入るのは参照先のシグネチャだけで、Blob は callee を名前で参照している（blaze の template と同じ考え方）。
   シグネチャを変えた場合だけ、それを参照している関数が無効になる。
2. **struct は deep hash で追跡する。**
   - deep hash は、その struct 自身と、フィールドから推移的に届く全 struct の定義ハッシュをまとめたもの。参照フィールド経由も含む。
   - シグネチャのハッシュにも、struct は deep hash で入る。
   - そのため、`fn get(o: &Outer) -> i64 { o.inner.b }` のように `Inner` を名前で書いていない関数でも、`Inner` のレイアウトが変われば正しく再コンパイルされる（確認済み）。
3. **整形やコメント編集では何も再コンパイルされない。**
   キャッシュは全ヒットし、`.o` も同一になるのでリンクも省略される。
4. **変わっていないアイテムは parse もしない。**
   ファイル全体を syn で parse すると、1000 関数（320KB）で 80ms かかる。自前 lexer なら 2.6ms で済む。
5. **到達しない関数は parse も型検査もコード生成もしない。**
   その結果、オブジェクトにも入らない。
6. **ディスク消費に上限がある。**
   保存するのは今回のビルドで使ったエントリだけなので、古い世代は自動的に消える。キャッシュのサイズは「今のコード量」程度に収まる。
7. **メモリ**
   - 同時に生きている AST は常に 1 関数分だけ。
   - Cranelift の `Context` と `FunctionBuilderContext` はスレッドごとに使い回す。
   - インクリメンタル時はスレッドを起動しない。
   - cold 時（要約のミスが 33 件以上）は、要約用の parse を並列にし、AST は持ち帰らない。
     全部保持すると、5000 関数では syn の AST だけでピーク RSS が 162MB になっていた。持ち帰らない形にして 35MB になった。

## 結果

環境は Linux x86-64 のクラウドコンテナ（4 コア相当）。rustc 1.97.0、wild 0.10.0、Cranelift 0.137。
`bench/gen.py` が生成する「N 個の関数 + 相互呼び出し」のプログラムで測った。どのツールの出力も rustc の実行結果と一致した。

### N = 1000 関数（320KB）

| tool | cold ms | no-op ms | 1 関数編集 ms | peak RSS (cold) | binary | cache / incr dir |
|---|---:|---:|---:|---:|---:|---:|
| exrustc（wild を直接起動） | 128 | 11.7 | **19.0** | **13.4 MiB** | 413 KiB | **498 KiB** |
| exrustc（bfd を直接起動） | 129 | 12.1 | 33.2 | 13.2 MiB | 422 KiB | 498 KiB |
| exrustc（lld を直接起動） | 141 | 12.0 | 36.7 | 58.7 MiB | 412 KiB | 498 KiB |
| rustc `-Cincremental -Cdebuginfo=0` | 887 | 217 | 461 | 164 MiB | 5013 KiB | 22151 KiB |
| rustc `-Cincremental -Cdebuginfo=2` | 1015 | 218 | 563 | 183 MiB | 5557 KiB | 24592 KiB |
| rustc `-O`（参考、非 incremental） | 12445 | - | - | 147 MiB | 382 KiB | - |

### N = 5000 関数（1.6MB）

| tool | cold ms | no-op ms | 1 関数編集 ms | peak RSS (cold) | binary | cache / incr dir |
|---|---:|---:|---:|---:|---:|---:|
| exrustc（wild を直接起動） | 497 | 63.6 | **87.0** | **34.4 MiB** | 2030 KiB | **2479 KiB** |
| exrustc（bfd を直接起動） | 471 | 53.9 | 87.0 | 34.6 MiB | 2038 KiB | 2479 KiB |
| rustc `-Cincremental -Cdebuginfo=0` | 3810 | 756 | 1911 | 499 MiB | 8040 KiB | 113644 KiB |

rustc と比べると、1 関数編集は 20〜24 倍速く、cold の RSS は 1/12〜1/14、キャッシュは 1/45 になった。
5000 関数の no-op が 60ms 前後あるのは、ファイル全体の lex、`.o` の作り直し、キャッシュの読み書きが N に比例するため。
マルチファイル化してファイル単位の要約にすれば、ここも O(変更量) にできる。

1 関数を編集したときの exrustc の内訳（`--time`、N=1000）：

| フェーズ | ms |
|---|---:|
| lex | 2.6 |
| 要約（1 アイテムだけ parse） | 1.3 |
| plan | 0.9 |
| キャッシュ参照 | 0.5 |
| codegen（1 関数） | 0.4 |
| .o 生成 | 3.6 |
| キャッシュ保存 | 1.2 |
| link（wild） | 5.6 |
| **合計** | **15.9（ピーク RSS 12 MiB）** |

hello（`examples/hello.rs`）の出力サイズは、exrustc で 6.0 KiB、rustc の debug（strip あり）で 333 KiB。
rustc 側はほとんどが std 由来。

### 途中で分かったこと

- **cold 時に要約用の syn AST を全部保持していたため、5000 関数で RSS が 162MB になっていた**。
  要約の parse を並列にし、AST をスレッド内で捨てるようにした結果、RSS は 35MB、cold は 855ms から 526ms になった。
- **初版は syn でファイル全体を parse していた**。そのため no-op でも 114ms かかり、cold では並列スレッドがそれぞれ全体を parse し直して RSS が 191 MiB になっていた。
  自前 lexer と要約キャッシュに変えた結果、no-op は 12ms、cold の RSS は 39 MiB になった。
- **リンクは cc ドライバのプロセス段数が支配的だった**。cc + lld は 21ms、ld.lld の直接起動は 17ms、wild の直接起動は 5ms。
- Cranelift の `single_pass` regalloc は、開発ビルドならこれで十分。

## 次にやること

- **言語**
  - `impl` とメソッド、配列とスライス、`match`、enum、タプル
  - ジェネリクス（単相化。レイアウトをキーにして共有する）
  - 最小の自前 std（relax 方式）
- **インクリメンタル**
  - α 正規化ハッシュ
  - マルチファイル / モジュールごとの要約
  - libwild の埋め込みと、メモリ上の .o（`.o` 書き出しと `exec` をなくす）
- **増分リンク**
  - `.text` に余白を残し、変更された関数だけを差し替える
  - ジャンプテーブル経由の呼び出し（blaze 方式）
- **cold ビルド**
  - 要約と parse を並列化する
  - cold 時に AST を 2 回 parse している点を解消する
- **サイズ**
  - コンパイラ側の ICF（コード + リロケーション先のハッシュ）
  - 文字列の merge セクション
- **`-O`**
  - 自前インライン展開と stack-slot forwarding（Cranelift に SROA がない分を補う）
