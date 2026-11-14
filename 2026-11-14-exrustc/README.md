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

```bash
./target/release/exrustc check examples/traits.rs
```

型検査だけを行い、全関数の診断を表示する。

```bash
./target/release/exrustc lsp
```

LSP サーバ（stdio）。対応は initialize / didOpen / didChange（全文同期）/ didClose / shutdown で、publishDiagnostics を返す。

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

テスト一式が走る。
- lexer の単体テスト
- LSP の結合テスト（実バイナリを stdio で駆動）
- check がエラー後も止まらないこと、キャッシュされた呼び出し元越しでもジェネリック本体のエラーを拾うこと
- サイズ予算（`tests/budget.rs`）

```bash
tests/compare.sh
```

`examples/*.rs` を exrustc と rustc の両方でビルドし、実行結果が一致するかを確かめる。

```bash
python3 bench/run.py 1000 5000
```

rustc との比較ベンチ。`bench/gen.py` が N 関数のソースを生成し、cold、no-op、1 関数編集の各ケースを測る。

### 回帰監視（criterion + サイズ予算）

```bash
bench/regress.sh save
```

機能を足す前に、現在の状態をベースライン `main` として保存する。

```bash
bench/regress.sh
```

ベースラインと比べる。次のどれかがあれば失敗する。
- criterion が「回帰」と判定したもの
- サイズ予算（`bench/budget.txt`）の超過
- rustc との出力差（`tests/compare.sh`）

| ベンチ（`benches/compile.rs`、N=1000） | 何を見るか | 現在値 |
|---|---|---:|
| `lsp/did_change_1fn` | LSP 応答性：1 関数編集の didChange から publishDiagnostics まで | 4.6 ms |
| `lsp/did_change_noop` | 内容が同じ didChange（保存など） | 0.25 ms |
| `cold/lsp_did_open` | 初めて開いたとき（全関数を検査） | 58 ms |
| `build/noop` | 無変更ビルド（リンク省略） | 9.2 ms |
| `build/edit_1fn` | 1 関数編集ビルド（リンク込み） | 18 ms |
| `cold/build` | キャッシュなしのビルド | 95 ms |
| `front/lex` | 字句解析だけ（no-op の下限） | 2.3 ms |

- 判定の閾値：5% 未満の変化は無視し（`noise_threshold`）、有意水準は 1% にしている。
  cold 系は実行ごとに ±10% ぶれるので、閾値を 15% にしている。
- サイズ予算は、バイナリ・`.o`・キャッシュ・LSP のメモリ上キャッシュを、例ごとに上限バイトで持つ。
  意図して増やすなら `UPDATE_BUDGET=1`、新しい例を足しただけなら `UPDATE_BUDGET=new`（既存の上限は変えない）。
- 回帰検出が実際に働いた例（どれも予算や閾値を緩めずに直した）：
  - サイズ
    - enum の追加で要約が 1 byte/アイテム増えた。要約の符号化を詰めて解決した。
    - 検査済みの印が 4 byte/関数増えた。空なら書かないようにした。
    - prelude を全部付けていたので、どのキャッシュも約 2.3 KB 増えた。使われるものだけを付けるようにした。
    - フィールド名 `min` / `max` で `core::cmp` の関数まで付いていた。`core` という識別子が現れたときだけにした。
  - 速度
    - no_std 対応で、LSP の 1 関数編集が +31%、no-op ビルドが +10% 遅くなった。
    - 時間帯によるぶれを除くため、ベースラインのコミットを worktree で同時にベンチし、callgrind で原因を詰めた。
    - 直したもの：毎回の全識別子のハッシュ（prelude の選択）、LSP が本文全体をハッシュしていたこと、名前表の SipHash。
    - 名前表は自前の FxHasher に替えた。キャッシュのキーには使わない。
    - 結果、LSP の 1 関数編集は −7%、no-op ビルドは −4% と、ベースラインより速くなった。

## 対応している言語サブセット（no_std の範囲）

当面のゴールは「no_std で扱う範囲が動くこと」。具体的には、次の条件を満たすプログラムを対象にしている。

- alloc を使わない。
- 書式化マクロ（`write!` / `format_args!`）を使わない。
- `#![no_std]` / `#![no_main]` / `#[panic_handler]` / `#[unsafe(no_mangle)] pub extern "C" fn main` を使ってよい。

検証は、そうしたプログラムを rustc でもビルドし、実行結果（標準出力と終了コード）が一致することで行う。

- rustc 側は libcore だけでビルドする（`-C panic=abort`）。
- 比較は `tests/compare.sh` が行う。対象は `examples/*.rs` の 13 本で、そのうち no_std は 6 本（`nostd_calc` は字句解析と後置記法の電卓で、ジェネリック struct・`?`・const 配列・`&&str` の自動 deref を使う）。

### 言語

- アイテム：
  - `fn`
  - `extern "C"` の宣言と定義
  - `struct`：名前付き / タプル / unit
  - `enum`：unit / tuple / struct のバリアントと、判別値
  - `const`、`static`、`static mut`
  - `use`（名前は最後のセグメントで解決する）
  - クレート属性（`#![no_std]` など）
- impl と trait：
  - 固有 `impl`
  - `trait`（必須メソッドとデフォルトメソッド）
  - `impl Trait for S`：struct / enum / プリミティブ型（`impl Tr for u32`）
  - `#[derive(Clone, Copy, PartialEq, ..)]`
- ジェネリクス：
  - 関数（`<T: A + B>`、`where`、turbofish）
  - struct / enum（`Pair<T>`、`Option<T>`）
  - `impl<T> X<T>`、`impl<T> Option<&T>` のような self 型パターン
  - すべて呼び出し元で型引数を推論し、単相化する。ライフタイムは消去する
- 型：
  - 整数（`i8`〜`i64`、`u8`〜`u64`、`isize`、`usize`）、`bool`、`char`、`()`、`!`
  - `&T`、`&mut T`、`*const T`、`*mut T`
  - `[T; N]`（N はリテラルか const）、`&[T]`、`&mut [T]`、`&str`
  - タプル、struct、enum
  - `core::ffi::c_int` などの C 型の別名
- 式：
  - 算術・ビット演算・比較・短絡評価・`as`
    - キャストは整数 ⇔ char / bool、参照 → ポインタ、ポインタ ⇔ usize、フィールドのない enum → 整数
  - `let`（分解も）、代入、複合代入
    - 左辺はローカル変数・フィールド・`*p`・`a[i]`・static
  - `if` / `match` / `if let` / `while let` / `while` / `loop` / `for`（`break` / `continue` / `return`）
  - `for` で回せるもの：
    - `a..b`、`a..=b`、`(a..b).rev()`
    - 配列・スライス（`&xs`、`&mut xs`、値渡し）と、それに `.iter()` / `.iter_mut()` / `.copied()` / `.enumerate()` / `.rev()` を付けたもの
  - 構築：struct / タプル / enum / 配列（`[a, b]`、`[x; N]`）
  - 添字 `a[i]`（範囲外なら panic）、`&a[lo..hi]`（`..` / `lo..` / `..=hi` も）
  - `&[T; N]` → `&[T]` の暗黙の変換
  - `==` / `!=`：struct / enum / タプル / 配列は構造的に比べる。手書きの `eq` があればそれを呼ぶ
  - リテラル：`"..."`（`&'static str`）、`b"..."`、`b'x'`、`'x'`、負のリテラル
  - `?`（Option / Result）
  - マクロ：`panic!` / `unreachable!` / `todo!` / `assert!` / `assert_eq!` / `assert_ne!` / `debug_assert*!` / `matches!`
    - メッセージは書式化しない
- パターン：
  - `_`、束縛（`ref` / `mut`）、`x @ p`、リテラル、範囲、`|`
  - バリアント、struct、タプル、`&p`、入れ子、ガード
  - 参照を match したときのデフォルト束縛モード
  - `None` などの unit バリアント
- メソッド：
  - 自動 deref と自動参照、UFCS（`S::m(&x)` / `Trait::m(&x)` / `<S as Trait>::m(&x)`）
  - 静的ディスパッチ。固有メソッド → trait 実装 → デフォルトメソッドの順に探す
- panic：
  - `#[panic_handler]` があればそれを呼ぶ（範囲外アクセス・`unwrap`・`assert!`・`panic!` など）
  - なければ trap する

### core の代わり（組み込みの prelude、`src/prelude.rs`）

- `Option<T>`：`is_some` / `is_none` / `unwrap` / `unwrap_or` / `ok_or` / `or` / `xor` / `take` / `replace` / `as_ref` / `as_mut` / `copied` / `cloned`
- `Result<T, E>`：`is_ok` / `is_err` / `unwrap` / `unwrap_err` / `unwrap_or` / `ok` / `err`
- `core::panic::PanicInfo`
- `core::cmp::{min, max}`、`core::mem::{swap, replace, size_of, align_of}`、`core::ptr::{null, null_mut, read_volatile, write_volatile}`
- 組み込みメソッド：
  - 整数：`wrapping_*` / `checked_*`（→ Option）/ `saturating_*` / `min` / `max` / `abs` / `abs_diff` / `signum` / `pow` / `count_ones` / `leading_zeros` / `trailing_zeros` / `is_power_of_two` / `rotate_*` / `swap_bytes` / `is_ascii_*` / `to_ascii_*`
  - 関連定数：`u32::MAX` / `MIN` / `BITS`
  - 配列・スライス：`len` / `is_empty` / `get` / `get_mut` / `first` / `last` / `swap` / `contains` / `as_ptr` / `as_mut_ptr`
  - `&str`：`len` / `is_empty` / `as_bytes` / `as_ptr`
  - 生ポインタ：`add` / `sub` / `offset` / `is_null` / `read` / `write` / `read_volatile` / `write_volatile` / `cast`

prelude は、ユーザのコードの識別子から辿れるアイテムだけを付ける。`Option` を使わないプログラムには何も足さないので、キャッシュもビルド時間も増えない。

- `core::cmp::max` などは、ユーザが同じ名前の関数を定義してもぶつからないよう `__core_*` という名前で持っている。
- それらは、コードに `core::` か `use core::` が現れたときだけ付ける。

### 型推論と検査

- 型推論は関数単位の単一化で行う（`src/infer.rs`）。
  - 参照・ポインタ・配列・スライス、ジェネリックな ADT の形も単一化する。
  - リテラル・構築・呼び出しの型も記録し、codegen で期待型が分からないときに使う（`Pair { a: 1 }.swap()` の `1` を、後ろの使用箇所から `i64` に決める）。
- 借用検査・可変性検査・ムーブ検査・match の網羅性検査・算術オーバーフロー検査はしない。
  - rustc を通るプログラムなら意味は変わらない前提で、struct は常にコピーしている。
  - どの arm にも合わなければ trap する。
  - オーバーフローは wrap する（rustc の debug ビルドなら panic する）。

### 未対応

- alloc（`Vec` / `Box` / `String`）
- 書式化（`write!` / `core::fmt`）
- クロージャ
- `dyn Trait`、関連型、関連定数（`u32::MAX` などの組み込みは可）
- 浮動小数点
- 128 bit 整数
- モジュール（`mod`）、マクロ定義（`macro_rules!`）
- `|` パターンの中での束縛
- ラベル付き `break`
- イテレータのアダプタ（`for` の中で直接使う形以外）
- `impl Trait for [T; N]` のような、プリミティブ以外の組み込み型への impl

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
| `src/lexer.rs` | トップレベルのアイテム分割と、正規化トークン列のハッシュ。syn より 1 桁以上速い。`impl` はヘッダとメンバに分ける |
| `src/front.rs` | 型（intern）、struct レイアウト、シグネチャ、アイテム要約とそのシリアライズ、参照グラフ、到達可能性 |
| `src/infer.rs` | 関数単位の型推論（union-find。参照の形も単一化する） |
| `src/codegen.rs` | syn AST から Cranelift IR への 1 パス lowering と型検査（place / スタックスロット / sret）。出力は `Blob` |
| `src/cache.rs` | 1 ファイルキャッシュ（index + payload、tmp + rename で書き込む） |
| `src/link.rs` | `cc -###` からリンク行を抽出して保存し、リンカを直接起動する |
| `src/driver.rs` | `Session`：2 層キャッシュ（ディスク + メモリ）、check / build、インスタンスの作業リスト、並列化、.o 生成 |
| `src/lsp.rs` | 最小の LSP サーバ（ドキュメントごとに Session を持つ） |
| `src/main.rs` | CLI（build / check / lsp） |

### インクリメンタル性の要点

（単相化について）
- ジェネリック関数とデフォルトメソッドは、要求された（関数, 型引数）ごとにインスタンスになる。
  キーは「元の関数のキー + 型引数（struct は deep hash）」。
- インスタンスは作業リストで不動点まで処理する。キャッシュした呼び出し元の Blob には、インスタンス参照を（元の関数名, 型引数）で保存してある。そのため読み戻すと、必要なインスタンスがまた要求される。
- 実測（examples/traits.rs、25 関数 / 11 インスタンス）：
  - `total_area<T>` の本体を編集すると、再コンパイルはその 2 インスタンスだけ。
  - デフォルトメソッド `score` の本体を編集すると、再コンパイルはそれを使う 2 型分だけ（上書きしている Rect は対象外）。
- check（LSP）でも、検査済みの印に「その関数が要求したインスタンス」を入れてある。
  そのため、呼び出し元がキャッシュに当たっても、ジェネリック本体の型エラーは報告される。

1. **callee の本体を変えても caller は再コンパイルされない。**
   キーに入るのは参照先のシグネチャだけで、Blob は callee を名前で参照している（blaze の template と同じ考え方）。
   シグネチャを変えた場合だけ、それを参照している関数が無効になる。
2. **struct は deep hash で追跡する。**
   - deep hash は、その struct 自身と、フィールドから推移的に届く全 struct の定義ハッシュをまとめたもの。参照フィールド経由も含む。
   - シグネチャのハッシュにも、struct は deep hash で入る。
   - そのため、`fn get(o: &Outer) -> i64 { o.inner.b }` のように `Inner` を名前で書いていない関数でも、`Inner` のレイアウトが変われば正しく再コンパイルされる（確認済み）。
3. **`impl` ブロックはメソッド単位で追跡する。**
   - lexer が `impl X {` のヘッダとメンバを別のアイテムに分ける。メンバのハッシュにはヘッダのハッシュを混ぜる。
   - そのため、メソッドを 1 つ編集しても parse し直すのはそのメソッドだけで、再コンパイルもそのメソッドだけ（確認済み：18 関数中 1 miss）。
   - シグネチャを変えた場合は、そのメソッドと呼び出し元だけが再コンパイルされる（2 miss）。
   - `x.len()` のようなメソッド呼び出しは、型を見ずに名前の最後のセグメント（`len`）で参照グラフに入れる。どの型の `len` も参照したことになるので過大近似だが、安全側になる。
4. **整形やコメント編集では何も再コンパイルされない。**
   キャッシュは全ヒットし、`.o` も同一になるのでリンクも省略される。
5. **変わっていないアイテムは parse もしない。**
   ファイル全体を syn で parse すると、1000 関数（320KB）で 80ms かかる。自前 lexer なら 2.6ms で済む。
6. **到達しない関数は parse も型検査もコード生成もしない。**
   その結果、オブジェクトにも入らない。
   その副作用として、到達しない関数の中の型エラーは報告されない（rustc なら報告される）。
7. **ディスク消費に上限がある。**
   保存するのは今回のビルドで使ったエントリだけなので、古い世代は自動的に消える。キャッシュのサイズは「今のコード量」程度に収まる。
8. **メモリ**
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
  - クロージャ、`dyn Trait`、モジュール、`core::fmt` 相当の書式化
  - 浮動小数点、オーバーフロー検査（debug ビルドの rustc と同じ panic）
  - 単相化をレイアウトでキーにして共有する（`Vec<A>` と `Vec<B>` を 1 つにする。blaze の polymorph）
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
