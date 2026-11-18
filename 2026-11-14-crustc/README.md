# crustc

自分用の Rust (サブセット) コンパイラ。Rust で書き、自前のフロントエンド (lexer / parser / 型推論) と、
Cranelift (既定) か自前の minclif でコード生成する。crustc 自身で crustc をコンパイルできる (minclif の経路)。
**インクリメンタルな開発ビルド**に特化して、次の 4 つを小さくすることを狙う。

- 再ビルド時間
- ピークメモリ
- キャッシュのディスク消費
- 出力バイナリのサイズ

設計の元にした blaze / relax / wild の調査は [NOTES.md](NOTES.md) にまとめた。

### crate の構成 (このディレクトリの Cargo ワークスペース)

層は base → syntax → sema → typeck → codegen → core (driver) の順。syntax・sema・typeck がコンパイラと LSP・解析で
共有する部分で、LSP は core を feature `build` なしで使い、バックエンド (codegen / minclif / Cranelift) に依存しない。

| crate | 中身 | 依存 |
|---|---|---|
| `crates/base` (`crustc-base`) | fxhash (FxMap / FxSet)・key (キャッシュのキー)・prof (段ごとの時間) | なし |
| `crates/syntax` (`crustc-syntax`) | front (前処理 → アイテムへの分割 (差分) → prelude を付ける、の流れと編集をまたぐ記憶 FrontMemo) / lexer / parse (自前の構文木) / cfg / macros / ctors / prep (前処理の差分) / srcmap / stub / types (要約の型)・types_enc / enc (直列化) / ast_types / ast_closure / ast_summary (アイテムの要約) / prelude (選択と prelude_lib.rs.in、build.rs) | base、proc-macro2 |
| `crates/sema` (`crustc-sema`) | program (名前・型・トレイト・impl の表、インスタンス) / solver / ast_infer (型推論) / ast_consteval | syntax |
| `crates/ir` (`crustc-ir`) | 命令の列の型 (Type / Value / Block / Op …) と、命令を記録するだけの Recorder (値の型を覚える) | なし |
| `crates/typeck` (`crustc-typeck`) | acg (構文木 → 命令の列の記録。型の検査はここで出る) / cache2 (Blob の直列化) | sema、ir、proc-macro2 |
| `crates/codegen` (`crustc-codegen`) | lower (記録を minclif に組み立て直して機械語にする。Cranelift なら crustc-clif に渡す) / link | typeck、minclif |
| `crates/core` (`crustc-core`) | driver2 (Session2: 解析・check・キャッシュ) と、feature `build` (既定) の driver2/build.rs (機械語・fork の並列化・オブジェクト・リンク・ワークスペース)。下の層を公開し直す | typeck、(build なら) codegen |
| `crates/ide` (`crustc-ide`) | エディタ向けの解析: symbols (documentSymbol / workspace/symbol のシンボル) / navigate (定義・参照・hover・補完。字句の列と定義の表からの軽い型の推定)。コンパイラ本体からは使わない | syntax |
| `crates/lsp` (`crustc-lsp`) | LSP サーバ (バイナリ `crustc-lsp`)。診断は driver2 の check、それ以外は ide。ワークスペースの本文はメモリの上限まで | core (build なし)、ide、serde_json |
| `crates/minclif` | Cranelift に似た API の小さな x86-64 バックエンド (レジスタ割り当て・スタックの struct のフィールドの変数化・ELF の .o)。`from_recording` で記録を組み立て直す | ir |
| `crates/clif` (`crustc-clif`) | driver2 の Cranelift バックエンド: minclif が組み立てた命令の列を Cranelift の IR にしてコンパイルする | minclif、cranelift |
| `.` (`crustc`) | CLI と、上のものの再輸出 (テスト・ベンチ)。ide と lsp には依存しない (ベンチの lsp/* だけが dev-dependency で使う) | 上のもの (ide・lsp を除く) |

各 crate の lib.rs は下の層のモジュールを公開し直す (`pub use crustc_syntax::*;` など)。ファイルの中は `crate::lexer` の
ように書き、どの crate に置いても、セルフホストで全部を 1 つのクレートのモジュールに並べても同じパスで引ける。

型の検査とバックエンドの分け方: acg は命令を crustc_ir::Recorder に記録するだけ (値・ブロックなどに番号を振り、値の型を
minclif と同じ決め方で覚える)。check はそこで止め、ビルドは codegen の lower が記録を minclif の FunctionBuilder に同じ順で
組み立て直してから機械語にする (minclif は命令の中で値やスロットを作るので番号は組み立て直す側で振り直す。
`CRUSTC_IR_VERIFY=1` で値の型が記録と同じかを確かめる)。出力の機械語は分ける前とバイト単位で同じ。

経路は **driver2** だけ (`crustc x.rs` は Cranelift、`--backend=minclif` で minclif)。build / check / LSP のすべてが driver2 を使う。
最初のドライバ (syn + Cranelift、`crates/legacy`) は 2026-11-16 に削除した (git の履歴にある)。
セルフホスト (tests/selfhost*.sh) は base / syntax / sema / ir / typeck / codegen / core と minclif のソースを 1 ファイルにまとめて crustc でコンパイルする。

## 現状（2026-11-15、`python3 bench/vs_rustc.py`）

同じソースを同じ手順（cold → no-op → 1 か所編集）で crustc と rustc 1.97.0 でビルドし、残るディスク・バイナリ（strip 後）・実行時間を比べた。
crustc のリンカは wild 0.10.0。どの組み合わせでも出力は rustc `-O` と一致した。全結果（no-op を含む）は `bench/vs_rustc_result.md`。

| プログラム | 設定 | cold ms | edit ms | disk KiB | binary KiB (strip) | run ms（rustc -O 比） |
|---|---|---:|---:|---:|---:|---:|
| gen1000 | crustc | 118 | 28 | 104.9 | 520.9 | 12（4.97x） |
|  | crustc -O | 169 | 30 | 93.9 | 273.3 | 6（2.58x） |
|  | rustc -Cincremental | 827 | 417 | 22156.3 | 1043.4 | 9（3.78x） |
|  | rustc -O | 11448 | - | 0.0 | 382.4 | 2（1.00x） |
| fib | crustc | 24 | 4 | 19.0 | 18.8 | 99（3.87x） |
|  | crustc -O | 34 | 5 | 16.0 | 13.8 | 73（2.85x） |
|  | rustc -Cincremental | 127 | 97 | 312.4 | 335.6 | 95（3.73x） |
|  | rustc -O | 111 | - | 0.0 | 334.6 | 25（1.00x） |
| hashmap | crustc | 37 | 6 | 35.1 | 27.3 | 218（1.91x） |
|  | crustc -O | 41 | 6 | 29.3 | 18.9 | 176（1.55x） |
|  | rustc -Cincremental | 211 | 107 | 2632.2 | 373.7 | 732（6.43x） |
|  | rustc -O | 280 | - | 0.0 | 347.8 | 114（1.00x） |
| matmul | crustc | 51 | 7 | 46.1 | 42.2 | 198（6.32x） |
|  | crustc -O | 56 | 7 | 37.9 | 26.3 | 53（1.69x） |
|  | rustc -Cincremental | 162 | 98 | 2671.3 | 378.4 | 692（22.13x） |
|  | rustc -O | 211 | - | 0.0 | 361.0 | 31（1.00x） |
| nbody | crustc | 36 | 6 | 36.3 | 39.9 | 413（9.83x） |
|  | crustc -O | 46 | 5 | 28.4 | 22.6 | 69（1.65x） |
|  | rustc -Cincremental | 143 | 105 | 2226.5 | 365.9 | 1185（28.22x） |
|  | rustc -O | 165 | - | 0.0 | 363.9 | 42（1.00x） |
| sieve | crustc | 26 | 4 | 20.4 | 20.5 | 211（2.30x） |
|  | crustc -O | 29 | 4 | 16.8 | 14.2 | 130（1.42x） |
|  | rustc -Cincremental | 131 | 94 | 1475.6 | 340.4 | 431（4.69x） |
|  | rustc -O | 128 | - | 0.0 | 335.0 | 92（1.00x） |
| sort | crustc | 42 | 7 | 40.7 | 46.4 | 252（1.33x） |
|  | crustc -O | 51 | 8 | 32.3 | 30.0 | 195（1.03x） |
|  | rustc -Cincremental | 174 | 113 | 3102.2 | 384.9 | 1983（10.45x） |
|  | rustc -O | 281 | - | 0.0 | 348.0 | 190（1.00x） |
| strings | crustc | 38 | 7 | 41.3 | 33.7 | 113（3.32x） |
|  | crustc -O | 52 | 6 | 33.7 | 22.3 | 73（2.17x） |
|  | rustc -Cincremental | 172 | 102 | 2198.4 | 357.3 | 121（3.57x） |
|  | rustc -O | 187 | - | 0.0 | 339.8 | 34（1.00x） |

- 1 か所編集の再ビルドは rustc の incremental の 13〜24 倍速い（gen1000 で 28 ms 対 417 ms）。
- 残すディスク（crustc はキャッシュ + 圧縮した `.o`、rustc は incremental ディレクトリ）は 1/16〜1/210（gen1000 で 105 KiB 対 22156 KiB）。
- バイナリは、小さいプログラムでは rustc の 1/8〜1/18（rustc は std を静的にリンクする）。gen1000 は debug の検査の分だけ rustc `-O` より大きい。
- 実行時間は、crustc の debug ビルドが rustc の debug ビルドより速いものが多い（gen1000 と fib を除く）。crustc `-O` は rustc `-O` の 1.0〜2.9 倍（sort は同じ、sieve は 1.4 倍、matmul・nbody は 1.6〜1.7 倍）。

## 動かし方

```bash
cargo build --release
./target/release/crustc examples/hello.rs -o out/hello --time
./out/hello
```

```bash
./target/release/crustc build tests/workspace/workspace.txt -o out/ws   # 複数クレートのワークスペース (書き方は下の「複数ファイル・複数クレート」)
```

```bash
./target/release/crustc check examples/traits.rs
```

全関数 (到達しないものも、総称関数は要求されたインスタンスごと) を検査して全診断を表示する (`Session2::check`。
コード生成までして minclif でエラーを集める。結果はキャッシュに入る)。

```bash
cargo build --release -p crustc-lsp
./target/release/crustc-lsp
```

LSP サーバ（stdio、crustc とは別のバイナリ）。対応は initialize / didOpen / didChange（全文同期）/ didClose / shutdown /
documentSymbol / workspace/symbol / definition / references / hover / completion で、publishDiagnostics を返す。
`CRUSTC_LSP_MEM_MB` (本文を持つ上限、既定 4)・`CRUSTC_LSP_NO_STD` (std の索引を作らない)・`CRUSTC_LSP_SYSROOT`。
bench/lsp_compare.py と bench/lsp_parity.py は `target/release/crustc-lsp` を使う (`cargo build --release --workspace`)。

| オプション | 意味 |
|---|---|
| `-o PATH` | 出力パス。同じ場所に `PATH.o`、`PATH.crcache2`（キャッシュ）、`PATH.link`（リンク行）を置く |
| `-O` | 小さい関数のインライン展開と、Cranelift の `speed` + backtracking regalloc (minclif ならレジスタ割り当ての後の命令の形)。オーバーフローの検査をしない |
| `--backend=cranelift\|minclif` | コード生成 (既定 cranelift。環境変数 `CRUSTC_BACKEND` でも) |
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

`examples/*.rs` を crustc と rustc の両方でビルドし、実行結果が一致するかを確かめる。

```bash
python3 bench/run.py 1000 5000
```

rustc との比較ベンチ。`bench/gen.py` が N 関数のソースを生成し、cold、no-op、1 関数編集の各ケースを測る。

```bash
python3 bench/vs_rustc.py
```

本物の rustc との比較（上の「現状」の表）。gen1000 と `bench/rt/*.rs`（fib / hashmap / matmul / nbody / sieve / sort / strings）を、
crustc（debug / `-O`）と rustc（`-Cincremental` / `-O`）でビルドし、cold・no-op・1 か所編集の時間、ディスク、バイナリ、実行時間を表にする。
`NAME` を並べればそのプログラムだけ。`python3 bench/runtime.py` は実行時間とバイナリだけを比べる軽い版。

```bash
tests/probe.sh
```

よく使う機能の小さいプログラム（`tests/probe/*.rs`、91 本）を rustc と比べ、通った数を出す。

```bash
tests/selfhost.sh
```

セルフホストの確認。crustc 自身のソースのうち外部クレートに依存しない部分を crustc でコンパイルし、rustc でコンパイルしたものと出力を比べる。
- `src/lexer.rs`（と `src/cache.rs` の KeyHasher = std の DefaultHasher 2 つ）：crustc 自身のソース・examples・probe の 144 ファイルを字句解析し、アイテムの区切りと 128 bit のハッシュが rustc 版と完全に一致する。
- `src/prelude.rs`（prelude の項目を名前で選ぶ。lexer / fxhash / KeyHasher と、build.rs が作る 46 万文字の表 `PRELUDE_ITEMS` を含む）：probe と examples の 125 ファイルをユーザのソースとして、選ばれた prelude の長さ・行数・ハッシュが rustc 版と一致する（-O で rustc 版 0.13 秒に対し 0.40 秒）。
- `src/link.rs`（リンカを探して直接起動する）と `src/testgen.rs`：C の `.o` を自分でリンクして実行し（2 回目は保存したリンク行で）、出力が rustc 版と一致する。`std::path` と `std::process::Command` を使う。
- `src/elfz.rs`（`.o` のセクションを SHF_COMPRESSED にする）：zstd クレートの代わりに同じ API の小さな RLE を付けて、C の `.o` を圧縮・解凍し、セクションとシンボルが rustc 版と一致する。
- `src/macros.rs`（`macro_rules!` の展開と `include_str!`）：proc_macro2 の代わりに使う分だけの字句解析器（`tests/selfhost/mini_pm2.rs`、トークン木・位置・Spacing）を付けて、probe / examples / src の 147 ファイルを展開し、結果が rustc 版と一致する。
- `src/parse.rs`（syn を使わない自前の構文解析）：probe / examples / src の 149 ファイル（parse.rs 自身も）を解析し、アイテムと節の数が rustc 版と一致する（-O で rustc 版 0.12 秒に対し 0.33 秒）。
- `src/ast_closure.rs`（自前の構文木の上のクロージャ解析。closure.rs の移植）：全ファイルの関数のクロージャの自由変数と「値でキャプチャするか」が rustc 版と一致する。
- `src/types.rs` + `src/ast_types.rs`（front.rs から分けた型の表現・intern と、自前の構文木の型 → TySyn）：全ファイルの型の変換結果と intern が rustc 版と一致する。
- `src/ast_summary.rs`（アイテムの要約。front.rs の summarize / parse_sig / クロージャの持ち上げの移植）：全ファイルの全アイテムの要約が rustc 版と一致する。
- `src/program.rs`（全体の表。front.rs から syn に依存しない部分を分けたもの：関数シグネチャ・struct / enum の定義とレイアウト・インスタンス化・trait 実装・const の評価）：要約までのモジュールと prelude を合わせた約 1 万 1 千行のプログラムで、probe / examples の 135 ファイルの Program を作り、関数・型（サイズ・フィールドのオフセット・128 bit の深いハッシュ）・impl・const（と `src/ast_consteval.rs` で評価した static の初期値のデータ）と、全関数の本体のパスを `fn_by_p` / `struct_by_p` / `variant_by_p` で引いた結果、`src/solver.rs`（型推論の単一化器）でジェネリックな関数を解いた結果、`src/ctors.rs` の書き換え、`src/ast_infer.rs`（型推論）でユーザの関数 552 個の let の型を推論した結果の 7 万 5 千行の出力が、probe / examples / src の 157 ファイルで rustc 版と一致する（-O でも）。
- `src/enc.rs`（キャッシュの直列化。`enc_struct!` マクロと trait の静的呼び出し `T::dec(r)`）と `src/fxhash.rs`（FxHasher / `FxMap`）：構造体・Vec・Option・タプルを符号化して戻し、バイト列とハッシュが rustc 版と一致する（-O でも）。

環境変数：`CRUSTC_DUMP_PRELUDE=1` は付けた prelude を stderr に出す。`CRUSTC_NO_STRIP=1` は出力のシンボルを残す（callgrind などで関数名を見るため）。
`CRUSTC_BENCH_LINKER` は criterion のベンチのリンカを固定する。

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
| `lsp/did_change_1fn` | LSP 応答性：1 関数編集の didChange から publishDiagnostics まで | 8.0 ms |
| `lsp/did_change_noop` | 内容が同じ didChange（保存など） | 0.26 ms |
| `cold/lsp_did_open` | 初めて開いたとき（全関数を検査） | 115 ms |
| `build/noop` | 無変更ビルド（リンク省略） | 12.7 ms |
| `build/edit_1fn` | 1 関数編集ビルド（リンク込み、wild） | 20.7 ms |
| `cold/build` | キャッシュなしのビルド (Cranelift) | 188 ms |
| `front/lex` | 字句解析だけ（no-op の下限） | 2.3 ms |

(2026-11-17、check を型検査で止めた後の値。driver2 に移した直後は 10.0 ms / 0.28 ms / 166 ms だった。
最初のドライバの check は型検査だけで 1 関数編集 3.9 ms / didOpen 41 ms だった)

（値はマシンの状態で変わる。比べるときは同じ時間帯に、ベースラインのコミットと並べて測る。`build/edit_1fn` はリンク（この環境では ld.lld）が大半を占める）

- 判定の閾値：5% 未満の変化は無視し（`noise_threshold`）、有意水準は 1% にしている。
  cold 系は実行ごとに ±10% ぶれるので、閾値を 15% にしている。
- サイズ予算は、バイナリ・`.o`・キャッシュ (driver2 + Cranelift。キャッシュは機械語を持つ) を、例ごとに上限バイトで持つ。
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
  - 正しさ（クロージャ・dyn の追加時。`tests/compare.sh` に「キャッシュなし」と「キャッシュからの再ビルド」の両方を比べる段を足して見つけた）
    - 同じ関数の中のクロージャは、struct の deep hash（＝親アイテムのハッシュ）とシグネチャがどれも同じになる。そのため `consume::<closure#8>` と `consume::<closure#9>` のキャッシュキーが衝突し、2 回目のビルドで別のクロージャの機械語が使われていた。キーにクロージャの名前を入れて直した。
    - vtable を zero-init で定義したため `.bss` に置かれ、リロケーションが適用されず null を呼んでいた。
  - 速度（クロージャ・dyn の追加時）
    - HEAD との A/B で、LSP の 1 関数編集に +4% が出た。型推論の呼び出しごとの `Vec` の clone と、struct のハッシュに名前を入れていたことが原因で、どちらも外した。
    - callgrind の命令数で見ると、1 関数編集（`Session::check`）は HEAD より少なくなった。didOpen（cold）は約 +2% で、criterion でも有意差はない。
  - 関数ポインタ・Box の追加時
    - `Ty` が 16 → 24 byte になっていた（前回の `Dyn(&'static str)` が fat pointer だったため）。`&'static String` にして 16 byte に戻した。
    - criterion では LSP の 1 関数編集が +14%、lexer が +10% と出た。ただ lexer のコードは変えていない。
    - callgrind で見ると、`lexer::St::tok` の `b"..".contains(&c)` が memchr になっていた。呼び出し回数は同じで、命令数だけ +32% 増えていた（静的な文字列の配置の違い）。
    - `matches!` に替えた結果、ベースラインより LSP の 1 関数編集 −5%、no-op ビルド −13%、lexer −6% になった。
  - Drop の追加時（`tests/compare.sh` に valgrind の段を足した：メモリエラーと確実なリークがあれば失敗）
    - `*b` の読み出しが `b` のムーブ扱いになり、Box がリークしていた。deref の中身はムーブせずに読むようにした。
    - `self.top = n.next`（Box の中身のフィールドだけのムーブ）で、`n` 全体をムーブ扱いにしていたため、ノードがリークしていた。
      ムーブしたフィールドを 0（null の Box / `None`）で埋め、持ち主は普通に drop するようにした。
    - 以前からあった不具合も 2 つ見つかった。
      - `&Rect { .. }` を `&'static` で返すと、スタック上の一時値を指していた。rustc と同じく定数式は静的データに昇格するようにした。
      - タプル構造体のフィールド `n.0` の型を推論していなかった。
    - Box も Drop も使わないプログラムは、drop の処理を一切生成しない（`Program::has_drop`）。速度とサイズ予算は変わらない。
  - Vec / String の追加時
    - criterion ではほぼ全項目が +20〜40% になったが、変えていない lexer も同じだけ遅く、マシンの負荷による揺れだった。
      同じ時間帯に HEAD と並べて測ると ±10% 程度の揺れの中。callgrind の命令数も HEAD 以下。
    - 本物の増加は prelude の選択（+26%）だけだった。prelude の名前が増え、先頭 1 byte ごとの候補表が長くなったのが原因。(先頭 1 byte, 長さ) で引く表にして HEAD より少なくした。
    - `==` は識別子として現れないので、prelude の `Vec::eq` / `String::eq` が付かず、ポインタを比べていた。eq は型と一緒に付け、到達可能性の根にも入れた。
  - Iterator の追加時
    - サイズ予算がほぼ全部の例で超過した（hello のキャッシュが 3.2 KB → 10.8 KB）。`for x in v.iter()`・変数名の `count`・`a.max(b)`・`Option::take`・フィールドの `.next` で、イテレータの部品が丸ごと付いていた。
      付ける条件を「イテレータを作る呼び出しの後ろにアダプタ・消費メソッドが続く」などに絞り、既存の例の予算は 1 byte も緩めずに通るようにした。
    - LSP の 1 関数編集で prelude の選択が +63%（候補の名前が増えたため）。アイテムのハッシュごとに結果を Session に覚え、変わっていないアイテムは走査しないようにした。callgrind で `Session::check` は HEAD より少ない。
    - 型推論が一時的に parse したマクロの引数の型をアドレスで記録していたため、解放後のアドレスに codegen が作ったノードが重なり、別の式の型（`&str`）を引いていた。一時ノードは記録しないようにした。
    - クロージャを 1 つ編集すると、それを受け取る map などのインスタンスが 65 個作り直されていた。クロージャの struct の deep hash から親アイテムのハッシュを外し、18 個（親とクロージャだけ）にした。
  - format! / HashMap の追加時
    - nostd_core のキャッシュが 8.7 KB → 22.7 KB になった。生ポインタの `p.write(v)` の `write` と `a.max(b)` で、書式化の部品と String / Vec が丸ごと付いていた。
      書式化のマクロ名は `name!` の形のときだけ数え、`__exr_str_cmp` も max / min では付けないようにした。
    - iterators は HashMap のイテレータ（`HmIter` など、`impl Iterator for` の側）が付いていた。これらも HashMap の名前があるときだけにした。
    - 書式化の helper（`__exr_fmt_int` など）はジェネリック `<G>` にして根から外した。呼ばれた分だけインスタンスとしてコンパイルされる。
      instance の本体から呼ばれる非ジェネリックな関数（`__exr_print` → `putchar`）は、codegen の各波の後に生きている集合を広げて拾う。
    - 予算を緩めたのは vec_string のキャッシュだけ（45.0 KB → 48.5 KB）。`name.to_string()` は受け手の型が選択の時点で分からず（整数なら Display が要る）、書式化の部品を付けるため。
    - callgrind: gen1000 のビルドは +0.15%、check は −4%。prelude を多く使う例は増えた（iterators +5%、vec_string +15%。付く prelude が増えた分）。
  - 大小比較（PartialOrd / Ord）の追加時
    - 比較の glue（`__exr_cmp`）を常に付けると、小さい例のキャッシュが 20 byte ずつ増えて予算を超えた。
      再帰する型は Box / Vec を通るので、glue はそれらを使うプログラムにだけ付け、ほかは codegen が比較をその場で展開するようにした。
    - `use core::cmp::{max, min}` の `cmp` と境界の `T: Ord` で、trait の宣言と `Ordering` が付いていた。`cmp::` はパスとして数えず、trait の宣言は手書きの `impl Ord for` があるときだけ付ける。既存の予算は 1 byte も緩めていない。
    - 途中で見つけた既存のバグ
      - ブロック末尾の式や match の arm の `println!` の一時値を文の終わりで drop していて、Cranelift の検証（値の支配関係）に落ちていた（`if c { println!(..) } else { .. }`）。
        末尾の式と arm の一時値は、その場で drop するようにした（Rust 2024 と同じ）。
      - `&&&str` の `len()` が 1 段しか辿らず、0 を返していた。
      - 比較の右辺の `None` / `Ordering::Less` を場所（ローカル変数）として読もうとしていた。
    - callgrind: gen1000 のビルドは −2.6%、check は −6.8%、prelude を使う例は +0.4〜1.5%。
  - マージソート・binary_search・clamp の追加時（`examples/sorting.rs`）
    - sort は 16 個ずつの挿入ソート + 下から上へのマージ（作業領域は realloc で n 要素分）。要素は `read` / `write` でビット移動するので Drop を持つ型でも二重解放しない（valgrind で確認）。
      20 万要素の `Vec<u64>` の sort が 56 秒 → 0.04 秒になった。
    - sort を使う例はサイズ予算を上げた（vec_string +3%、ordering +16% など）。マージソートの本体が挿入ソートより大きく、要素型と比較のクロージャごとにインスタンスができるため。
      vec_string のビルドも +9%（callgrind。コンパイルする関数が増えた分）。sort を使わないプログラムは変わらない（gen1000 は ±0%）。
    - 途中で見つけた既存のバグ
      - `&mut u64` / `&u64` を受け手にした整数のメソッド（`s.wrapping_mul(..)`）が、指す先ではなくアドレスを計算していた。
      - `std::cmp::Ordering::Less` のような 3 段以上のパスのバリアントを、2 段目の名前（`cmp`）で探していた。
      - 生ポインタの `==` / `!=` が使えなかった。
  - BinaryHeap・sort_by_cached_key の追加時
    - サイズ予算は既存の項目をどれも変えていない（BinaryHeap は名前があるときだけ付く）。
    - callgrind: prelude が大きくなった分、プロセスごとに 1 回の prelude の字句解析が +0.5M 命令（hello で +3.8%、約 0.1 ms）。gen1000 のビルドは ±0%。
      check の計測は 1000 関数で ±8% ほど揺れる（HEAD 同士でも 1.82G〜1.92G）ので、3 回ずつ測って差がないことを見た。
  - BinaryHeap の append / extend の追加時
    - 型推論が `Vec<T>::extend` の `Item = T` を `v.extend(w.iter())` の `&i32` と単一化して、`v` を `Vec<&i32>` にしていた。extend は引数の要素型（参照なら中身。`&str` は除く）から v の要素型を決めるようにした。
    - 途中で見つけた既存のバグ：`&&str` の `to_string()` の型を推論できず、`a.iter().map(|s| s.to_string())` のクロージャの戻り値が決まらなかった。
    - サイズ予算・callgrind（gen1000 / hello / vec_string / binary_heap）はどれも ±1% 以内。
  - よく使う機能をまとめて足した時（`tests/probe.sh`：`tests/probe/*.rs` の小さいプログラムを rustc と比べて通った数を数える。2 / 28 から始めた）
    - prelude が 4000 行近くになり、プロセスごとに 1 回の prelude の字句解析が hello のビルドの半分（8M 命令）を占めた。
      分割を build.rs に移し（`src/lexer.rs` をそのまま使う）、prelude の本体は `src/prelude_lib.rs.in` に置いた。hello は 14.9M → 9.3M 命令。
    - 文字列の helper・演算子の trait で、既存の例のサイズ予算が軒並み超えた（nostd_core のキャッシュ 8.7 KB → 29.7 KB など）。
      - 整数の `to_ascii_uppercase` や `Option::replace` と同じ名前の helper は、String か文字列リテラルの受け手があるときだけ付ける
      - 演算子の trait の宣言は手書きの `impl Add for ..` があるときだけ
      - String の新しいメソッドはジェネリック `<G>` にした（メソッド名だけで生きているとみなされ、Vec の `pop` などで String の `pop` までコンパイルされていた）
      - chars で回していた helper は UTF-8 を直接読み、イテレータの部品を付けない
      既存の予算は 1 byte も緩めていない。
  - 浮動小数点・i128・標準入出力・Rc/RefCell・`impl Iterator` の戻り値などを足した時（probe を 73 本に増やし、すべて rustc と一致）
    - 10 進変換は glibc の `ecvt_r` が桁を間違える場合があったので、1e9 進の多倍長で正確に行う。`examples/float_fuzz.rs`（6000 行）・`examples/int128.rs` の乱数の照合が rustc と一致する。
    - サイズ予算は、新しい prelude の型を「名前が出たときだけ付ける」（owner の gating）にして、ほとんどの項目を変えずに済ませた。上限を上げたのは iterators（+68 B）・text_more（+0.4%）・hashmap（+0.3%、Iterator の既定メソッドの `cycle` の分）だけ。
    - callgrind（命令数）：hello は作業前 9.96M → 9.95M（prelude の項目表を作るときの `Vec` の確保をやめて、増えた分を打ち消した）。gen1000 は 2.43G で、HEAD のビルド同士でも ±2% 揺れる範囲で差はない。
  - オーバーフロー検査の追加時
    - callgrind：gen1000 のビルドは 2.43G → 2.55G 命令（+5%）、`-O` は HEAD と同じ 3.06G。hello は 9.95M → 10.25M（main のハンドラ登録の分）。
      最初は検査ごとに panic 用の分岐とブロックを作っていて +50%（3.67G）だった。`trapnz` にし、定数の除数・シフト量の検査を省いた。
    - サイズ：検査の分だけコードが増える（gen1000 のバイナリ 443 KB → 498 KB、`-O` なら増えない）。rustc の debug ビルドと同じ性質の増加なので、予算は UPDATE_BUDGET=1 で取り直した。
    - 途中で見つけた既存のバグ：`?` で String のエラーを返すと二重解放、`{ return None; }` が `()` 型、enum の `==` が使っていないバリアントのポインタを辿る、`saturating_add` がベクトル用の命令、`struct Node { children: Vec<Node> }` が「無限の大きさ」、`Node<T>` が `Option<Box<Node<T>>>` を持つと具体化が止まらない、char のリテラルパターン、型推論の中で作り直した式のアドレスが codegen のノードと重なる、など。
    - `|a| a + 1` の戻り値を呼び先の境界 (`B`) で決めようとして、まだ決まっていない `B` を親の型推論より優先していた（`map(|w| w.parse().unwrap())`）。
  - panic のメッセージを種類ごとに出した時
    - 実行時のハンドラ（表引き・数の書き出し・メッセージ）が std のプログラムに必ず付く分、小さい例のサイズが +3〜6 KB（hello のバイナリ 6.0 KB → 10.0 KB）。
      最初は +6〜7 KB で、書式つき panic の関数が String / Vec を引き込んでいた（書式化を使うプログラムだけに付けるようにした）・
      sigaction の構造体を配列の初期化で作っていた（static にした）・メッセージを if の連鎖で選んでいた（繋げた文字列と位置の表にした）のを直した。
    - 表は最初 1 か所 16 byte で gen1000 のオブジェクトが +15%。オフセット 20 bit・メッセージ番号 13 bit・種類と行と列 31 bit を 1 語に詰めて +8%（キャッシュも同じ形で +5%）。予算は UPDATE_BUDGET=1 で取り直した。
    - callgrind：gen1000 のビルドは 2.54G → 2.57G（+1%）、`-O` は 3.06G → 3.11G。hello のキャッシュなしのビルドは 10.3M → 29M（ハンドラの関数の parse と codegen。約 4 ms）、キャッシュありの再ビルドは 2.8M → 5.6M。
    - 途中で見つけた既存のバグ：`__FmtError` が Debug を実装していなかった（`Result<_, fmt::Error>` の unwrap）。
  - 型の違う `==` を PartialEq の trait で解決した時
    - 名前の付け方：右辺の型が self 型と違う `impl PartialEq<R> for S` のメソッドは `S::<PartialEq<R>>::eq`（Display / Debug の `S::<Display>::fmt` と同じ）。`==` は lt と rt が違うときだけその候補を探すので、同じ型の比較は今までどおり。
    - 型推論：`==` の両辺が Vec と配列・スライスなら要素どうしを、数のリテラルのときだけ単一化する（`v == [1, 2]` で v: Vec<i64>）。`Vec<String> == Vec<&str>` は単一化しない。
    - サイズ：配列を比べるプログラムに impl の分が付き、キャッシュは +0.1〜1.7%（enums +256 B など）。callgrind：gen1000・hello ±0%、vec_string +2%（付く prelude の parse）。
  - assert_eq! の left / right の値を出した時
    - 書式化を使っていなかったプログラムに書式化の部品が付く（options のバイナリ 16.7 KB → 23.9 KB、ビルドは 57M → 125M 命令）。値を `{:?}` で出すのに要る分なので予算は取り直した。gen1000・hello は ±0%。
    - 途中で見つけた既存のバグ：
      - panic の場所の行がアイテムの中の行だった（2 番目以降の関数の行がずれる）。dbg! も同じだった。
      - prelude の名前を何も使わないプログラム（traits など）には panic のハンドラが付かず、panic が SIGILL（終了コード 132）だった。
      - `&None` を変数の借用として扱っていた。
  - イテレータ・Vec・clone の追加時
    - **これまで `#[derive(Clone)]` の型の `.clone()` は使えず、`Vec<String>` の clone と `cloned()` はビットごとのコピー（二重解放）だった**。clone の glue を足して直した。
    - 新しいアダプタ（Rev / Peekable / FlatMap など）の `impl Iterator` が、イテレータを使うプログラムに全部付いていた。その struct が要るときだけにした。
      書式化の Formatter の `fill` フィールドが `Vec::fill` を引いて clone 一式まで付いていたので、フィールド名を変えた。既存の予算は緩めていない。
    - `vec![..].join(..)` の一時値の Vec を drop していなかった（valgrind で見つけた）。
    - callgrind: gen1000 ±0%、hello +2.5%、vec_string / iterators / hashmap は +0.5〜1%。
  - マクロの引数の中のクロージャの追加時
    - 要約と codegen が `|` を含むマクロの引数だけを parse してクロージャを数える。callgrind は gen1000 +0.2%、iterators +0.7%、formatting ±0%。
    - 型推論は `vec![..]` の引数を parse し直したノードを推論の終わりまで持ち、クロージャの本体を呼び先の境界が分かるまで後回しにできるようにした（`vec![v.iter().map(|x| x * x).sum::<i32>(); 2]`）。

## 対応している言語サブセット（no_std の範囲）

当面のゴールは「no_std で扱う範囲が動くこと」。具体的には、次の条件を満たすプログラムを対象にしている。

- alloc を使わない。
- 書式化マクロ（`write!` / `format_args!`）を使わない。
- `#![no_std]` / `#![no_main]` / `#[panic_handler]` / `#[unsafe(no_mangle)] pub extern "C" fn main` を使ってよい。

検証は、そうしたプログラムを rustc でもビルドし、実行結果（標準出力と終了コード）が一致することで行う。

- rustc 側は libcore だけでビルドする（`-C panic=abort`）。
- 比較は `tests/compare.sh` が行う。対象は `examples/*.rs` の 20 本で、そのうち no_std は 6 本（`nostd_calc` は字句解析と後置記法の電卓で、ジェネリック struct・`?`・const 配列・`&&str` の自動 deref を使う）。

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
  - 引数位置の `impl Trait`（名前のない型パラメータとして扱う）
- クロージャ（`examples/closures.rs`）：
  - キャプチャ：参照（既定）と `move`（値）。入れ子のクロージャ、`self` やメソッド呼び出しを含む本体
  - `F: Fn(A) -> B` / `FnMut` / `FnOnce` 境界、`impl Fn(A) -> B` 引数、`&F` / `&mut F` 渡し
  - 戻り値位置の `impl Fn(A) -> B`（末尾が `move` クロージャで、キャプチャが引数だけのもの）
  - struct のフィールドに入れる（`struct Button<F: Fn(i64) -> i64> { on_click: F }`、`(self.on_click)(x)`）
  - 引数・戻り値の型は、注釈 → 呼び先の境界 → 使われ方（親の型推論）→ 本体の型推論の順に決める
- `dyn Trait`（`examples/dyn_trait.rs`）：
  - `&dyn Trait` / `&mut dyn Trait`。`&T` からの暗黙の変換（引数・let・フィールド・配列・戻り値）と `as &dyn Trait`
  - デフォルトメソッド、ジェネリックな impl（`impl<T> Tr for W<T>`）、プリミティブ型への impl を vtable 経由で呼ぶ
  - `&dyn Fn(A) -> B`（クロージャを vtable 経由で呼ぶ）
- 関数ポインタ（`examples/fn_ptr_box.rs`）：
  - `fn(A, B) -> R` 型。関数アイテムを値として使う（引数・戻り値・let・struct のフィールド・配列）、`(op.f)(a, b)`
  - `F: Fn(A) -> B` に関数を渡す（`apply(double, 5)`、`o.map(double)`）
  - ジェネリック関数は turbofish（`id::<i64>`）か期待型（`let f: fn(i64) -> i64 = id;`）で型引数を決める
  - `&dyn Fn` / `Box<dyn Fn>` にする（引数 3 個まで。prelude の中継関数 `__exr_fnptrN` が vtable に入る）
- `Box<T>`（`examples/fn_ptr_box.rs`）：
  - `Box::new(x)` は libc の `malloc` でヒープに置き、drop で `free` する
  - `Box<dyn Trait>` / `Box<dyn Fn(A) -> B>`、自動 deref、`*b`、`&Box<T>` → `&T`、`Option<Box<Node>>` のような再帰的な型
- `Vec<T>` / `String`（`examples/vec_string.rs`）：prelude に Rust で書いてある（`src/prelude.rs`）。libc の `realloc` / `free` を使い、Drop で要素を drop してから解放する
  - `Vec`：`new` / `with_capacity` / `push` / `pop` / `len` / `is_empty` / `capacity` / `get(_mut)` / `first` / `last` / `insert` / `remove` / `swap_remove` / `swap` / `reverse` / `truncate` / `clear` / `append` / `as_slice` / `as_mut_slice` / `extend_from_slice` / `clone`（`T: Copy`）/ `contains` / `==`（`T: PartialEq`）/ `sort`（`T: Ord`、挿入ソート）
  - `vec![a, b]` / `vec![x; n]`、添字 `v[i]`（範囲外は panic）、`&v[a..b]`、`for x in &v` / `v.iter_mut()` / `v`（値で回す）
  - `&Vec<T>` → `&[T]`、`&String` → `&str` の暗黙の変換（ジェネリックな `&[T]` 引数にも）
  - `String`：`new` / `from` / `with_capacity` / `push_str` / `push(char)`（UTF-8 に符号化）/ `len` / `is_empty` / `as_str` / `as_bytes` / `clear` / `truncate` / `clone` / `==`（`String` 同士と `&str`）、`"..".to_string()` / `to_owned()`
  - `&str` / `&[T]` の `==` は中身で比べる
  - Vec / String のメソッドは、型の名前がコードに現れたときだけ prelude から付ける（自分の struct に `push` があるだけでは付かない）
- Iterator（`examples/iterators.rs`）：prelude に `trait Iterator { type Item; fn next(..); .. }` とアダプタを Rust で書いてある
  - 関連型：`type Item = u32;`（impl）と `Self::Item` / `I::Item`（シグネチャ）、`I: Iterator<Item = T>` / `where Self: Iterator<Item = &T>` の制約で型パラメータを解く
  - 自前の `impl Iterator for S`、`for x in it`（`while let Some(x) = it.next()` にする）
  - 作るもの：`v.iter()` / `iter_mut()` / `into_iter()`（Vec・配列・スライス）、`(a..b)` / `(a..=b)` を値として、`s.chars()`（UTF-8 を復号）
  - アダプタ：`map` / `filter` / `filter_map` / `enumerate` / `zip` / `take` / `skip` / `take_while` / `chain` / `copied` / `cloned`
  - 消費：`collect`（`Vec<_>` / `String`、`collect::<Vec<_>>()` と `let v: Vec<_> = ..` の `_` も）/ `sum` / `product` / `count` / `fold` / `for_each` / `any` / `all` / `find` / `position` / `last` / `nth` / `max` / `min`
  - クロージャの引数の型は、呼び先の境界（`F: FnMut(Self::Item) -> B`）を関連型まで解いて決める。型推論も関連型を辿る（`let s: i64 = (1..=10).sum();` の範囲は i64）
  - prelude のイテレータ部品は、イテレータを作ってアダプタ・消費メソッドを呼ぶコード（`v.iter().map(` / `(0..n).sum(` / `impl Iterator` / `.chars()`）があるときだけ付ける。`for x in v.iter()` だけ、`Option::map` だけ、変数名の `count` だけでは付けない
- 書式化（`examples/formatting.rs`）：`format!` / `print!` / `println!` / `eprint!` / `eprintln!` / `write!` / `writeln!`（`String` と `fmt::Formatter` に）
  - `{}` / `{:?}` / `{:#?}`、位置・名前つき引数、`{x}` の取り込み、`{{` / `}}`
  - 幅・寄せ・埋め（`{:>8}` / `{:*^9}` / `{:08}` / `{:>w$}`）、`+`、`#`、精度（文字列の切り詰め）、`x` / `X` / `b` / `o`
  - `impl fmt::Display for S`（`fmt::Result` / `fmt::Formatter<'_>` / `f.pad`）と `x.to_string()`
  - `#[derive(Debug)]`（struct・タプル構造体・unit 構造体・enum・ジェネリック）。整数・bool・char・`&str`・String・Vec・配列・スライス・タプル・Option・Result・HashMap の Debug
  - 型ごとの Display / Debug の本体は codegen が作る（`__exr_display::<T>` / `__exr_debug::<T>`）。prelude は Rust で書いた helper だけ
- HashMap（`examples/hashmap.rs`、`examples/text_more.rs`）：prelude に Rust で書いた（オープンアドレス法・線形探索・FNV-1a）
  - `new` / `with_capacity` / `insert` / `get` / `get_mut` / `remove` / `contains_key` / `len` / `is_empty` / `clear` / `m[&k]`
  - `entry(k).or_insert(v)` / `or_insert_with` / `and_modify`
  - `iter` / `iter_mut` / `keys` / `values` / `values_mut` / `into_iter` / `for (k, v) in &m`、`collect::<HashMap<_, _>>()`
  - キーは整数・bool・char・`&str`・String と `#[derive(Hash)]` の struct / enum / タプル（ハッシュ関数は型ごとに codegen が作る）
  - 回る順番は rustc と違う（rustc もランダムなので、例ではキーを並べ替えてから出す）
- 文字列と char（`examples/strings_chars.rs`）：prelude の `__exr_str_<名前>` / `__exr_char_<名前>` があれば、
  `s.名前(..)` / `c.名前(..)` はそれを呼ぶ（型推論もそのシグネチャを使う。&str の引数には char も渡せる）
  - `to_uppercase` / `to_lowercase` / `replace` / `replacen` / `repeat` / `split("..")` / `split_once` / `strip_prefix` / `strip_suffix` /
    `trim_start` / `trim_end` / `rfind` / `char_indices` / `bytes` / `eq_ignore_ascii_case` / `parse::<整数>()`（`ParseIntError` の Display / Debug も std と同じ）
  - String の `insert` / `insert_str` / `remove` / `pop` / `retain` / `+` / `+=`、`&s[a..b]`（文字の境界でなければ panic）
  - char の `is_alphabetic` / `is_numeric` / `is_alphanumeric` / `is_whitespace` / `is_uppercase` / `is_lowercase` / `to_digit` / `is_digit` /
    `to_uppercase` / `to_lowercase` / `len_utf8`、`char::from_digit` / `char::from_u32` / `char::from(u8)`。
    Unicode の表は持たないので、ASCII・Latin-1・ギリシャ文字・キリル文字・仮名・漢字・ハングルの範囲で近似する
- イテレータの追加分（`examples/iter_vec_more.rs`）：`rev`（Vec・配列・範囲・chars・map / filter / copied / chain の後。DoubleEndedIterator の代わりに各型が `next_back` を持つ）/
  `step_by` / `peekable`（`peek` / `next_if`）/ `skip_while` / `scan` / `inspect` / `flat_map` / `flatten`（中身は Vec・Option・イテレータ）
- Vec / スライスの追加分：`retain` / `retain_mut` / `dedup` / `dedup_by_key` / `drain(範囲)` / `split_off` / `resize` / `fill` / `first_mut` / `last_mut` /
  `windows` / `chunks` / `to_vec` / `join` / `concat`（要素が `&str` か String）
- Option / Result の追加分：`expect` / `zip` / `map_or_else` / `or_else` / `and` / `get_or_insert_with` / `insert` / `is_none_or` / `is_ok_and` / `is_err_and` / `map_or`
- clone（`examples/clone_derive.rs`）：`#[derive(Clone)]` の struct / enum / ジェネリック、Box・タプル・配列・Option・Result、手書きの `impl Clone`。
  本体は型ごとに codegen が作る（`__exr_clone::<T>`。drop の要らないフィールドはコピー、ほかはフィールドごとに clone）
- Default（`examples/defaults.rs`）：`#[derive(Default)]`、手書きの `impl Default`、`Default::default()` / `T::default()` / `i32::default()` / `Vec::default()`、
  構造体の更新記法 `S { a: 1, ..Default::default() }` / `..base`、`unwrap_or_default`、`std::mem::take` / `swap` / `replace`（Copy でない型も）。
  本体は型ごとに codegen が作る（`__exr_default::<T>`: 0 で埋め、手書きの impl を持つフィールドはそれを呼ぶ）
- コレクション（`examples/collections.rs`）：`HashSet`（HashMap<T, ()> の上に。和・積・差・collect）、`BTreeMap` / `BTreeSet`（キーの順に並べた Vec と二分探索。
  回る順・`entry().or_insert` / `or_default`・`first_key_value`・`pop_first`・`m[&k]` は std と同じ）、`VecDeque`（リングバッファ。`d[i]`・両端の push / pop）。
  キーが `&str` / String の map の `get("k")` は std の Borrow と同じく `&str` で引ける
- `impl Index` / `IndexMut`：`x[i]` は x が `index` / `index_mut` を持つ struct なら `*x.index_mut(i)` にする
- 関連定数（`examples/assoc_consts.rs`）：`impl S { const N: usize = 4; }` / trait の `const K: u32;`（既定値も）。`S::N` / `Self::N` / `T::N`（型引数）、配列の長さにも
- モジュール（`examples/modules.rs`）：1 ファイルの中の `mod m { .. }`（入れ子・`pub(crate)`・`use self:: / super:: / crate::`・`m::f()` / `m::S` / `m::N`）。
  字句解析の前に `mod m {` と対応する `}` を空白にして平らにし（位置と行は変えない）、名前解決はパスの先頭のモジュール名を読み飛ばす。
  `#[cfg(test)] mod tests { .. }` は消す
- `macro_rules!`（`examples/macros.rs`）：字句解析の前にテキストとして展開する（展開は 1 行に収め、後ろの行番号を変えない）。
  断片 expr / ident / ty / literal / tt / block など、繰り返し `$(..),*` / `$(..)+` / `$(..)?`、再帰、アイテムを作るマクロ。
  式の断片は `( .. )` で包んで埋め込むので `square!(1 + 2)` の優先順位は Rust と同じ
- 演算子のオーバーロード：`impl Add / Sub / Mul / Div / Rem / Neg / Not / BitAnd / BitOr / BitXor / AddAssign ..`。
  `a + b` は左辺が struct で `add` メソッドがあれば `a.add(b)` にする（`impl Mul<i32> for V` のような右辺の型も）
- `&str` のメソッド：`split(char)` / `split_whitespace` / `lines` / `trim*` / `starts_with` / `ends_with` / `contains` / `find`（引数は `&str` か `char`）、`<` などの比較と `sort`
- 大小比較（`examples/ordering.rs`、`examples/ordering_impl.rs`、`examples/ordering_inline.rs`）：
  - `<` / `<=` / `>` / `>=` をタプル・配列・スライス・`&str`・String・Vec・Option・Result と `#[derive(PartialOrd, Ord)]` の struct / enum に（辞書順。enum はバリアントの判別値の順）
  - 手書きの `impl PartialOrd` / `impl Ord`（`impl PartialEq` / `impl Eq for S {}` も）。`<` と sort はそれを呼ぶ
    - sort / sort_unstable / max / min / clamp / BinaryHeap / BTreeMap / BTreeSet / Reverse / 境界 `T: Ord` / `T: Eq`、タプルや Option の中、enum への impl も
    - 配列・スライス・Vec・参照の `==` の要素も手書きの eq を呼ぶ
    - `BTreeSet` / `BTreeMap` の collect は std と同じく、等しいものが続いたら後のものを残す（insert は前のキーを残す）
  - `cmp` / `partial_cmp`（整数にも）、`Ordering` と `reverse` / `then` / `then_with` / `is_lt` など、`std::cmp::Reverse`
  - `max` / `min`（Ord な struct・タプル・String にも）、`std::cmp::max` / `min`
  - `sort` / `sort_by` / `sort_by_key` / `sort_by_cached_key` / `sort_unstable*`（安定なマージソート。Vec・配列・スライス・`v[a..b]`）、`binary_search` / `binary_search_by` / `binary_search_by_key`（同じ値が並ぶときに返す位置も std と同じ）、`clamp`（整数・Ord な型。`lo > hi` なら panic）、イテレータの `max` / `min` / `max_by_key` / `min_by_key` / `max_by` / `min_by`
  - `BinaryHeap`（`examples/binary_heap.rs`）：`new` / `with_capacity` / `from(vec)` / `collect` / `push` / `pop` / `peek` / `len` / `is_empty` / `clear` / `iter` / `into_iter` / `into_vec` / `into_sorted_vec` / `{:?}`。
    sift の手順を std と同じにしてあるので、`iter` / `into_vec` / `{:?}` の並びも rustc と一致する。`Reverse` で最小ヒープ
  - `BinaryHeap::append` / `extend`（`examples/binary_heap_extend.rs`）：std と同じ `rebuild_tail`（足す数が多ければ全体を作り直し、少なければ 1 つずつ sift_up。2048 要素を超えたら別の閾値）なので、足した後の並びも一致する
  - `Vec::extend`。引数は Vec・配列・`&v` / `&[..]`・範囲・イテレータ（IntoIterator の代わりに codegen が `into_iter()` / `iter()` を補う。要素の参照なら Copy として足す）
  - 比較の本体は型ごとに codegen が作る（`__exr_cmp::<T>`。Box / Vec を使わないプログラムではその場で展開する）
- Drop（`examples/drop.rs`、`examples/drop_more.rs`。drop の順序を rustc と比べ、valgrind でリーク・二重解放がないことを確かめる）：
  - `impl Drop for S`（ジェネリックな impl も）、`drop(x)`
  - スコープの終わりで宣言と逆順に drop する。対象は return / `?` / break / continue で抜けるスコープ、値を捨てる式文、`let _ = e`、代入で上書きされる古い値
  - ムーブを追跡する：値として読む・値で受ける `self`・`move` キャプチャ・値で束縛するパターン（`if let Some(b) = o`）・`for x in arr`
  - 一時値（`make().area()`、`&Box::new(..)`、match 対象）は文の終わりで drop する。let の初期化式の一時値はブロックの終わりまで延ばす
  - フィールドだけのムーブ（`self.top = n.next`）は、元のフィールドを 0 で埋めて持ち主を drop する
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
    - ラベル付きの `break 'outer` / `continue 'outer`、`break v`（`loop` の値）、ラベル付きブロック `'a: { .. break 'a v; .. }`
    - `let PAT = e else { return; };`
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
    - `assert_eq!` / `assert_ne!` は `==` / `!=` として比べる（Option・Vec・String と `&str` なども）
    - マクロの引数の中のクロージャも使える（`println!("{:?}", v.iter().map(|x| x * 2).collect::<Vec<_>>())`、`examples/closures_in_macros.rs`）。
      クロージャは最初の `|` の位置で識別する（codegen がマクロの引数を parse し直してもノードの対応が取れる）
- パターン：
  - `_`、束縛（`ref` / `mut`）、`x @ p`、リテラル、範囲、`|`（`A(x) | B(x)` のように束縛してもよい）
  - スライスパターン `[a, b]` / `[first, .., last]` / `[x, rest @ ..]`（配列・スライス・`v[..]`）
  - バリアント、struct、タプル、`&p`、入れ子、ガード
  - 参照を match したときのデフォルト束縛モード
  - `None` などの unit バリアント
- メソッド：
  - 自動 deref と自動参照、UFCS（`S::m(&x)` / `Trait::m(&x)` / `<S as Trait>::m(&x)`）
  - 静的ディスパッチ。固有メソッド → trait 実装 → デフォルトメソッドの順に探す
- 浮動小数点 `f32` / `f64`：
  - 四則演算・`%`・比較・`as` キャスト（整数への変換は rustc と同じ飽和）、`1.5` / `1e-3` / `2.0f32` リテラル
  - メソッド：`sqrt` / `abs` / `floor` / `ceil` / `round` / `trunc` / `powi` / `powf` / `exp` / `ln` / `sin` / `cos` / `atan2` / `hypot` / `mul_add` / `max` / `min`（NaN を無視）/ `total_cmp` / `is_nan` / `to_bits` など（超越関数は libm）
  - 定数：`f64::MAX` / `EPSILON` / `NAN` / `INFINITY` / `consts::PI` など。`f64::sqrt(x)` のパス呼び出し
  - 書式化は rustc と同じ桁：最短表現（往復で同じ値になる最短の桁、`{:?}` は指数が -5 以下か 16 以上で `1e-7` 形式）、`{:.N}`（正確な 10 進展開から偶数丸め）。10 進変換は 1e9 進の多倍長で正確に行う
  - `"1.5".parse::<f64>()`（strtod、`ParseFloatError`）
  - `examples/float_fuzz.rs` の 6000 行（乱数のビット列の Display / Debug / `{:.N}` / parse）が rustc と一致する
- 128 bit 整数 `i128` / `u128`：
  - 算術・比較・シフト・ビット演算は Cranelift の I128（引数と戻り値は `enable_llvm_abi_extensions` で SysV と同じ 2 レジスタ）
  - 除算・剰余、浮動小数点との変換（丸めは 1 回）、書式化（`{}` / `{:x}` / `{:b}` / 幅など）、`parse` は prelude の関数
  - `checked_*` / `wrapping_*` / `pow` / `count_ones` / `MAX` / `MIN`、HashMap のキー、derive
  - `examples/int128.rs` の乱数による照合（3000 組の四則演算・シフト・比較）が rustc と一致する
- そのほか：
  - 整数の `overflowing_*` / `checked_pow` / `saturating_pow` / `rem_euclid` / `div_euclid` / `next_power_of_two` / `leading_ones` / `isqrt` / `from_str_radix`
  - `i32::pow(2, 10)` のような、プリミティブのメソッドを関数の形で呼ぶもの
  - `From` の impl と `.into()`（期待型の `from`。String は `String::from`、数値はキャスト）
  - `vec![v; n]` の要素が Copy でないもの（clone して並べる）、`Vec<Vec<T>>` の `concat`、`collect::<String>()` の要素が `&str` / `String`
  - `partition` / `rposition`、`collect::<Option<Vec<T>>>()` / `collect::<Result<Vec<T>, E>>()`、メソッドの turbofish の型推論
  - `String::from_utf8` / `std::str::from_utf8`（検査と `Utf8Error` の表示は std と同じ）、`trim_matches` 系（パターンは &str / char / クロージャ）、`split` / `lines` / `split_whitespace` の `rev()`
  - `Rc`（`clone` / `strong_count` / `ptr_eq`。弱参照はない）、`RefCell`（`borrow` / `borrow_mut` は参照を返すだけで、借用の検査はしない）、`Cell`
  - メソッドが見つからないときの `deref` 先での検索（`rc.borrow_mut()`）。`struct Node { children: Vec<Node> }` や `Vec<Rc<RefCell<Node>>>` のような再帰型
  - 書式の `{:e}` / `{:E}`（精度つきも）、幅・精度を引数から取る `{:w$}` / `{:.p$}` / `{:.*}`、`{:12e}` のような幅と型の組み合わせ
  - `println!` などの書式マクロの引数の中の型推論（parse し直したノードの型はソース上の位置で codegen に渡す）
  - 終わりのない範囲 `(1..)`、`char::is_ascii_digit` などを関数の値として渡すこと
  - `use E::*;` で取り込んだバリアントを短い名前で使うこと、`Box` を通した再帰型の derive(PartialEq)（比較の glue 関数を作る）
  - `try_into`（整数どうし。範囲外は `TryFromIntError`）、`i64::from(x)`、char の範囲 `'a'..='e'`
  - `BTreeMap::range`（`a..b` / `a..=b` / `a..`）、`HashMap` の `entry(..).or_default()`
  - `for x in (0..n).step_by(k)` のようにアダプタを通した for のループ変数の型推論
  - `Vec` の `rotate_left` / `rotate_right` / `split_at`、`Iterator::eq`、`std::cmp::Reverse(x)` のような `std::` 付きのパスでの構築
  - ローカル変数をキャプチャして返すクロージャ（`let mut c = 0; move || { c += 1; c }`。型は注釈かリテラル・String の初期化式から）
  - `?` でのエラーの `From` 変換（`impl From<ParseIntError> for AppError`）
  - 関数の中に書いたアイテム（`fn` / `struct` / `enum` / `const` / `use`。字句レベルでファイルの末尾に移す）
  - `std::collections::HashMap::new()` のような長いパス、`VecDeque::from(vec)`、スライスの `split_at` / `split_at_mut`、`Chars::as_str`
  - 標準入出力：`io::stdin()` の `read_line` / `lines()` / `read_to_string` / `lock()`、`io::stdout()` / `lock()` / `flush()`、`BufWriter::new(..)`、`write!` / `writeln!` を `write_str` を持つ任意の型に（libc の stdio を使うので `print!` と順序が混ざらない）
  - `std::process::exit`、`dbg!`、`for i in 0..`（終わりのない範囲）、`char_indices()` / `bytes()` の `rev()`
  - クロージャを含む型を返すクロージャ（`flat_map(|i| (0..i).map(move |j| (i, j)))`、`|a| move |b| a + b`）: 型推論がクロージャの struct 型（親の型引数・キャプチャ・引数・戻り値）まで解く
  - `Iterator::cycle`、`Box<T>` の `as_ref()` / `as_mut()`
  - 書式化のビルダー `f.debug_struct(..)` / `debug_tuple` / `debug_list` / `debug_set` / `debug_map`、`f.alternate()` / `width()` / `precision()` / `pad()`
  - `HashMap::from([..])`（HashSet / BTreeMap / BTreeSet / VecDeque / BinaryHeap も）、HashMap の `retain` / `drain` / `extend`、HashSet / BTreeMap / BTreeSet / String の `extend`、値としての `[a, b].into_iter()`
  - `Ord::cmp(&a, &b)` / `PartialOrd::partial_cmp(..)` の UFCS
  - char のリテラル・範囲のパターン（`'a'..='z'`）、文字列リテラルのパターン（`match s { "add" => .. }`）
  - 引数のパターン（`fn index(&self, (i, j): (usize, usize))`）、参照への演算子の impl（`impl Add for &M`）
  - `Iterator` を受ける引数への `&Vec` / スライス / 配列（`a.iter().zip(&b)`）、範囲の添字を受け手にしたメソッド（`v[a..b].iter()`）
  - ユーザの `impl IntoIterator for &Coll`（`for x in &c`）、型パラメータ以外を縛る where 節（`where C::Item: Debug` / `where Self: Sized`。読み飛ばす）
  - `Box<dyn Error>`（`dyn Error + Send + Sync` も。変換した時点で `{}` / `{:?}` の文字列にした prelude の型で表す。`?` / `Box::new(e)` / `"msg".into()` で変換）、`impl std::error::Error for E {}`、`fn main() -> Result<(), E>`（Err なら `Error: {:?}` を stderr に出して終了コード 1）
  - 自分を `Option<Box<..>>` で含むジェネリックな型（`Node<T> { next: Option<Box<Node<T>>> }`）
  - ブランケット impl（`impl<T: Display> Describe for T`。境界は検査せず、メソッドが見つからないときにどの型でも探す）
  - `-> impl Iterator<Item = T>` などクロージャ以外の `impl Trait` の戻り値（プログラムを組み立てた直後に本体を型推論して具体型を入れる。呼び出し側のキャッシュのキーには解いた型が入る）
  - `const fn` を const の初期化式から呼ぶこと、範囲パターンの端に const（`i32::MIN..=-1`）、`?Sized` 境界
  - const ジェネリクス `const N: usize`（関数・impl・struct の型パラメータ）
    - 配列の長さ `[T; N]`、値としての `N`（`for i in 0..N`、`[0; N]`、`N * 10`）
    - 引数 `f::<3>()` / `Buf<4>` / `Mat::<C, R>::zero()` / const の名前 `zeros::<K>()`、引数の配列からの推論（`sum([1, 2, 3])`）
    - 型引数の位置には長さを持つ配列型 `[(); n]` を入れ、インスタンスの中の `N` はそこから読む（ジェネリックな型の長さは `CONST_LEN_BASE + i`）。式の長さ（`[T; N * 2]`）は未対応（rustc の安定版も同じ）
  - `Type::<A>::f()` の型のセグメントの型引数（`Vec::<i32>::new()` も）
  - 型の違う `==` / `!=` は trait で解決する：右辺の型を持つ `impl PartialEq<R> for S`（手書きのものも）を、両辺の型（片方の参照を外したものも）で選んで `eq` を呼ぶ
    - prelude に std と同じ組み合わせ：`Vec<T>` と `[U; N]` / `&[U; N]` / `&[U]` / `[U]` / `Vec<U>`、`[T]` と `Vec<U>` / `[U; N]`、`[T; N]` と `[U]` / `&[U]` / `[U; N]`（要素の型も違ってよい：`Vec<String> == ["a", "b"]`）
    - impl の self 型に配列・スライス（`impl .. for [T; N]` / `for [T]`）
    - `a[..2] == [1, 2]`（サイズ不定のスライスの比較は参照で比べる）
  - 型の違う `<` / `<=` / `>` / `>=` も同じく `impl PartialOrd<R> for S` の `partial_cmp` で（`Meters < Feet`、`Ver < (1, 5)`、`Ver < 2`）
    - 右辺がリテラル・タプル・配列なら、左の型の impl のうち形の合う R を期待型にする（`(1, 5)` を `(u32, u32)` に）。`==` も同じ
    - `partial_cmp` が `None` なら 4 つともすべて false（rustc と同じ。同じ型の手書きの partial_cmp も）
    - f64 / f32 の `partial_cmp` は NaN で `None`
- panic：
  - `#[panic_handler]` があればそれを呼ぶ（範囲外アクセス・`unwrap`・`assert!`・`panic!`・オーバーフロー検査など）
  - なければ trap する。std のプログラム（`no_std` でないもの）では main の最初に trap（SIGILL）のシグナルハンドラを登録し、
    rustc の panic と同じく stdout を書き出して終了コード 101 で終わる
  - stderr には rustc と同じ形で、場所と種類ごとのメッセージを出す（`thread 'main' panicked at src/main.rs:行:列:` の次の行。ファイル名は `src/main.rs` 固定）
    - オーバーフロー・0 除算・`MIN / -1`・シフト量、`panic!` / `unreachable!` / `todo!` / `unimplemented!`（書式つきも）、`assert!`（式の文字列か独自のメッセージ）、`assert_eq!` / `assert_ne!`（`left` / `right` の値を `{:?}` で。独自のメッセージも）
    - 範囲外の添字は `index out of bounds: the len is N but the index is I`
    - `Option` / `Result` の `unwrap` / `expect` は呼んだ側の場所（`#[track_caller]` 相当）。`Result::unwrap` の `Err` の値は、書式化を使うプログラムでだけ `{:?}` で出す
  - `assert_eq!(a, b)` は `let l = &(a); let r = &(b); if !(*l == *r) { panic }` に展開し、a と b を 1 回だけ評価する。
    右の型は左から決める（`assert_eq!(o, None)`）。左が数のリテラルなら右を先に評価して左の型を右から決める（`assert_eq!(2, n)`）。
    値を書式化するので、assert_eq! を使う std のプログラムには書式化の部品が付く
  - 場所の行はアイテムの開始行を足したファイルの中の行。場所を書き込んだ関数のキャッシュは開始行を覚えておき、上に行を足して行がずれたらコンパイルし直す
  - しくみ：trap ごとに別の trap コードを付け、codegen が「関数の中のオフセット → 種類・行・列・メッセージ」を記録する。
    driver がそれを static の表 `__EXR_TRAP_TABLE`（1 か所 8 byte）にし、SIGILL のハンドラ（`SA_SIGINFO`）が発生したアドレスで引く。
    検査そのものは `trapnz` のままなので、検査の速さは変わらない。範囲外の添字だけは長さと添字を渡すため、関数ごとに 1 つの冷たいブロックから `__exr_panic_index` を呼ぶ
- オーバーフロー検査（rustc の debug ビルドと同じ。`-O` なら入れない）：
  - `+` / `-` / `*`（と `+=` など）、単項の `-`、`abs`、`pow`、`<<` / `>>` の量が型の幅以上
  - 0 除算と `MIN / -1`（rustc と同じく `-O` でも検査する）。除数・シフト量が定数なら省く
  - `sum` / `product`（std と同じく呼び出し側の設定に従う: prelude の `#[rustc_inherit_overflow_checks]`）
  - prelude の中は検査しない（ハッシュなどで折り返す演算を書いている）。128 bit の掛け算は検査しない
  - 検査は分岐でなく条件つき trap（`trapnz`）にした。分岐とブロックで panic の場所を渡すと gen1000 のコンパイルが +50%、`trapnz` なら +5%

### core の代わり（組み込みの prelude、`src/prelude.rs`）

- `Option<T>`：`is_some` / `is_none` / `unwrap` / `unwrap_or` / `ok_or` / `or` / `xor` / `take` / `replace` / `as_ref` / `as_mut` / `copied` / `cloned`
  - クロージャを取るもの：`map` / `and_then` / `unwrap_or_else` / `map_or` / `is_some_and` / `filter` / `ok_or_else`
- `Result<T, E>`：`is_ok` / `is_err` / `unwrap` / `unwrap_err` / `unwrap_or` / `ok` / `err`
  - クロージャを取るもの：`map` / `map_err` / `and_then` / `unwrap_or_else`
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
- 借用検査・可変性検査・ムーブ検査・match の網羅性検査はしない。
  - rustc を通るプログラムなら意味は変わらない前提で、struct は常にコピーしている。
  - どの arm にも合わなければ trap する。
  - オーバーフローは wrap する（rustc の debug ビルドなら panic する）。

### 未対応

- `Box<[T]>`、`Box::into_raw` などの Box の関数（`new` 以外）
- `String + &str` / `Vec` のスライスメソッドの一部（prelude に書いたもの以外。`&v` を `&[T]` にすれば組み込みのスライスメソッドは使える）
- イテレータの制限：`DoubleEndedIterator` / `ExactSizeIterator` の trait（`rev` は next_back を持つ prelude の型だけ）、`collect` は Vec / String / HashMap / HashSet / BTreeMap / `Option<Vec<T>>` / `Result<Vec<T>, E>` など prelude にあるもの
- 関連型の制限：trait の関連型は `Self::X` / `T::X` / `<T as Tr>::X` の形だけ。関連型に境界（`type Item: Copy;`）は付けられない
- 書式化の制限：`{:p}`、`format_args!`、`Formatter` の `fill()` / `align()`
- 比較の制限：`partial_cmp` は浮動小数点の NaN 以外では常に `Some`。BinaryHeap の `drain` / `retain` / `peek_mut`。一般の `IntoIterator` 境界はない（`extend` は codegen が引数をイテレータにする）
- HashMap の制限：キーに `impl Hash` を手書きすること
- Drop の制限
  - panic では drop しない（unwind しない）
  - `impl Drop` を持つ型のフィールドの一部だけをパターンでムーブすると、残りのフィールドはリークする（`let Pair { a, .. } = p;` の `b`）
  - クロージャの本体から `move` キャプチャした値をムーブすること（FnOnce として消費する使い方）はエラーにする
  - `Box` の中身のフィールドをムーブすると（`let n = boxed; n.val`）、Box の領域はリークする
  - 一時値の寿命は rustc の規則の近似（let の初期化式の一時値は常にブロックの終わりまで延びる）
- クロージャの制限
  - Fn / FnMut / FnOnce を区別しない（`FnMut` のクロージャを `&dyn Fn` にしてもエラーにならない）
  - ローカル変数をキャプチャするクロージャを `impl Fn` で返すとき、型が注釈やリテラルから分からない変数（`let v = Vec::new();` など）
- `dyn` の制限：`Self` やジェネリクスを使うメソッド（object safe でないもの）は dyn 経由で呼べない。`dyn A + B` は不可
- 別ファイルのモジュール（`mod x;`）。1 ファイルの中のモジュールは平らにするので、別のモジュールに同じ名前のアイテムがあるとぶつかる
- 関数の中のアイテムは末尾に移すので、ほかの関数の中の同名のアイテムとぶつかる。そのエラーの行番号は移した先のもの
- `macro_rules!` の制限：衛生 (hygiene) はない（展開はテキストの置き換え）。`$x:expr` などは次の区切りまでのトークン列として取る近似
- ジェネリックな関数の `-> impl Trait` の戻り値
- 参照の Vec を `iter().flatten()` すること（`into_iter().flatten()` は可）、型パラメータのある trait（`From<T>` は名前だけ持ち、同じ型への `From` の impl は 1 つまで）
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
- **クロージャ**（`src/closure.rs`）
  - 要約の段階で、本体の k 番目（前順）のクロージャを次の 2 つに持ち上げる（lambda lifting）。
    - struct `親::{closure#k}<親の型引数, __C.., __P.., __R>`。フィールドはキャプチャで、参照キャプチャなら `*mut T`、`move` なら `T`。
    - fn `親::{closure#k}::call(__env: *mut その struct, 引数..) -> __R`。
  - キャプチャ・引数・戻り値の型がすべて struct の型引数に入る。そのためクロージャの型だけで本体のインスタンスが決まり、既存の単相化とインスタンス単位のキャッシュにそのまま乗る。
  - 本体は親のアイテムを parse し直し、k 番目を取り出して普通の fn にしてコンパイルする。キャプチャは `__env` のフィールドとしてスコープに入る。
  - 呼ばれないクロージャも、作った時点でインスタンスを要求する（本体のエラーを報告するため。未使用なら `--gc-sections` で消える）。
- **関数ポインタ**：関数のアドレス 1 語。`func_addr` で取り、`call_indirect` で呼ぶ（ABI は普通の呼び出しと同じ）。
- **`Box<T>`**：ヒープへのポインタ（`Box<dyn ..>` は fat pointer）。参照と同じ経路で自動 deref・dyn への変換をする。
- **drop**（`codegen.rs` の Drop 節）
  - drop が要る値を持つローカル変数・一時値は、1 byte の drop フラグと一緒に所有者として登録する。ムーブでフラグを 0 にし、スコープの出口でフラグが 1 のものを逆順に drop する（分岐でムーブするかどうかが変わっても正しい）。
  - drop 本体は prelude の `__exr_drop_in_place::<T>` のインスタンス（drop glue）。codegen が型ごとに 1 段だけ展開した本体を作り（`impl Drop` の呼び出し → フィールド / バリアント / 要素 → Box なら free）、入れ子の型は呼び出しにするので、再帰的な型でも展開が止まる。
  - vtable の最後の欄は中身の型の drop glue。`Box<dyn Trait>` の drop はここを呼んでから free する。
  - drop glue はインスタンス単位でキャッシュされる。`impl Drop` の有無は deep hash に入れてあり、後から足し引きしても、その型を使う関数が作り直される。
- **`&dyn Trait`**：(データのポインタ, vtable のアドレス) の 16 byte。
  - vtable は trait のメソッドの宣言順に並べた関数アドレスの列で、Blob に「関数 ID の列」として持つ。
  - キャッシュには関数名（インスタンスなら元の関数名と型引数）で保存する。
  - `.o` では内容で重複を除いた読み取り専用データにする。
  - `&dyn Fn(A) -> B` の vtable は、クロージャ本体のインスタンス 1 個だけ。

## アーキテクチャ

今の主経路 (driver2):

```
src.rs ──srcmap (mod x; のファイル)──▶ macros (macro_rules!) ──▶ モジュールを平らに ──▶ lexer: アイテム + ハッシュ
   ──▶ 要約 (キャッシュ、fork で並列) ──▶ Program (シグネチャ・レイアウト) ──▶ 到達する関数
   ──▶ 関数ごと: キャッシュ? ── miss ──▶ ast_infer (型推論) → acg (自前の構文木から minclif の命令の列。-O で小さい関数を展開)
                                        → Cranelift (crustc-clif) か minclif (レジスタ割り当て) で機械語 → Blob (fork で並列)
   ──▶ minclif の ELF 書き出しで .o (panic の場所の表を含む) ──▶ wild でリンク (変わらなければしない)
```

| 場所 | 役割 |
|---|---|
| `crates/syntax/src/lexer.rs` | トップレベルのアイテム分割と、正規化トークン列のハッシュ。`impl` はヘッダとメンバに分ける |
| `crates/syntax/src/macros.rs` / `srcmap.rs` | macro_rules! の展開・include_str!、`mod x;` のファイルの読み込みと行の対応 |
| `crates/syntax/src/parse.rs` / `ast_*.rs` | 自前の構文木・要約・型推論・クロージャ解析・定数評価 |
| `crates/sema/src/program.rs` | 全体の表 (シグネチャ・レイアウト・trait 実装・インスタンス) |
| `crates/typeck/src/acg/` | 構文木から命令の列 (crustc_ir::Recorder) への 1 パスの lowering と型検査。-O で小さい関数をインライン展開 |
| `crates/core/src/driver2.rs` / `cache2.rs` | `Session2`：キャッシュ (ディスク + メモリ)、build / check / ワークスペース、fork の並列化、.o の書き出し |
| `crates/codegen/src/link.rs` | `cc -###` からリンク行を取り出して保存し、リンカを直接起動する |
| `crates/minclif` | 命令の記録・レジスタ割り当て・x86-64 の機械語・ELF の .o |
| `crates/clif` | 記録した命令の列 → Cranelift の IR → 機械語 |
| `crates/lsp/src/lib.rs` | LSP サーバ (ドキュメントごとに `Session2` を持ち `check` の診断を返す。ワークスペースと std の索引) |
| `crates/ide/src/navigate.rs` / `symbols.rs` | 定義・参照・hover・補完の名前解決と、シンボルの一覧 |
| `src/main.rs` | CLI (build / check / ワークスペースの build) |

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

## 結果（ここから下は作業の記録。数値はそれぞれの時点のもの。最新は上の「現状」）

環境は Linux x86-64 のクラウドコンテナ（4 コア相当）。rustc 1.97.0、wild 0.10.0、Cranelift 0.137。
`bench/gen.py` が生成する「N 個の関数 + 相互呼び出し」のプログラムで測った。どのツールの出力も rustc の実行結果と一致した。

### N = 1000 関数（320KB）

| tool | cold ms | no-op ms | 1 関数編集 ms | peak RSS (cold) | binary | cache / incr dir |
|---|---:|---:|---:|---:|---:|---:|
| crustc（wild を直接起動） | 128 | 11.7 | **19.0** | **13.4 MiB** | 413 KiB | **498 KiB** |
| crustc（bfd を直接起動） | 129 | 12.1 | 33.2 | 13.2 MiB | 422 KiB | 498 KiB |
| crustc（lld を直接起動） | 141 | 12.0 | 36.7 | 58.7 MiB | 412 KiB | 498 KiB |
| rustc `-Cincremental -Cdebuginfo=0` | 887 | 217 | 461 | 164 MiB | 5013 KiB | 22151 KiB |
| rustc `-Cincremental -Cdebuginfo=2` | 1015 | 218 | 563 | 183 MiB | 5557 KiB | 24592 KiB |
| rustc `-O`（参考、非 incremental） | 12445 | - | - | 147 MiB | 382 KiB | - |

### N = 5000 関数（1.6MB）

| tool | cold ms | no-op ms | 1 関数編集 ms | peak RSS (cold) | binary | cache / incr dir |
|---|---:|---:|---:|---:|---:|---:|
| crustc（wild を直接起動） | 497 | 63.6 | **87.0** | **34.4 MiB** | 2030 KiB | **2479 KiB** |
| crustc（bfd を直接起動） | 471 | 53.9 | 87.0 | 34.6 MiB | 2038 KiB | 2479 KiB |
| rustc `-Cincremental -Cdebuginfo=0` | 3810 | 756 | 1911 | 499 MiB | 8040 KiB | 113644 KiB |

rustc と比べると、1 関数編集は 20〜24 倍速く、cold の RSS は 1/12〜1/14、キャッシュは 1/45 になった。
5000 関数の no-op が 60ms 前後あるのは、ファイル全体の lex、`.o` の作り直し、キャッシュの読み書きが N に比例するため。
マルチファイル化してファイル単位の要約にすれば、ここも O(変更量) にできる。

1 関数を編集したときの crustc の内訳（`--time`、N=1000）：

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

hello（`examples/hello.rs`）の出力サイズは、crustc で 6.0 KiB、rustc の debug（strip あり）で 333 KiB。
rustc 側はほとんどが std 由来。

### rustc との比較（2026-11-15 時点。オーバーフロー検査・panic のメッセージなどを足した後）

wild は入っていない環境で、リンカは自動選択（ld.lld）。

ビルド時間（`python3 bench/run.py 1000`、N = 1000 関数）：

| tool | cold ms | no-op ms | 1 関数編集 ms | peak RSS (cold) | binary | cache / incr dir |
|---|---:|---:|---:|---:|---:|---:|
| crustc | 167 | 20 | **44** | **59 MiB** | 553 KiB | **642 KiB** |
| rustc `-Cincremental -Cdebuginfo=0` | 962 | 230 | 433 | 163 MiB | 5013 KiB | 22151 KiB |
| rustc `-O`（非 incremental） | 11604 | - | - | 147 MiB | 382 KiB | - |

1 関数編集は rustc の incremental の約 10 倍速い。上の表（wild、検査を足す前）より crustc が遅いのは、リンカの違い（wild 5ms / ld.lld 17ms）と、検査と panic の表の分。

生成したコードの速さ（`python3 bench/runtime.py`、`bench/rt/*.rs`。ビルドはキャッシュなし、ビルド・実行とも 3 回の中央値。どれも出力は rustc と一致）：

| プログラム | crustc | crustc `-O` | rustc (debug) | rustc `-O` | ビルド ms（crustc / rustc debug / rustc -O） |
|---|---:|---:|---:|---:|---|
| fib（再帰 fib(35)） | 86 ms（3.1x） | 68 ms（2.5x） | 97 ms（3.5x） | 27 ms | 44 / 106 / 127 |
| hashmap（100 万回の entry） | 212 ms（1.9x） | 144 ms（1.3x） | 723 ms（6.5x） | 112 ms | 55 / 162 / 289 |
| matmul（200×200 を 5 回） | 225 ms（5.2x） | 94 ms（2.2x） | 657 ms（15x） | 43 ms | 67 / 168 / 268 |
| nbody（5 体を 100 万ステップ） | 747 ms（19x） | 203 ms（5.1x） | 1388 ms（35x） | 40 ms | 53 / 119 / 191 |
| → nbody（下の改善の後） | 437 ms（8.2x） | **91 ms（1.7x）** | 1423 ms | 53 ms | 64 / 129 / 183 |
| sieve（2000 万までの篩） | 373 ms（4.0x） | 195 ms（2.1x） | 467 ms（5.0x） | 94 ms | 40 / 117 / 155 |
| sort（200 万要素の sort ほか） | 1178 ms（6.0x） | 892 ms（4.6x） | 2161 ms（11x） | 195 ms | 58 / 142 / 314 |
| → sort（下の改善の後） | 782 ms（3.8x） | **611 ms（2.9x）** | 2102 ms | 209 ms | 58 / 153 / 311 |
| strings（format! / split / parse） | 257 ms（6.3x） | 176 ms（4.3x） | 127 ms（3.1x） | 41 ms | 61 / 125 / 203 |

（括弧は rustc `-O` に対する倍率）

- crustc の debug ビルドは、strings を除いて rustc の debug ビルドより速い（検査は同じく入っている）。rustc の debug は std のジェネリックなコード（イテレータ・HashMap など）を最適化なしで実体化するため遅い。
- crustc `-O` は rustc `-O` の 1.3〜5 倍。差が大きいのは、浮動小数点の配列を添字で回す nbody（LLVM はループと範囲検査を消してベクトル化する）、マージソートの sort、書式化の strings。
- ビルドは crustc が rustc debug の 2〜3 倍、rustc `-O` の 3〜5 倍速い（キャッシュなし。インクリメンタルなら上の表のとおり差はもっと開く）。
- nbody と sort の改善（同じマシンでも 2 割ほどぶれるので、倍率はその時の rustc `-O` に対するもの）：
  - `for k in 0..C`（C は定数）のループ変数で長さ C 以上の配列を引くときは範囲の検査を省く（debug でも）
  - 同じブロックの中で、同じ添字と長さの検査は 1 回だけ（debug でも）
  - `-O`：回数が定数で 4 回までのループは展開する（k が定数になり、`d[k]` がアドレスの定数に畳まれる）
  - `-O`：添字の検査の panic のブロックを場所ごとに作り、場所の定数をそこで作る（熱い道で長さ・添字・行・列をブロックの引数としてレジスタに並べない）
  - enumerate でない for の番号を数えない（ループで引き継ぐ変数が 1 つ減る）
  - sort / sort_unstable は比較をクロージャの呼び出しにせず `<` で直接（整数なら命令 1 つ）。マージは分岐を使わずに選ぶ
  - sort_by_key はキーの関数を直接呼ぶ（比較のクロージャを挟まない）。binary_search も `<=` / `==` で直接比べる
  - 残りの差：sort_by_key のキーの関数の呼び出し（LLVM はインライン展開する）、sort のアルゴリズム（std は pdqsort / driftsort）

バイナリサイズ（KiB）：

| プログラム | crustc | crustc `-O` | rustc (debug) | rustc (debug, strip) | rustc `-O` (strip) |
|---|---:|---:|---:|---:|---:|
| fib | 15.4 | 10.9 | 4259 | 336 | 335 |
| hashmap | 24.1 | 16.1 | 4324 | 373 | 348 |
| matmul | 41.4 | 24.3 | 4330 | 378 | 361 |
| nbody | 40.4 | 20.9 | 4295 | 366 | 364 |
| sieve | 18.2 | 12.1 | 4267 | 340 | 335 |
| sort | 29.1 | 17.8 | 4332 | 385 | 348 |
| strings | 30.4 | 19.9 | 4293 | 357 | 340 |

#### インクリメンタルビルドのディスク使用量

- キャッシュ（`.crcache`）は関数ごとの機械語を持たない。機械語は出力の `.o` に入っているので、キャッシュには（長さ, ハッシュ）だけを書き、
  次のビルドで前回の `.o` の symtab を辿って同じ機械語を取り出す（ハッシュが合わなければ・`.o` がなければコンパイルし直す）。
  どこからも呼ばれず `.o` に置かなかった関数だけは機械語ごとキャッシュに書く。
- 残す `.o` の `.text` は ELF 標準の圧縮セクション（`SHF_COMPRESSED`、zstd レベル 1）にする（`src/elfz.rs`）。
  - wild は入力のどのセクションでも `SHF_COMPRESSED` を解凍して読むので、圧縮した `.o` をそのまま渡す。
  - ld.lld 18 は `SHF_ALLOC` の圧縮セクションでクラッシュし、ld.bfd は「reloc against ...: error 4」で失敗する（gABI では圧縮は ALLOC でないセクション用）。
    この 2 つには、圧縮しない `.o` を一時ファイル（`<out>.tmp.o`、リンク後に消す）で渡す。
- `.o` は 1 つの `.text` にまとめ、置く関数は main / export からリロケーションと vtable を辿って決める（`--gc-sections` の代わり）。

gen1000（ソース 310 KB）での変化：

| | 最初 | 機械語をキャッシュから外した後 | `.text` を圧縮した後 | 索引と panic の表を詰めた後 |
|---|---:|---:|---:|---:|
| `.crcache` | 642 KiB | 116 KiB | 116 KiB | **38 KiB** |
| `.o` | 757 KiB | 631 KiB | 153 KiB | **67 KiB** |
| 合計 | 1399 KiB | 747 KiB | 270 KiB | **105 KiB**（rustc の incremental ディレクトリは 22156 KiB） |
| バイナリ | 553 KiB | 553 KiB | 553 KiB | 519 KiB |

- キャッシュのファイルは本体を zstd で圧縮し、キーは下位 64 bit・長さは可変長で書く（索引は 1 件 20 byte → 9 byte 前後。圧縮で要約や panic の場所も 1/3 になる）。
- panic の表は可変長整数のバイト列にし、参照するのは `.text` の先頭だけにした（関数の位置は `.o` を作るときに自分で数え、symtab と合っているか確かめる）。
  前は 1 関数あたりリロケーション 24 byte + 見出し 16 byte、1 か所あたり 8 byte だった。表はバイナリにも載るので、バイナリも 6% 小さくなった。
  ハンドラ（`__exr_on_trap`）は panic のときに列を前から読む。
- `.o` は `.text` だけでなく、メモリに載る PROGBITS（panic の表・文字列）も圧縮する（wild はどれも解凍して読む）。

`python3 bench/run.py 1000`（wild 0.10.0 を PATH に入れて）：

| tool | cold ms | no-op ms | 1 関数編集 ms | peak RSS (cold) | binary | cache + .o |
|---|---:|---:|---:|---:|---:|---:|
| crustc（wild） | 124 | 17 | **28** | 19.5 MiB | 554 KiB | **270 KiB** |
| crustc（ld.lld、圧縮しない一時 `.o`） | 151 | 18 | 41 | 59.0 MiB | 553 KiB | 270 KiB |

- 圧縮は 587 KB → 69 KB で 0.3 ms（`--time` の `compress`）。wild の解凍込みのリンクは 6〜9 ms。

#### 本物の rustc との比較（`python3 bench/vs_rustc.py`）

同じソースを同じ手順（cold → no-op → 1 か所編集）でビルドし、残るディスク・バイナリ（strip 後）・実行時間を並べる。全結果は `bench/vs_rustc_result.md`。
crustc のリンカは wild 0.10.0、rustc 1.97.0。どの設定でも出力は rustc `-O` と一致した。

| プログラム | | cold ms | edit ms | disk KiB | binary KiB | run ms |
|---|---|---:|---:|---:|---:|---:|
| gen1000 | crustc | **118** | **28** | **105** | 521 | 12 |
| | rustc -Cincremental | 827 | 417 | 22156 | 1043 | 9 |
| | rustc -O | 11448 | - | - | 382 | 2 |
| nbody | crustc -O | **47** | **6** | **29** | **23** | 92 |
| | rustc -Cincremental | 149 | 106 | 2227 | 366 | 1231 |
| | rustc -O | 177 | - | - | 364 | 62 |
| sort | crustc -O | **51** | **8** | **32** | **30** | **195** |
| | rustc -Cincremental | 203 | 128 | 3102 | 385 | 2041 |
| | rustc -O | 317 | - | - | 348 | 184 |

- 編集後の再ビルドは rustc の incremental の 15〜20 倍速く、残すディスクは 1/75〜1/210、バイナリは 1/10〜1/20（gen1000 は rustc `-O` より大きい: debug の検査と最適化なしの分）。
- 実行時間は crustc（debug）が rustc（debug）より速いものが多い（gen1000 を除く）。crustc `-O` は rustc `-O` の 1.0〜2.6 倍（sort は同じ、strings は 2.2 倍）。

#### strings と sort の `-O` を速くした時

| | 前 | 後 | rustc -O |
|---|---:|---:|---:|
| strings `-O` | 141 ms | **73 ms** | 34 ms |
| sort `-O` | 550 ms | **195 ms** | 190 ms |
| strings（debug） | 259 ms | 113 ms | （rustc debug 121 ms） |
| sort（debug） | 725 ms | 252 ms | （rustc debug 1983 ms） |

- 書式化（callgrind で整数の書式化が 41%）：
  - `{}` の整数（10 進・幅と符号の指定なし）は、桁を後ろから詰めてまとめて足す（一時の String と 1 文字ずつの push をやめた。10 の割り算は掛け算になる）
  - `format!` の String は、決まった部分の長さ + 引数 1 つあたり 8 byte を先に確保する
  - `Vec::truncate` は drop の要らない要素なら長さを変えるだけ（`core::mem::needs_drop` を足した）。`extend_from_slice` は memcpy
  - 整数の定数で埋める配列（`[0u8; 24]`）は 8 byte ずつ書き、アラインメントの小さい集約型の写しも 8 byte ずつ（前は 1 byte ずつ）
  - `split` の next はフィールドをローカルに写してから回す
- sort：
  - 64 bit 以下の整数の `sort` と、キーが整数の `sort_by_key` は、安定な LSD 基数ソート（全要素で同じバイトの桁は飛ばす）。
    sort_by_key のキーの関数は要素ごとに 1 回だけ呼ぶ（std は比較ごとに呼ぶので、呼ばれる回数は std と違う）。
    （キーを 1 回だけ計算してから添字や (キー, 添字) を並べる方法も試したが、並べ替えのランダムな読み書きで遅くなった）
  - prelude の型ごとの分岐（`if __exr_int_kind::<T>() == 0` / `needs_drop::<T>()`）は、型で決まるので使わない側をコンパイルしない（インスタンスも作らない）
- サイズ：整数を sort するプログラムは基数ソートの分が付く（sorting のバイナリ +42%、ordering のキャッシュ +49%）。書式化を使うプログラムも少し増える。予算は取り直した。
- キャッシュに残ったもの：panic の場所の表（1 か所 4〜5 byte）、リロケーション、要約（21 KB）、エントリの索引（1 件 20 byte、約 40 KB）。整数は可変長で、オフセットと行は前の値との差で書く。

crustc のバイナリはシンボルを持たない（strip しても同じ大きさ）。rustc は std を静的にリンクするので、strip しても 335 KiB 以上ある。crustc は使う部分の prelude だけをコンパイルし、libc を動的にリンクするので、rustc の 1/10〜1/30 になる。

#### matmul と sieve の `-O`、ジェネリクスと単相化

| | 前 | 後 | rustc -O |
|---|---:|---:|---:|
| matmul `-O` | 77 ms | **53 ms** | 31 ms |
| sieve `-O` | 252 ms | **144 ms** | 92 ms |
| nbody `-O` | 92 ms | **69 ms** | 42 ms |

- ループの前に出す（`-O`）：for / while の本体（と条件）で変わらない添字の受け手の先頭と長さを、ループの前で読む。
  - 対象はローカルの Vec と `&Vec` / `&mut Vec` の引数（`c` / `b`）と、ループで変わらない変数で引いた行（`c[i]` / `b[k]`）。
    本体で受け手が `x[..]..` の形（読むか、要素がスカラーの代入）でしか現れず、メソッドの受け手・`&mut`・束縛し直し・マクロの中・クロージャがないこと。
  - 行の読み出しはループの前で行い、範囲外なら読む場所をダミーの領域にする（`select`）。`i < c.len()` の検査は本体の中で、前で比べた値を使って行う（panic の順序は変わらない）。
  - matmul の内側のループは 30 命令 → 12 命令（`c` と `c[i]` の読み直しが消えた）。
- `vec![x; n]` は push の呼び出しをやめ、確保した領域に直接書く（1 byte の要素は memset）。sieve の初期化の 2000 万回の push が消えた。
- `for (i, x) in v.iter().enumerate()` は組を作らずに i と x を束縛する（スタックへの書き出しと読み戻しが消えた。sieve の命令数 615M → 535M）。
- 同じ機械語になるインスタンスは 1 つだけ置く（ICF：`Vec<u32>::push` と `Vec<i32>::push` など）。機械語・リロケーション（呼び先は代表に直す）・データ・vtable・panic の場所・呼び出し規約が同じものを、変わらなくなるまで繰り返して畳む。
  キャッシュは機械語を（長さ, ハッシュ）で `.o` から引くので、畳んだインスタンスも代表の機械語でヒットする。examples 全体でバイナリ −3%（binary_heap −13%）、`.o` −3%。
- drop の要らない型の `__exr_drop_in_place::<T>` / `drop(x)` は呼ばない（インスタンスも作らない。collections でインスタンス 353 → 341）。

#### std の範囲へ（セルフホストの第 1 段階: crustc 自身の lexer）

crustc 自身の `src/lexer.rs` を 1 ファイルにまとめて（`tests/selfhost/build_lexer.py`）crustc でコンパイルし、足りないものを順に足した。
- `std::hash`：`Hash` / `Hasher`（`write_u8` 〜 `write_usize`・`write_str`・`write_length_prefix` の既定のメソッドも）と、整数・bool・char・str・String・スライス・Vec の `impl Hash`、
  `DefaultHasher`（std と同じ SipHash-1-3・鍵 0 で、値も std と一致する）
- str とスライスへの trait 実装（`impl Hash for str` / `impl<T: Hash> Hash for [T]`）のメソッド呼び出し（組み込みのメソッドより先に探す）
- 型の別名 `type Key = u128;` / `type Res<T> = Result<T, String>;`
- `std::fs::read_to_string` / `read` / `write`、`std::env::args` / `var`、`std::io::Error`（Display / Debug は std と同じ形: `Os { code: 2, kind: NotFound, message: ".." }`）、`std::io::Result<T>`
- `String::from_utf8_lossy`（std と同じく壊れた列ごとに U+FFFD）、`bool::then` / `then_some`、整数の `to_le_bytes` / `to_be_bytes` / `to_ne_bytes` / `from_le_bytes` など、u8 の `is_ascii_alphanumeric` など
- バイト列のパターン（`match w { b"pub" | b"fn" => .. }`、w: &[u8]）、`&[u8] == b"pub"`、`&&str` を `&str` の引数に（deref の強制変換）
- 関数の中で定義した `macro_rules!`
- 本体がキャプチャを値として使うクロージャ（`changed.then(|| String::from_utf8(out))`）は、rustc と同じく値でキャプチャする（FnOnce）。値で持ったキャプチャからのムーブは、環境の側を 0 にする
- prelude の新しい項目（Hash / Hasher / Args など）は、その名前が要るときだけ付ける（`fn write` / `len` などのよくある名前では引かない）

#### セルフホストの第 2 段階: enc.rs + fxhash.rs

- trait の静的呼び出し：`Enc::dec(r)?`・`T::dec(r)`（Self は期待される型・型推論から）、ジェネリックな impl（`impl<T: Enc> Enc for Vec<T>` / `impl<A, B> Enc for (A, B)`）への静的ディスパッチと単相化
- `Trait::m(&x, ..)` / `crate::m::Trait::m(..)`（self を取るメソッド）はメソッド呼び出しとして
- スライス → 配列の `try_into`（`u64::from_le_bytes(b[..8].try_into().unwrap())`、`TryFromSliceError`）、`get(a..b)` / `get_mut(a..b)`（Vec・配列・スライス）
- `S::default()` / `<S>::default()` / `m::S::default()` / ジェネリックな別名 `FxMap::default()`
- `HashMap<K, V, BuildHasherDefault<H>>` / `HashSet<K, S>`（ハッシャの型引数は受け付けて、既定のハッシュを使う。反復順は std と違う）
- `macro_rules!`：`$t:ty { .. }` のように後ろに括弧が続くフラグメント、`$crate`、`crate::m!(..)` の呼び出し
- ユーザ定義の trait をタプルに実装（`impl<A, B> Tr for (A, B)`）

#### セルフホストの第 3 段階: prelude.rs

- `include_str!("..")` / `include_bytes!("..")`（入力ファイルからの相対パス。中身はエスケープして 1 行のリテラルにする）
- `std::sync::OnceLock`（`get_or_init` / `get` / `set`）。mut でない static も書き込める所に置く（内部可変性）
- `str::rsplit` / `rsplit_once` / `match_indices`、クロージャのパターンの `starts_with` / `ends_with`、`Iterator::find_map`
- 要素が整数などでないスライスの `contains`（`NAMES.contains(&w)`、PartialEq で比べる）
- 参照の列の `flatten`（`map.get(k)` の `&Vec<T>` などを `&T` で順に）
- const のパターン（`Some(BOX_FREE) | Some(DROP_GLUE)`、初期値がリテラルの const）
- `&s[a..b]`（s: &str / String）の型推論、`matches!` の中のメソッドの連鎖（parse し直した式の型を位置で引く）
- 修正：`a || f(&format!(..))` の右辺の一時値の drop フラグが、右辺を通らないとき初期化されていなかった（入口で 0 にする）

#### セルフホストの第 4 段階: link.rs + testgen.rs

- `std::path`：`Path`（= str）/ `PathBuf` / `OsStr` / `OsString`（= String）。`join` / `push` / `file_name` / `file_stem` / `extension` / `parent` /
  `with_extension` / `set_extension` / `display` / `to_string_lossy` / `is_file` / `is_dir` / `exists`（stat）
- `std::process::Command`：`new` / `arg` / `args` / `output` / `status`（fork + execvp。exec の失敗は close-on-exec の pipe で受け取り `Err` にする。
  stdout と stderr は poll で両方読む）、`ExitStatus`（`success` / `code` / Display）
- `env::var_os` / `env::split_paths`、`fs::remove_file`
- 同じ型への複数の `impl From<A> for T` / `impl From<B> for T`（呼び出し側が最初の引数の型で選ぶ）
- Deref の先が str の型のメソッド（`path_buf.join(..)` → str の trait のメソッド）、`os_string.push("..")`
- 書式文字列に直接書いた名前（`format!("{cands:?}")`）をクロージャのキャプチャとして数える
- 修正：`&&str` を受け手にした str の trait メソッドが 1 段余計に読んでいた

#### セルフホストの第 5 段階: elfz.rs

- `std::borrow::Cow`（`Cow<[T]>` / `Cow<str>`：`Borrowed` / `Owned`、`into_owned`、Deref）
- スライス・配列・Vec の `copy_from_slice` / `clone_from_slice`

#### セルフホストの第 6 段階: macros.rs

- struct と同じ名前のバリアント（`TokenTree::Punct(Punct { .. })`：裸の名前は struct を優先）
- `&a == &b`（a, b が struct などの参照どうし）、`impl PartialEq` と `impl PartialEq<str>` の両方
- `Option::flatten`、HashMap の `clone`（以前は浅いコピーで二重解放になっていた）、`String::replace_range` / `into_bytes`
- 修正：`None` などの大文字の名前のパターンをクロージャのキャプチャ候補に数えていた
- 修正：`include_str!` の展開が、同じ行の 2 つ目以降のコメント・文字列の中のものも展開していた

#### セルフホストの第 7 段階: 自前の構文解析 (syn から離れる土台)

- `src/parse.rs`：外部クレートなしの Rust の字句・構文解析（アイテム・型・パターン・式・文。優先順位、`if` の頭の struct リテラル、
  `>>` とジェネリクス、let チェーン、ラベル、クロージャ、マクロの呼び出しはトークン列のまま）。crustc がコンパイルできる書き方で書いてある
- `tests/parse.rs`：probe / examples / src の全ファイルで、節の種類ごとの数 (22 種) が syn で数えたものと一致する
- 後で代入する `let x;`（0 で埋めて置く）
- 修正：非ジェネリックな enum のフィールドの `Option<T>` が、T の大きさが決まる前に配置されてフィールドが重なっていた
  （再帰する構文木で起きる。インスタンスを取り直した後、非ジェネリックな定義も取り直す）
- 修正：ユーザの `struct Path` が prelude の `type Path = str` に負けていた

#### セルフホストの第 8 段階: クロージャ解析を自前の構文木へ移す

- `src/ast_closure.rs`：closure.rs（syn の構文木）と同じ解析（本体の中のクロージャを前順に、マクロの引数の中も・自由変数・
  キャプチャを値として使うか）を src/parse.rs の構文木の上で行う。`tests/ast_closure.rs` が全ファイルの 1273 個のクロージャで syn 版と一致することを確かめる
- 修正：値として渡す変数と代入する変数が混ざったクロージャ（`|| { r = cnt(caps); }`）は全部を値でキャプチャしていたので、外の `r` が変わらなかった
  （代入するキャプチャがあれば参照で持つ）
- 修正：型推論で `&Vec<T>` を `&[T]` の引数に渡すと参照どうしを同じ型にしていたので、クロージャがキャプチャした `&Vec<T>` が `&[T]` として扱われた
- 修正：`matches!(x, p if guard)` の guard の中の変数をキャプチャに数えていなかった
- `&Box<T>` / `&&T` を `&dyn FnMut(&T)` の引数に渡す（deref の強制変換）

#### セルフホストの第 9 段階: front.rs の型の部分を分ける・モジュールの名前空間

- `src/types.rs`：front.rs から syn に依存しない部分（`Ty` / `TySyn` / `ArrLen` / intern）を分けた（front は `pub use` で見せる）
- `src/ast_types.rs`：front.rs の `parse_tysyn`（syn の型 → TySyn）を自前の構文木に移植。`tests/ast_types.rs` が全ファイルの 3914 個の型で syn 版と一致することを確かめる
  （`dyn` の trait は proc_macro2 と同じ空け方のテキストにする）
- モジュールの名前空間：別々のモジュールで同じ名前（`types::Ty` と `parse::Ty`）を定義したら、平らにする前にモジュールの中の定義を `m__Ty` にし、
  中の参照・`use crate::m::{Ty as X}`（別名も）・`m::Ty` / `crate::m::Ty` / `super::Ty` のパスを直す（enum のバリアント名・フィールドは変えない）。Debug の表示は元の名前
- 関数の中の同じ名前の static（`intern` ごとの `static TABLE`）は、外に出す前に関数ごとに名前を変える
- `std::sync::Mutex`、`Box::leak`、`std::ptr::eq`、`Result::as_ref`、`HashSet<&T>::get(&t)`（`&T` を `&&T` に）、生ポインタのハッシュ、`&&str == &&str`
- 修正：参照を通して再帰する型（`enum T { R(&'static T) }`）の `==` でコンパイラがスタックを使い切っていた（glue 関数で比べる）
- 修正：型推論で `&String` を `&str` の引数に渡すと参照どうしを同じ型にしていた（ガードつきの腕で `t.len()` が壊れた値を読んだ）
- 修正：`v == b"crate"`（v は後で型の決まる `x.to_vec()`）で v をバイト列の型にしていた
- 修正：`String` の `contains` などの書き換えで作ったノードに型の記録がなく、メソッドが見つからなかった
- クロージャの変数の呼び出し `tok(k)` の戻り値の型を `peek` で引く

#### セルフホストの第 10 段階: アイテムの要約を自前の構文木へ移す

- `src/ast_summary.rs`：front.rs の `summarize` / `parse_sig` / `lift_closures` / `parse_and_summarize`（syn の構文木 → 要約）を自前の構文木に移植。
  `tests/ast_summary.rs` が driver と同じ前処理の後の全 2199 アイテムで syn 版と同じ要約になることを確かめる
  （const の初期値のテキスト・境界のテキストは proc_macro2 の表示と同じ空け方）
- 要約の型（`Summary` / `SigSyn` / `VariantSyn` / `ClosureSyn` / `Owner` など）も types.rs へ
- モジュールの中の `use a::{X as Y}` の別名（名前がぶつからないときも）
- 修正：`let nn = ..; if let Some(nn) = nn { v.push(nn) }`（match 対象と同じ名前の束縛でムーブ）で外側の変数をムーブ済みにせず、二重に解放していた
- `if` / ブロックの式の型を `peek` で引く（`if c { "a" } else { "b" }.to_string()`）

#### セルフホストの第 11 段階: Program の表を syn から分ける

- `src/program.rs`：front.rs の `Program` / `FnSig` / `StructDef` / `AdtTable` / `Insts` と、`build`（要約 → 表）・レイアウト・
  インスタンス化・`implements`・`resolve_with` などを移した。syn の構文木を引く検索（`fn_by_path` / `variant_by_path` / `eval_int` など）は
  front.rs の別の `impl Program` に残る。const の評価（配列の長さ）は自前の構文解析（`parse::parse_expr`）で行う
- `src/key.rs`：キャッシュキーの `KeyHasher` を cache.rs から分けた（cache.rs は codegen に依存するため）
- 足したもの：`thread_local!`（遅延初期化する `static mut` と `with` を持つ unit struct に展開）、unit struct の static、
  モジュールの中の static のパス（`m::S`）、`use crate::m::*` で取り込んだ名前の改名、`&T` → `*const T`、
  `&mut v[i]`（`Vec<Box<T>>`）→ `&mut T`、MutexGuard など Deref の型の `*g` / `g[i]`、`Range::contains`、`next_multiple_of`、
  u128 / i128 の掛け算のオーバーフロー検査（Cranelift に `umul_overflow.i128` がないので 64 bit ずつ組み立てる）、
  タプル・Option の `Hash::hash`（std と同じ順に書き込むので DefaultHasher の値も一致する）、`it.chain([(&a, &b)])`、
  `&&u32 == &&u32`、`print!` 系の型 ()
- 修正：
  - HashMap のキーの Vec を（要素でなく）バッファのアドレスでハッシュしていた（`HashMap<(u32, Vec<Ty>), u32>` が引けない）
  - `*b`（b: Box）から中身をムーブしても b を drop して中身を二重に解放していた
  - `it.zip(slice)` の要素の型を推論せず、`sn == "__array"` で `&String` を `&str` と取り違えていた（推論でも `.iter()` にする）

#### セルフホストの第 12 段階: 定数評価を自前の構文木へ移す

- `src/ast_consteval.rs`：consteval.rs（static の初期値 → データのバイト列とリロケーション）を自前の構文木に移植し、
  driver の static の評価はこちらを使う（失敗したときだけ、場所つきのメッセージを syn 版で作る）。
  文字列・バイト文字列のリテラルの中身（エスケープ・raw 文字列）も自前で読む。
  `tests/ast_consteval.rs` が probe / examples の全 696 個の const / static で syn 版と同じバイト列・リロケーションになることを確かめる
- 足したもの：`&Box<T>` → `&T` の強制変換（`e = x;` で x: `&Box<Expr>`。推論でも参照どうしをつながず中身を単一化する）

#### セルフホストの第 13 段階: パスの検索を syn から分ける

- `program.rs` の `PPath`（セグメントの名前と型引数の有無）の上に、モジュールを外す・struct / enum のバリアント / 関数をパスで引く処理を移した。
  front.rs の syn 版（`fn_by_path` など）は syn のパスを `PPath` にして呼ぶだけ。自前の構文木のパスからも `PPath::of` で引ける
- 修正：関数の中のアイテムを外に出すとき、トップレベルの同じ名前（`fn walk` が 2 つ）とぶつかるものも改名する

#### セルフホストの第 14 段階: 型推論の単一化器を分ける

- `src/solver.rs`：infer.rs から、構文木に依存しない単一化器（型変数・形 `Shape`・union-find・関連型と境界の後回し・
  `known_g` / `apply` / `resolve` など 900 行余り）を分けた。infer.rs の `Inf` は syn に固有の部分だけを持ち、`Deref` で `Solver` を使う。
  境界の関連型の束縛（`Iterator<Item = T>`）も自前の構文解析で読む
- `src/ctors.rs`：値として使ったタプルのバリアント / タプル struct（`.map(Some)` / `.map(Ty::Struct)` / `.map(W)` / `let f = E::V;`）を
  クロージャ（`|__c0| Some(__c0)`）に書き換える（字句の上で、enum / struct の定義から要素数を読む。行番号は変えない）
- 足したもの：`None::<u32>`（型引数を書いたバリアント）、ユーザの `impl Index` の結果の型の推論、配列の `as_slice()`
- 修正：
  - `move` でないクロージャのキャプチャを、クロージャ全体で「値か参照か」を決めていた。Copy の値（`f(p)` の p）を渡すだけで
    Vec のキャプチャまで値で持ち、二重に解放していた。キャプチャごとに決める（値として渡す・返すものだけ値で持つ）
  - const ジェネリクスの仮の長さで配列のサイズの掛け算があふれていた（デバッグビルドで trap）

#### セルフホストの第 15 段階: 型推論を自前の構文木へ移す

- `src/ast_infer.rs`：infer.rs の構文木を辿る部分（パターン・ブロック・式・メソッド呼び出し・クロージャ・書式マクロの引数など 1,900 行）を
  自前の構文木に移植した。単一化器は solver.rs を共有する。`tests/ast_infer.rs` が probe / examples と crustc 自身のモジュール
  （`out/selfhost/program_main.rs`）の 885 関数・2,380 個の let で syn 版と同じ型になることを確かめる
- 推論の結果をノードのアドレスに加えてソース上の位置でも記録し、codegen が書き換えで複製したノード（`__exr_slice_to_vec(&a[..])` の中の式など）も型を引けるようにした
- 足したもの：`DerefMut` trait、Deref 越しのフィールド（`self.prog` で self: `&mut Inf`）、`Option::iter` / `into_iter` と `it.chain(Some(x))`、
  クロージャで `split`（`s.split(|c| ..)`）
- 修正：
  - `let Some(name) = name else { .. }`（外の変数を同じ名前で隠す）で、外の変数の drop をやめず二重に解放していた
  - 一時値のフィールドのムーブ（`f().0`）で一時値を丸ごと drop していた（残りのフィールドだけ drop する）
  - 相互再帰の型（`Ty` ↔ `FnTy`）の deep hash が同じになり、別の型のインスタンス（`HashMap::find<&FnTy>` と `<&Ty>`）のキャッシュキーがぶつかっていた
  - 自前の構文解析で `&&T` を 3 重の参照と読んでいた
  - `*const T` の `const` をモジュールの名前空間の定義と見ていた
- 回避（crustc が未対応のもの）：スコープにない trait のメソッドを候補から外さない（`Solver::deref` を `deref_var` に改名）、
  `std::mem::discriminant`（形の番号の関数にした）

#### セルフホストの第 16 段階: Cranelift の代わりのバックエンドの土台

- `src/minclif.rs`：Cranelift の FunctionBuilder に寄せた API（ブロック・ブロック引数・変数・スタックスロット・整数の命令・比較・select・
  読み書き・呼び出し (System V、7 個目以降はスタック)・return）で x86-64 の機械語を直接出し、ELF の再配置可能オブジェクトを書く最小のバックエンド。
  値はすべてフレームのスロットに置く単純な生成。`tests/minclif.rs` がループ・再帰・8 引数の関数を組み立てて cc でリンクして動かす。
  crustc でコンパイルした版も同じバイト列の .o を書き、それをリンクして動く（tests/selfhost.sh の minclif）
- 浮動小数点 (f32 / f64 の四則・sqrt・丸め・比較 (NaN の扱いも)・符号の操作・整数との変換 (Rust の `as` と同じく飽和))、
  i128 (加減・ビット演算・比較・シフトは命令列、乗除は libgcc の `__multi3` など)、オーバーフローつきの演算、clz / ctz / popcnt /
  bswap / 回転 / min / max / abs / umulhi、trapz / trapnz、関数ポインタの呼び出し、名前のアドレス (GOT 経由)、
  浮動小数点の引数・戻り値 (xmm)、.data とその中のポインタ (R_X86_64_64) まで。`tests/minclif.rs` の ext_ops が 69 個の結果を rustc の計算と比べる

#### セルフホストの第 17 段階: syn・Cranelift を使わない経路でプログラムを作る

- `src/ast_codegen.rs`：自前の構文木と ast_infer の型から minclif で機械語を作るコード生成 (codegen.rs の置き換えの始まり)。
  `build_object(src)` が lexer → prelude → ast_summary → program → ast_infer → ast_codegen → minclif → ELF の .o を作る (syn も Cranelift も使わない)
- いま扱えるもの: 整数・bool・char・キャスト、let / 代入 / 複合代入、算術・ビット演算・比較・`&&` / `||`、if / while / loop / break / continue / return、
  ユーザの関数と extern "C" の関数の呼び出し。`tests/ast_codegen.rs` が `tests/acg/*.rs` (no_std) を cc でリンクして rustc 版と出力を比べる
- v1: 値の表し方を codegen.rs に寄せた。ローカル変数はスタックスロット、struct / enum / 配列 / タプル / fat pointer はアドレスで扱い memcpy で複製、
  集約型の引数はアドレス渡し、集約型の戻り値は隠れた第 1 引数 (sret)。struct / タプル / 配列 / enum の値とパターン (match / if let / while let / let-else)、
  フィールド・添字 (範囲検査)・`&a[i..j]`・`&[T; N]` → `&[T]`、文字列 / バイト列リテラル (.data)、`len` / `as_bytes` / `as_ptr`、
  範囲・配列・スライスの for、ジェネリックでないメソッド。`tests/acg/aggs.rs` が rustc と同じ出力になる
  (ast_infer は配列・タプル・参照・演算・キャストの式の型も記録するようにした)

#### セルフホストの第 18 段階: codegen.rs を自前の構文木と minclif に移す (acg / driver2)

- `src/acg/`：codegen.rs (syn + Cranelift、12,500 行) を、自前の構文木 (parse.rs) と minclif の上にメソッド単位でそのまま移したもの。
  `clif.rs` が Cranelift と同じ形の API (`b.ins().iadd(..)` など、i128 / 浮動小数点の振り分けつき) を minclif の上に作るので、
  IR を組み立てるコードはほぼ変えずに移せる。`mod.rs` が Cx の定義と中核 (メモリ・変数・関数本体・ブロック)、`p1`〜`p8` が残り
  (式 / 呼び出し / 演算 / メソッド / ループとマクロ / match と drop / 書式化・ハッシュ・比較 / その他)
- `src/driver2.rs`：syn と Cranelift を使わないドライバ。lexer / macros / prelude → ast_summary → Program → acg → minclif の ELF の .o → リンク。
  インスタンスの不動点・クロージャ・static・vtable・同じ機械語のインスタンスの畳み込み・panic の場所の表は driver.rs と同じ
- `--features acg` で有効になる。`CRUSTC_DRIVER2=1 crustc x.rs` でこちらを使う。`tests/driver2.rs` が probe / examples / acg を今のドライバと比べ、
  `tests/selfhost.sh` (当時は `SELFHOST_DRIVER2=1`。今は常にこの経路) がセルフホストの全段階 (lexer〜program・minclif・enc) を driver2 でビルドして rustc 版と比べる (すべて一致)
- 移して分かったこと:
  - minclif のブロックに後から戻って続きを書けなかった (Cranelift は書ける)。離れるときに続きへの jmp を置くようにした
  - i128 の引数・戻り値はレジスタ 2 つ (System V)。i128 の変数・ブロック引数も 16 byte で写す
  - コード生成は式を複製して書き換えるので、アドレスで引く型の記録が当たらない。syn 版はソース上の位置で引いていた。
    型推論が読んだマクロの引数の構文木をそのままコード生成に渡し、さらに「式の形 (Debug のハッシュ) → 型」の表で引く
  - 型推論が捨てた一時的な式のアドレスが、コード生成の新しい式に再利用されて違う型が当たっていた。推論の構文木はすべて最後まで持つ
  - 同じ文字列のクロージャ (`|x| x.1.clone()` が 2 か所) が同じ番号になっていた。本体のクロージャはアドレスで引く
  - 自前の構文解析の誤り (`&&x` のパターン、パターンの `true` / `false`)、整数の `clone()` の型推論の誤り (syn 版にもあった) も直した

#### セルフホストの第 19 段階: crustc で crustc (syn・Cranelift を使わない経路) をコンパイルする

- `tests/selfhost/build_compiler.py`：driver2 + acg + minclif と、それが使うモジュール全部 (lexer / macros / prelude / types / parse /
  ast_* / program / solver / ctors / ast_infer / ast_consteval / link) を 1 ファイル (約 3 万 1 千行) のコンパイラにまとめる
  (`compiler <in.rs> -o <out> [-O]`)
- これを crustc (driver2) でコンパイルしたものが、実際にプログラムをコンパイルして動かせる (crustc が自分自身をコンパイルした)
- `tests/selfhost_compiler.sh`：rustc 版と crustc 版の 2 つのコンパイラで probe / examples / acg をコンパイルし、.o と実行結果を比べる。
  さらに crustc 版で自分自身をもう一度コンパイルする (2 段目)
- 結果: **139 個のプログラムすべてで、rustc 版と crustc 版のコンパイラが同じバイト列の .o を出し、実行結果も同じ**。
  crustc 版のコンパイラで自分自身をコンパイルし直すと、1 段目と同じ .o (約 18 MB) になる (不動点)
- 比べて見つかったもの: `matches!(tmp, Some(V(x)) if ..)` で束縛が一時値からムーブして二重に drop していた (syn 版にもあった)、
  acg の i128 の `abs` / 加減算のオーバーフロー検査
- そこまでに直したこと: モジュールを平らにするときの名前 (`std::path::Path` と同名の定義、`use super::*` で見える use、`&'static`)、
  型推論 (受け手の型が分からない `len`、注釈つきの let、`x.clone()` の整数)、acg の deref の強制変換 (`&&T` → `&T`、`&String` → `&str`)、
  範囲の添字の受け手の自動参照、`matches!` の guard の中のクロージャ、minclif の bitcast の 0 拡張。crustc がまだ扱えない書き方
  (分解代入・`collect::<Option<Vec<_>>>`・クロージャから fn ポインタへの変換・`std::f64::consts` の値など) はソースを書き換えた

#### 自前の構文木のソース位置

- `parse::Span { start, at }` を、panic の場所に使う式 (Unary / Binary / AssignOp / MethodCall / Index / Macro) に持たせた。
  start は式の先頭 (syn の `e.span()`)、at は式ごとの位置 (メソッド名・`[`・演算子・マクロの開き括弧)
- 式のマクロの引数のテキストは、元の行・桁の配置のまま作る (`group_text_laid_out`)。読み直すときはマクロの位置からずらす
  (`parse_expr_at` / `ast_closure::macro_args_at`) ので、`println!` の引数の中の `unwrap()` なども元の位置になる
- acg の `loc()` がこの位置を使う: 添字の範囲外 (Vec は `[`、配列は式の先頭)・オーバーフロー・0 除算・`unwrap` / `expect` (メソッド名)・
  `panic!` / `assert!` / `dbg!` (マクロの先頭) の場所が rustc と同じになった。`tests/panic_loc.rs` が rustc と比べる
- `tests/driver2.rs` は stderr (panic のメッセージ) も比べるようにした (139 すべて一致)。`matches!` の引数は型推論とコード生成が同じ構文木 (`ast_closure::matches_parts`) を使う
- `dbg!` の桁は syn 版が引数の位置を出していた (rustc はマクロの位置)。syn 版も直した
- 移植の初めに作った ast_codegen.rs (v1) は acg が置き換えたので消した (tests/ast_codegen.rs は driver2 で tests/acg を試す)
- コンパイルエラーにも場所を付ける: Path / Field / Lit にも位置を持たせ、acg のエラーには「位置を持つ一番内側の式」の
  `行:桁:` を付ける (メソッド呼び出し・フィールドは名前の位置、let の型の不一致は初期値、引数の型の不一致はその引数、
  return・末尾の式はその値)。構文エラー・要約のエラーはアイテムの中の行をファイルの行に直す。
  `tests/compile_errors.rs` が tests/compile_errors/*.rs で syn 版のドライバとメッセージ (場所ごと) が同じか比べる

#### driver2 のキャッシュと並列化

- `src/cache2.rs`：キーは 128 bit のハッシュ、値はアイテムの要約 (`types_enc.rs` の Summary の直列化) か、関数の機械語 +
  シンボリックな再配置 (acg::Blob。呼び先のインスタンスは「元の関数名 + 型引数」で書く)。保存は今回使ったエントリだけ
- 関数のキーは「本体のハッシュ + 自分と呼び先のシグネチャ + 名前で現れる struct のレイアウト + const」(driver.rs と同じ考え方)
- 並列化は fork: 子プロセス k が i % jobs == k の関数をコンパイルし、Blob を pipe で親に返す。親は順に読み戻してインスタンスを
  要求し直す (j=1 と j=8 で .o は同じバイト列)。crustc の std にスレッドがないので libc の fork / pipe / waitpid を直接呼ぶ
- 400 関数のプログラム: cold 104 ms (j=1) / 90 ms (j=8)、変更なし 7 ms (リンクしない)、1 関数の変更で 1 関数だけコンパイル

#### 複数ファイル・複数クレート (`crustc build <workspace>`)

- `mod x;` は `x.rs` / `x/mod.rs` を読む (`src/srcmap.rs`)。行番号を変えないように、`mod x;` は空白にして中身をファイルの最後に
  `pub mod x { .. }` として足し、結合したソースの行 → (ファイル, 行) の表を持つ。エラーは `ファイル:行:桁:` になる
- ワークスペースのファイルは 1 行 1 クレート (`lib <名前> <根> [依存..]` / `bin <名前> <根> [依存..]`)。クレートごとに .o を作り、最後にリンクする
- クレート C は「C のソース + 依存するクレートのソースを `pub mod 名前 { .. }` で後ろに付けたもの」として解析する。
  依存するクレートの総称でない関数と static は、そのクレートの .o にあるのでコンパイルせずシンボルで呼ぶ。総称関数のインスタンスと
  prelude は各 .o に local で置く。クレートのモジュールの中の名前は必ず `名前__Item` にする (どのクレートと一緒に解析しても同じシンボル)。
  クレートの中の `crate::` はそのクレート、`pub use` の再輸出も辿る
- panic の表はクレートごとに置き、実行ファイルの表が依存するクレートの表 (`_xT_<名前>`) の列を指す。表にはファイルの対応と、
  名前を変えて桁がずれた所 (`geom::f` → `geom__f` は同じ長さだが、`f` → `geom__f` は長くなる) の表を入れ、panic の場所は
  元のファイル・行・桁で出る (rustc と同じ `geom/src/lib.rs:13:37`)。unwrap の呼んだ場所 (`__EXR_CALLER`) は実行ファイルのものを皆で使う
- 指紋: クレートごとに「コンパイラ・設定・自分のソース・依存するクレートのインターフェースのハッシュ」を `<名前>.fp` に書き、同じなら
  何もしない。インターフェースのハッシュは、総称でない関数はシグネチャだけ、それ以外 (総称関数・trait・総称の impl・`const fn`・
  `-> impl Trait`) は本体と行も入れる。だから依存されるクレートの関数の本体を変えても、使う側はコンパイルし直さない (リンクだけ)
- 古いクレートは互いのソースしか使わない (.o を待たない) ので、fork して同時にコンパイルする。キャッシュはクレートごと (`<名前>.crcache2`)
- `tests/workspace.rs`：tests/workspace (3 クレート・5 ファイル: trait オブジェクト・static・再輸出・Display) を rustc (rlib + リンク) と
  比べる (出力・終了コード・クレートとファイルをまたぐ 0 除算 / 添字 / unwrap の panic の場所)。関数の本体の変更でそのクレートだけ、
  シグネチャの変更で使う側も作り直すことも確かめる

`python3 bench/ws_bench.py --rustc` (6 lib × 8 ファイル × 40 関数 = 1920 関数 + bin、4 コア。rustc は `-C incremental` の rlib + リンク):

| 場面 | crustc build | rustc |
|---|---:|---:|
| cold | 489 ms | 3786 ms |
| 変更なし | 7 ms | 1043 ms |
| いちばん下のクレートの関数の本体を変える | 66 ms | 1062 ms |
| いちばん下のクレートの関数のシグネチャを変える | 189 ms | 1136 ms |
| bin の main を変える | 83 ms | 1029 ms |

(どれも 3 回の中央値。シグネチャの場面は 150〜190 ms で揺れる)

速くするためにしたこと (この順に計った):

| 変更 | cold | 変更なし | 本体 | シグネチャ | bin |
|---|---:|---:|---:|---:|---:|
| 最初 (毎回すべてのクレートを解析) | 1229 | 725 | 936 | 733 | 805 |
| 指紋 + インターフェースのハッシュ + クレートごとのキャッシュ | 1296 | 15 | 798 → 83 | 743 | 237 |
| モジュールの名前の付け替えの 2 乗を除く (トークンごとの線形探索を表に) | 880 | 24 | 83 | 422 | 108 |
| 古いクレートを fork で同時に | 486 | 20 | 81 | 229 | 113 |
| インターフェースのハッシュを保存、名前の表を FxHash にしパスでない名前を先に除く | 464 | 7 | 60 | 154 | 82 |

(「本体」の 798 → 83 は、平らにした後のテキストで `{` を探していたインターフェースのハッシュの誤りを直した分)。
残りはリンク (約 22 ms、毎回すべての .o) と、使う側ごとに依存するクレートのソースを解析し直す時間 (名前の付け替え・分割・prelude の選択)。

セルフホストで見つかった遅さ: crustc で作ったコンパイラが自分自身をコンパイルすると、.o を作る段階が 13 分以上かかっていた。
`parse::tokenize` がトークンごとに行頭から文字を数えていた (長い行で 2 乗。rustc 版は SIMD の `chars().count()` で目立たなかった) ので、
前のトークンから数え足すようにした。2.4 秒になり、2 段目全体は 36 秒 (rustc 版のコンパイラは 5.6 秒)。

#### セルフホスト版のコンパイラの前半 (front) を速くする

crustc で作ったコンパイラ (`tests/selfhost/build_compiler.py`) で自分自身 (約 3 万 3 千行) をコンパイルするときの前半の時間
(`CRUSTC_PROF=1 ... --time` の段階ごとの時間。gdb で止めた場所を数えて、多いところから直した):

| 段階 | 最初 | 後 | rustc 版のコンパイラ (後) |
|---|---:|---:|---:|
| macros | 9870 ms | 499 ms | 96 ms |
| flatten (モジュールを平らに) | 2501 | 552 | 51 |
| hoist + ctor | 613 | 467 | 61 |
| split | 578 | 287 | 18 |
| prelude の選択 | 1219 | 74 | 16 |
| 要約 | 約 2400 | 1054 | 110 |
| program + refs | 862 | 645 | 72 |
| **front の合計** | **18159** | **3745** | **439** |

- macros: 自前の proc_macro2 (mini_pm2) の字句解析がトークンごとに行頭から桁を数えていた (前から数え足す)。
  `find_calls` / 定義集めが括弧ごとに中身を写していた (中にマクロの名前が無い括弧は辿らない)。
  展開したものにマクロの名前が無ければもう一周しない。さらに、ソース全体は速い `lexer::tokenize` で見て、proc_macro2 で読むのは
  定義と呼び出しの括弧の中だけにした (`tests/macros_expand.rs` が前の版 `expand_reference` と結果が同じか確かめる)
- flatten: 表を埋める `iter_mut().take(n).skip(m)` が毎回先頭から辿っていた (添字のループに)
- prelude の選択: ソース全体の `contains` を 40 回ほど呼ぶ。prelude の `str::find` を libc の `memmem` にした
  (crustc で作るプログラムすべてで速くなる)
- split: アイテムのハッシュ (キャッシュのキー) は SipHash を 2 つ使っていた。2 本の掛け算・回転の混ぜ合わせ (`src/key.rs`) にした。
  呼び出しの区切りに依らない (8 byte ずつ溜める) ので、std と crustc の prelude で Hash の実装が write を呼ぶ区切りが違っても同じ値になる
- 要約: アイテムごとに独立なので fork して分ける (driver2 の関数のコンパイルと同じ)。構文解析の `punct_at` (演算子の優先順位ごとに
  何度も呼ぶ) をバイトで比べ、記号でない位置では演算子を探さない。字句解析は ASCII をバイトのまま見る
- refs: 4 つの表を引く前に、どれかの表にある名前の集合で 1 回だけ引く
- 残りは生成するコードの質 (小さい関数のインライン展開がない、`Iterator` のアダプタが関数呼び出しになる)。後半の codegen は 15.8 秒
  (rustc 版 0.95 秒) で、こちらも同じ理由

#### 小さい関数のインライン展開 (driver2 の `-O`)

- minclif は IR を持たず命令をすぐ機械語にするので、構文木の段階で展開する (`src/acg/p9_inline.rs`)。
  呼び出しの引数を ABI の形 (値渡しの集成体はコピーしたアドレス、sret は書き込む先) まで作ったところで、呼ぶ代わりに
  呼び先の本体を同じ関数にコード生成する。本体の引数はその値に束縛し、`return` / `?` / 末尾の値は続きのブロックへの
  jump にする (`emit_return` が見る)。呼び先の型推論・スコープ・drop の状態は入れ替えてから戻す
- 展開する関数: 本体の構文木が小さく (Debug の文字列で 1600 字まで)、クロージャ・マクロ・入れ子のアイテムを含まず、
  特別な本体 (drop glue など) や呼んだ場所を出す unwrap / expect でないもの。prelude もユーザの関数も (`Vec::len` /
  `Option::is_some` / `Parser::tok` など、約 300 個)。展開した中ではもう 1 段まで、1 関数で 400 回まで
- ユーザの関数を展開しうる呼ぶ側は、キャッシュのキーに呼び先のアイテムのハッシュと行を入れる。panic の場所は
  呼び先の行を基準にする (rustc の -O と同じ場所)。依存するクレートの関数は展開しない (本体はインターフェースの
  ハッシュに入らないので、変わっても使う側が作り直されない)
- 呼び先の型推論は呼ぶ側に依らないので、インスタンスごとに 1 回だけ (プロセスの中で使い回す)
- 展開した本体だけが呼ぶ関数は、呼ぶ側の名前からは届かない: コンパイルした機械語の再配置が指す総称でない関数も
  コンパイルする (これがないとリンクで未定義になった)
- 一緒に minclif に、直前に書いた値をすぐ読み直す命令を省く覗き穴最適化を入れた (`rax_holds`: 直前の命令が
  「rax を v のスロットに書く」なら v を読まない。ブロックの始まりでは消す)
- `tests/driver2_inline.rs`：probe / examples / acg を `-O` と `-O` なしでビルドし、出力・stderr・終了コードが同じか
- 効果 (セルフホスト版のコンパイラで自分自身をコンパイルする時間。どれも `CRUSTC_NO_INLINE=1` で同じ仕事をさせて比べる):
  展開なし 20.85 秒 → 展開あり 19.76 秒 → 読み直しの省略も 18.32 秒 (12% 速い)。展開で生成するコードは 27% 大きくなり、
  `-O` のコンパイルは約 20% 遅くなる (型推論の使い回しの後)。minclif はすべての値をスタックに置くので、呼び出しを
  なくしても値の読み書きが残り、効果は小さい。大きく速くするにはレジスタ割り当てが要る

#### minclif のレジスタ割り当て

minclif は命令をすぐ機械語にし、値はすべてスタックのスロットに置いていた (命令ごとに rax / rcx に読んで結果を書く)。
割り当てには関数全体を見る必要があるので、2 回組み立てる形にした (`src/minclif.rs`):

1. **記録と解析**: `FunctionBuilder` は呼ばれた命令を `Op` (104 種。Emitter の公開メソッドから機械的に作った) として記録しながら、
   今までの組み立て (`Emitter`) で 1 回目を組み立てる (値の番号と型を決め、acg が問い合わせる値・ブロックを返す)。
   1 回目は値・変数の読み書き (命令の番号)、区間 (ブロックを続けて書いた範囲) と飛び先、関数を呼ぶ命令を記録する
2. **割り当て** (`plan`): 1 つの区間の中で書いて読む値はその範囲、区間をまたぐもの (変数・ブロック引数・ループで生きる値) は
   後ろ向きのデータフロー (区間の入口・出口で生きているか) で生きている区間の端まで延ばした範囲を生存区間にし、線形走査で
   rbx・r12〜r15 (呼ばれた側が保存: 呼び出しをまたぐ区間用) と rsi・rdi・r8〜r10 (呼び出しを含まない区間用) に割り当てる。
   足りなければ「読み書きの回数 / 長さ」が小さいものをスロットへ。浮動小数点・i128・スロットを直接使う値 (アドレスとして
   使う・幅の違う読み書き) はスロットのまま。すぐ後が trap の呼び出し (panic) は戻らないので、またぐとしない
3. **本番**: 割り当てを持った Emitter で記録した命令をもう一度組み立てる。今までの命令の組み立てはそのまま使え、
   値の読み書き (load_rax / store_rax など) が割り当てを見る。よく使う命令はレジスタの形で書く: 64 / 32 bit の
   add / sub / and / or / xor / imul と cmp は `mov d, a; op d, b`、定数は直接、load / store は `[base + off]`。
   使った呼ばれた側が保存するレジスタはプロローグで保存し ret の前に戻す。呼び出しの引数が引数のレジスタにあるときは
   スタックを経由する (順に書くと壊れる)

あわせて: `icmp` の値を `brif` だけが使うなら `cmp` + `jcc` にする、else に渡すものがなければ直接飛ぶ、すぐ次への `jmp` は nop。
浮動小数点は xmm8〜15 (呼び出しで壊れるので、呼び出しを含まない区間だけ) に置き、`fadd` などは 2 つ目もレジスタから読む。
変数と値の間は直接写す。自分の関数 (`_x..`) を呼ぶときは可変長引数用の `al` を書かない。
`MINCLIF_NO_REGALLOC=1` で割り当てなし (比較用)。

結果:
- `python3 bench/runtime.py` の実行時間 (driver2 -O。出力はすべて一致):

  | | 割り当てなし | 整数だけ | + 浮動小数点・直接の写し | Cranelift の経路 (`crustc -O`) | rustc -O |
  |---|---:|---:|---:|---:|---:|
  | fib | 111 ms | 90 | 96 | 82 | 30 |
  | hashmap | 341 | 210 | 208 | 149 | 103 |
  | matmul | 242 | 190 | 158 | 91 | 45 |
  | nbody | 1150 | 725 | 516 | 83 | 47 |
  | sieve | 346 | 185 | 169 | 141 | 109 |
  | sort | 624 | 344 | 341 | 252 | 210 |
  | strings | 227 | 119 | 120 | 70 | 42 |

  rustc のデバッグビルドよりどれも速く、ビルドは Cranelift の経路の約半分の時間。実行は Cranelift の経路にまだ届かない
  (struct のフィールド・配列の要素はメモリのまま、命令の選び方がまだ単純)
- 簡単なループ (1000 要素の和 × 20 万回): 1.08 秒 → 0.44 秒 (rustc の opt-level=0 と同じ)
- セルフホスト版のコンパイラで自分自身をコンパイルする時間: 18.0 秒 → **10.1 秒** (front 3.2 → 1.7 秒、codegen 13.0 → 7.6 秒)。
  コンパイラの実行ファイルは 23 MB → 12 MB
- リンカに wild (0.10) を入れた (`cargo install --locked wild-linker --version 0.10.0`。link.rs は wild > mold > ld.lld の順に使う)。
  ワークスペースのベンチ: cold 355 ms、関数の本体の変更 37 ms、bin の変更 60 ms

#### スタックの struct のフィールドをレジスタに置く (minclif)

- スロット (struct・tuple・配列の一時領域) のアドレスを、解析のときに追う: `stack_addr` の値と、それに定数を足した値は
  「(スロット, オフセット)」。そのアドレスが load / store / 定数の足し算にしか使われず (呼び出しに渡す・保存する・
  ほかの計算に使うと逃げたとみなす)、読み書きが重ならず同じ場所はいつも同じ型なら、(スロット, オフセット) ごとに
  フィールドを 1 つの変数にする (scalar replacement)。割り当てでは値・変数と同じに扱い、レジスタに置けなかったフィールドは
  スロットのその場所を使う。アドレスの値は作らない
- acg の struct のコピーを、8 byte ずつからフィールドごと (その型) にした (読み書きの幅がフィールドと揃うように)
- 入口で 0 にするスロット (drop フラグ) は変数にしない。`MINCLIF_NO_SROA=1` で切れる
- 値で渡して値で返す struct のループ (tests/acg/struct_fields_regs.rs と同じもの、2000 万回): 176 ms → 147 ms (rustc -O 87 ms)。
  bench/rt はほとんど変わらない (nbody などの struct は Vec の中: ヒープ)
- セルフホスト版のコンパイラで自分自身: 9.7 秒

Rust を 1.100.0、wild を 0.11.0 に上げた (`rustup update stable`、`cargo install --locked wild-linker`)。

#### driver2 の Cranelift バックエンド (minclif と比べる)

minclif の FunctionBuilder は acg が呼んだ命令をすべて `Op` として記録している。その列を Cranelift の IR に組み立て直して
コンパイルするバックエンドを足した (`src/clif_backend.rs`、`CRUSTC_BACKEND=cranelift`)。acg・インライン展開・キャッシュ・.o の
書き出しは同じなので、同じフロントエンド・同じ命令の列でバックエンドだけを比べられる。

- 値は minclif の値の番号 → Cranelift の値。ブロック・変数・スロット・関数も番号で対応させる (記録のときに命令ごとの戻り値も
  残すようにした)。acg は値を支配していないブロックで使うことがある (minclif は値をスロットに置くので困らない) ので、
  定義と別の区間で使う値は Cranelift の Variable にして、SSA は cranelift_frontend に組み立てさせる
- drop フラグのスロットは途中で作られるので、組み立てた後で入口の先頭に 0 を書く
- 再配置は名前で返す (呼び出しは PLT32、記号は GOTPCREL)。trap の番号も minclif と同じ
- `tests/driver2_cranelift.rs`：probe / examples / acg を -O なしと -O で、minclif と出力・stderr・終了コードが同じか (すべて一致)。
  セルフホスト版のコンパイラを Cranelift でビルドしたものも、minclif でビルドしたものと同じ .o を出す

実行時間 (`python3 bench/runtime.py`、ms):

| | driver2 -O (minclif) | driver2 + Cranelift | driver2 + Cranelift -O | 前の Cranelift の経路 (`crustc -O`) | rustc -O |
|---|---:|---:|---:|---:|---:|
| fib | 110 | 91 | 79 | 79 | 31 |
| hashmap | 214 | 155 | 144 | 148 | 112 |
| matmul | 168 | 105 | 76 | 85 | 40 |
| nbody | 522 | 232 | 86 | 105 | 54 |
| sieve | 158 | 150 | 154 | 134 | 94 |
| sort | 334 | 294 | 233 | 248 | 208 |
| strings | 111 | 91 | 68 | 73 | 42 |

セルフホスト版のコンパイラ (約 3 万 3 千行) で比べる:

| バックエンド | そのコンパイル (-j 1。acg を含む) | できる実行ファイル | できたコンパイラが自分自身をコンパイルする時間 |
|---|---:|---:|---:|
| minclif | 2.40 秒 | 8.8 MB | 11.5 秒 |
| minclif -O | 3.15 秒 | 10.6 MB | 10.1 秒 |
| Cranelift (opt none) | 5.07 秒 | 5.4 MB | 6.9 秒 |
| Cranelift -O (speed, backtracking) | 8.26 秒 | 6.6 MB | **6.0 秒** |
| Cranelift -O (speed, single_pass) | 6.95 秒 | 9.2 MB | 8.9 秒 |

(rustc -O でビルドしたコンパイラは 1.6 秒)。ワークスペースのベンチ (`bench/ws_bench.py`) は minclif / Cranelift で cold 440 / 727 ms、
変更なし 7 / 6 ms、関数の本体の変更 42 / 36 ms、bin の変更 74 / 67 ms (関数ごとのキャッシュで、変更のあとのコンパイルは小さい)。

分かったこと: minclif は Cranelift (opt none) の約 2 倍、-O どうしで約 2.6 倍速くコンパイルするが、出すコードは 1.0〜6 倍遅い
(セルフホスト版のコンパイラでは 1.7 倍)。増分のビルドでは変わった関数しかコンパイルしないので、コンパイルの速さの差は
cold のときだけ効く。

ここまでで、外部クレートに依存しないモジュール（lexer / enc / fxhash / prelude / link / testgen / program / ast_consteval / solver / ctors / ast_infer / minclif / ast_codegen）と、
依存が小さく代わりを書けるもの（elfz: zstd、macros: proc_macro2）は crustc でコンパイルでき、rustc と同じ結果になる。
残りの front（syn の構文木を引く検索）/ infer（syn 版。自前の構文木の版 ast_infer はある）/ consteval（codegen の定数の昇格）/ closure / codegen / driver / cache は syn と cranelift の上に書かれている。
syn の代わりは src/parse.rs（自前の構文木）を土台にして front から順に移す。cranelift の代わり (自前のコード生成) はその後。

#### 最初のドライバ (syn + Cranelift) を外す

- `crustc check` と LSP を driver2 の `Session2::check` に移した。全関数 (到達しないものも) と要求された総称関数のインスタンスを
  コンパイルしてエラーを集め、結果はキャッシュに入れる (次の検査・ビルドはそれを使う)
- `crates/legacy` (syn・cranelift-object・zstd) と、syn 版と比べていたテスト (ast_closure / ast_consteval / ast_infer /
  ast_summary / ast_types) とセルフホストの elfz を削除した。期待する出力は `tests/expected/` の golden ファイル
  (syn 版で作ったもの) にして、`tests/driver2.rs` が比べる
- 見つかったバグ: `Session2::save_cache` の後にメモリ上のキャッシュが空になっていた (使ったキーの集合を先に消していた)。
  同じ Session で続けてビルドすると全関数をコンパイルし直していた。ベンチの `build/edit_1fn` の assert で見つかった
- タプル構造体のパターンのフィールドの数のエラーに場所 (パスの位置) を付けた

#### 実在の小さなクレートをビルドする (`cratebench/`)

crates.io の小さなクレートのソースをそのまま `cratebench/vendor/` に置き (ライセンスも同梱)、それを使う app
(`cratebench/apps/<名前>/main.rs`) と一緒にワークスペースとしてビルドする。`python3 cratebench/run.py [名前..]` が
rustc (lib を rlib にして `--extern`) と比べる: 出力が rustc -O と同じか、ビルド時間 (cold / noop / app の 1 か所の編集)・
実行時間・サイズ。対象は `cratebench/crates.txt` (名前・edition・feature)。

足したもの:
- `#[cfg(..)]` / `#[cfg_attr(..)]` / `cfg!(..)` の評価 (`crates/syntax/src/cfg.rs`。feature はワークスペースの `features=a,b`
  か `--features=`。x86-64 Linux の値・`debug_assertions` は -O でなければ真)。`extern crate x;` は消す
- アイテム位置のマクロ呼び出しを `mod` / `impl` / `trait` の中でも展開する。`$x:expr` が式の終わりで止まる (`$($len:expr)+`)
- 定数式: impl の関連 const (`i128::MAX_STR_LEN`)・`u128::MAX.ilog10()` などの整数のメソッド。`const _: () = ..;` は読み飛ばす
- prelude: `chunks_exact` (`remainder`)・`Read` / `BufRead` (`&[u8]` の実装)・`ExactSizeIterator`・`slice::Iter::len`
- `impl Tr for &[u8]` のメソッドと境界・組み込みの `AsRef<str>` / `AsRef<[T]>` (`as_ref()` の先は期待型か関数の境界から)・
  `FromIterator` の境界・ブランケット impl の中のクロージャ・`&mut [u8; N] as &mut [u8]`・`&u8 >> 4`
- クロージャの中の `?`: 戻り値の Result のエラー型を `?` のものにする
- 同じ名前の impl が配列の長さごとに並ぶとき (`impl FromHex for [u8; 1]` .. `[u8; 4096]`) は self の型が合うものを選ぶ
- 型別名の名前がクレートの接頭辞で小文字始まり (`fnv__FnvHashMap`) でも、モジュールとして読み飛ばさない
- 直したバグ: `&&str` → `&str` の deref の強制変換が fat ポインタを読み込んでいた (値が壊れる)。注釈なしの let で
  型推論の型と実際の型が違うときは強制変換を試し、だめなら実際の型を使う

結果 (2026-11-16、ms。run は実行時間。下の最適化の後):

| crate | compiler | cold | noop | edit | run | size | same output |
|---|---|---:|---:|---:|---:|---:|---|
| fnv | rustc | 170 |  | 135 | 495 | 4483 KiB |  |
| fnv | rustc -O | 348 |  | 312 | 42 | 4413 KiB |  |
| fnv | crustc | 64 | 2 | 18 | 135 | 26 KiB | yes |
| fnv | crustc -O | 91 | 2 | 24 | 114 | 27 KiB | yes |
| adler2 | rustc | 157 |  | 95 | 624 | 4424 KiB |  |
| adler2 | rustc -O | 220 |  | 138 | 12 | 4389 KiB |  |
| adler2 | crustc | 52 | 2 | 18 | 262 | 16 KiB | yes |
| adler2 | crustc -O | 68 | 2 | 22 | 136 | 15 KiB | yes |
| hex | rustc | 265 |  | 121 | 148 | 4479 KiB |  |
| hex | rustc -O | 454 |  | 291 | 11 | 4399 KiB |  |
| hex | crustc | 79 | 2 | 27 | 84 | 52 KiB | yes |
| hex | crustc -O | 109 | 2 | 32 | 73 | 52 KiB | yes |
| rustc-hash | rustc | 203 |  | 142 | 468 | 4490 KiB |  |
| rustc-hash | rustc -O | 352 |  | 300 | 58 | 4408 KiB |  |
| rustc-hash | crustc | 67 | 2 | 19 | 182 | 29 KiB | yes |
| rustc-hash | crustc -O | 88 | 2 | 27 | 163 | 27 KiB | yes |
| cfg-if | rustc | 94 |  | 74 | 388 | 4390 KiB |  |
| cfg-if | rustc -O | 110 |  | 86 | 58 | 4387 KiB |  |
| cfg-if | crustc | 28 | 2 | 13 | 183 | 8 KiB | yes |
| cfg-if | crustc -O | 37 | 2 | 15 | 57 | 8 KiB | yes |

ビルドは rustc より 2〜4 倍速く、編集のビルドは 5〜10 倍速い。実行は rustc -O より遅い (adler2 は 40 倍だった)。

adler2 の内側のループ (`U32X4([u32; 4])` の `+=` と `chunks_exact`) を見て直したもの (-O の実行 478 → 114 ms、fnv 137 → 112 ms):
- Cranelift の経路にも SROA: minclif と同じ判定 (`minclif::sroa`。アドレスが load / store にしか使われないスロット) で、
  スロットの (オフセット, 型) ごとに Cranelift の Variable にする (Cranelift はスタックのスロットを SSA にしない)。
  `CRUSTC_CLIF_NO_SROA=1` で切る
- スロットのアドレスを入れた変数 (インライン展開した `&mut self`) も、定義がすべて同じアドレスならそのアドレスとして扱う
- スカラーの小さい配列 (16 要素まで) のコピーと `[0; N]` の初期化は要素の幅で読み書きする (8 byte ずつだと幅が合わず変数にできない)
- インライン展開の大きさを構文木の Debug の長さではなくソースのトークンの数 (110 まで) で測る (Debug は位置などで膨らみ、
  4 文の `add_assign` が 1565 文字になっていた)。`ChunksExact::next` も展開される

残り (rustc -O の約 10 倍): `a_vec.0.iter()` でアドレスが本当に漏れる、enum (`Option<&[u8]>`) のコピーが 8 byte ずつ、
rustc は SIMD にする。
itoa も通る (下)。either は未対応 (`Pin` / `Future` / io の各トレイト / `Error` / `fmt::Binary` など)。

itoa (-O の実行 440 → 73 ms、rustc -O は 32 ms) のために足したもの:
- 整数の const は使う所で畳み込む (`u8::MAX.ilog10() as usize + 1` を使うたびに u128 で計算していた: 実行の 74%)。
  評価は i128 で途中の切り詰めを見ないので、`!` を含むものと結果が型の範囲の外のものは実行時に計算する
- `999.try_into().expect(..)` (型に収まる整数リテラル) は定数
- クロージャの本体もインライン展開する (`Map::next` の中の `(self.f)(x)` など。キャプチャは `__env` のフィールドとして束縛する。
  `CRUSTC_NO_INLINE_CLOSURE=1` で切る)。依存するクレートのクロージャは展開しない (ほかの関数と同じ)
- prelude: `MaybeUninit<T>` (uninit は 0 で埋める)・`hint::unreachable_unchecked`。組み込み: `mem::zeroed`・整数の
  `ilog2` / `ilog10` / `unsigned_abs`・`get_unchecked(_mut)` (添字にする)・スライス → `&mut [T; N]` の `try_into`
  (行き先が型推論にないときは `.unwrap()` の期待型から)
- 定数: `<u64 as Integer>::MAX_STR_LEN` (修飾されたパスの関連 const)・プリミティブ型への impl の中の `Self::C`・
  const ジェネリクスの引数の式 (`enc::<{ u128::MAX_STR_LEN - 16 }>`)・static の `*b"..."`
- モジュールを平らにするときの名前の衝突回避が、impl の中の関連型の定義 (`type Buffer = ..`) まで改名していた (トップレベルに
  同じ名前の `struct Buffer` があると)
- `const _` を読み飛ばしたときに、const の型を付けるループの添字がずれていた

rustc-hash・cfg-if のために足したもの:
- マクロの呼び出しの引数と macro_rules! の中の `#[cfg]` は評価しない (cfg_if! の `if #[cfg(..)] { .. }` を消していた)。
  代わりにマクロを展開した周ごとに評価する (`#[cfg(p)] $crate::cfg_if! { .. }` を、中を展開する前に)。feature はクレートごと
- `$t:tt` に入った Joint の記号 (`->` の `-`) の後に空白を入れない (`- >` になっていた)
- prelude: `BuildHasher` (`hash_one`)・`split_first_chunk::<N>()` / `first_chunk` (turbofish を prelude の関数に渡す)
- `[T]` の Hash は std の `hash_slice` と同じく整数の列をまとめて `write` する (FxHasher で値が std と一致する)
- 型の中の `<<` (`Either<<&L as IntoIterator>::IntoIter, ..>`) は `<` と修飾されたパスの `<`

#### 依存の多い複数クレート (`bench/deps_bench.py`)

葉に cratebench の実在のクレート 6 個、その上に生成した中間クレート N 個 (それぞれ実在のクレート 2〜3 個と前の中間クレート
最大 3 個に依存する DAG。struct・trait・総称関数・クロージャ・cfg_if! を使う)、いちばん上に app を置き、cargo
(path 依存のワークスペース、dev プロファイル、4 並列) と比べる。場面は cold / noop / いちばん下の中間クレート m0 の関数の
本体の変更 (low-body) / m0 の pub 関数のシグネチャの変更 (low-sig、使う側も直す) / app の変更。出力は cargo と同じか確かめる。

最初は、使う側のクレートが推移的な依存のソース全体を毎回展開・解析し直していたので、依存が増えると cold と low-sig で cargo に
並ばれた (100 クレートで cold 3718 ms (cargo 3250)、low-sig 1581 ms (cargo 1578)。解析の内訳はマクロの展開が 1.5 秒)。直したもの:
- 依存するクレートのマクロの展開は 1 回: 各クレートは依存するクレートのマクロの定義だけを添えて自分の分だけ展開する
- 使う側に渡すのはインターフェース (`crates/syntax/src/stub.rs`): 総称でない関数の本体を `{ loop {} }` にする (依存する
  クレート自身がコンパイルする)。総称関数・総称な impl・trait・`const fn`・`-> impl Trait`・`async fn` は残す。
  展開とインターフェースは `<名前>.prep` に保存する。`CRUSTC_NO_DEP_STUB=1` で前の動き
- 並列にビルドする子プロセスが作ったキャッシュのエントリ (要約・関数の機械語) を親に返し、後の組のクレートが fork で引き継ぐ
  (prelude の単相化や依存するクレートのアイテムの要約は多くのクレートで同じ: 100 クレートの cold でコンパイルする関数 11360 → 5984)
- 細かい指紋 (`<名前>.fp2`): 依存するクレートのインターフェースのアイテムのうち、自分の識別子から名前でたどって届くもの
  (impl はヘッダの型・trait の名前、メソッドは名前で届く: 取りこぼしでなく取りすぎの側) のハッシュだけを入れる。前の指紋が
  違っても細かい指紋が同じなら作り直さない。本体を消してあるので、依存の依存の関数のシグネチャは使う側に届かない
- 本体を消したので、自分の型を `Box<dyn Tr>` にしたときのメソッドが名前で届かなくなる: vtable の欄からもコンパイルする

- 使う側に渡すのは、依存するクレートのインターフェースのうち自分から名前で届くアイテムだけ (細かい指紋と同じたどり方)。
  ほかは空白にする (`use` は名前の解決に要るので残す)。これで平らにする・要約・Program・prelude の選択が使う分だけになる
  (100 クレートの m50 で依存のアイテム 800 前後 → 150)。`CRUSTC_NO_DEP_PRUNE=1` で切る。
  impl の名前はヘッダの trait と型の名前で、`impl<..>` の中・`where` から後ろ・プリミティブ型は数えず、std の型 (`Vec` など)
  への trait の実装は trait の名前だけで届く (`impl FromHex for [u8; 4]` が `u8` と書いたクレートすべてから届いていた)

結果 (ms、中央値。「前」は使い回しの前、「中」はインターフェースと指紋とキャッシュの共有まで、右は名前で届くものだけにした後):

| 中間クレート | 場面 | crustc 前 | crustc 中 | crustc | cargo |
|---|---|---:|---:|---:|---:|
| 20 (400 関数) | cold | 759 | 462 | 261 | 858 |
| | low-body | 29 | 33 | 23 | 373 |
| | low-sig | 249 | 54 | 32 | 398 |
| | app-body | 62 | 42 | 24 | 113 |
| 50 (1500 関数) | cold | 1927 | 1050 | 612 | 1719 |
| | low-body | 33 | 39 | 27 | 797 |
| | low-sig | 731 | 76 | 61 | 803 |
| | app-body | 110 | 72 | 34 | 164 |
| 100 (3000 関数) | cold | 3718 | 1866 | 1190 | 2933 |
| | noop | 11 | 13 | 11 | 119 |
| | low-body | 46 | 42 | 33 | 1434 |
| | low-sig | 1581 | 126 | 87 | 1465 |
| | app-body | 204 | 107 | 49 | 246 |

その後、prelude の関数の機械語をクレートの間で使い回すようにした (100 クレートの cold で、コンパイルする関数 5984 → 4973):
- 閉じた prelude の関数 (本体が prelude で、総称でないか型引数が prelude・プリミティブの型だけ、クロージャでない) のキーは、
  コンパイラ ID・本体・自分のシグネチャ・`#[panic_handler]` だけにする。参照先のシグネチャを名前の最後で多めに取っていたので、
  `.push(` を呼ぶ prelude の関数のキーにユーザの `Acc5::push` が入り、クレートごとに違うキーになっていた
- prelude の関数の panic の場所は prelude の中の行にする (`PRELUDE_LINE` = 49152 から後ろを場所の表で `<crustc prelude>` の
  ファイルにする)。選んだ prelude のアイテムは元の行に置く (間は改行)。前は prelude の場所の行の基準がユーザのソースの長さと
  prelude の選び方で変わるのでキャッシュが当たらず、表示も `src/main.rs:259` のような意味のない場所だった
- 依存するクレートの総称関数のインスタンスも使い回す: 本体が prelude か依存するクレート j にあり、型引数にこのクレートの型を
  含まない関数 (閉じた関数) は、キーに「j と型引数の型のクレートから見えるクレート」の中の参照先だけを入れる。panic の場所は
  j のテキストの中の行で持つ: trap の表は .o を書くときに使う側での位置を足し、定数で渡す場所 (添字の検査・assert_eq!・
  unwrap の呼んだ側) は行の上位ビットに j の印 (`crate_tag`) を入れ、実行時に実行ファイルの表 (`_xT__exe`) の依存する
  クレートの表の列から j の表を選んで引く。ついでに、依存するクレートの中の定数で渡す場所を実行ファイルの表で引いていた
  (違うファイル・行を出していた) のも直った。tests/workspace に、依存するクレートの総称関数の中の範囲外の添字を足した。
  このベンチでは重複が 80 個ほどだったので、cold の時間は計測の揺れの範囲 (100 クレートで 1088〜1223 ms)
- Program を「依存の土台 + 自分のアイテム」に分けるのは見送った: 測ると Program の段は 1 クレートあたり約 1 ms
  (cold の合計の 4%、app の変更の 2%) で、土台 (依存の届く部分と選んだ prelude) もクレートごとに違って共有できない

| 中間クレート | 場面 | crustc | cargo |
|---|---|---:|---:|
| 20 | cold | 265 | 862 |
| 50 | cold | 553 | 1731 |
| 100 | cold | 1088 | 3070 |
| 100 | noop / low-body / low-sig / app-body | 11 / 37 / 94 / 49 | 120 / 1481 / 1557 / 250 |

cargo は依存するクレートが変わると (本体だけでも) 使う側をすべてコンパイルし直す。crustc は関数ごとのキャッシュと
インターフェースの指紋で、変わった所を使うクレートだけを作り直す。残り: 各クレートが prelude (500 アイテム余り) の要約・
Program を作り直している (クレートごとに選ぶ prelude が違うので、Program を丸ごと共有はしていない)。cold の残りの大半は関数の
コンパイル (Cranelift)。

### Cranelift の経路の関数のコンパイルを速くする (2026-11-17)

自分自身 (out/selfhost/compiler_main.rs、38886 行・9178 関数) を `--no-cache` でコンパイルした時間 (ms)。
どの段階でも出力の .o は同じ。

| 変更 | -j 1 | -j 4 |
|---|---:|---:|
| 前 | 4650 | 2040 |
| Cranelift の IR の検査を切る (`CRUSTC_CLIF_VERIFY=1` で入れる) | 4140 | |
| 関数の本体の構文木を本体ごとに 1 回だけ読む | 3950 | |
| 番号の表を FxHash に、`Context` / `FunctionBuilderContext` を使い回す | 3810 | |
| クロージャの本体を引くたびに囲む関数を読み直さない (囲む関数ごとにクロージャの式を覚える) | 約 −450 | |
| 式の形のハッシュ (`expr_hash`) を Debug の文字列から木をたどるハッシュに (部分式はアドレスで覚える) | 3740 | |
| キャッシュのキーで、参照先のシグネチャのハッシュを関数ごとに覚える | 3690 | |
| 文字列リテラルの解読で 1 文字ずつ String を作らない (コンパイラに埋め込んだ 286KB の prelude) | 3640 | 1740 |

(計測の揺れは ±200 ms 程度。最初の 3 行は前のコミット)

- 式の形のハッシュは、推論で記録した全部の部分式について Debug の文字列を作っていたので、深さに比例して重くなっていた
  (全体の 9%)。今は木を 1 回たどり、部分式のハッシュを混ぜる (Debug が同じなら同じ値)。
- 残りは Cranelift 自身が約半分 (regalloc2 が 22%、lower 6%、emit 4%)、acg のコード生成が 20%、型推論が 5%。
- `single_pass` の regalloc (`CRUSTC_CLIF_FAST_RA=1`) にすると 3640 → 3050 ms になるが、.o が 43% 大きくなり、
  できたコンパイラの実行が約 40% 遅くなる (lexer_main.rs で 75 → 104 ms)。既定は backtracking のままにした。

### Cranelift に渡す IR を小さくする (2026-11-17)

自分自身 (out/selfhost/compiler_main.rs) を `--no-cache -j 1` でコンパイルしたときの、Cranelift に渡した IR の合計と時間。

| | 前 | 後 |
|---|---:|---:|
| 値 | 837,051 | 785,840 (−6%) |
| スタックのスロット | 74,805 | 62,674 (−16%) |
| 命令 | 972,578 | 997,071 (+3%: スロットのアドレスを使う所で作り直す分) |
| Cranelift の compile の時間 | 1420〜1490 ms | 1270〜1320 ms (−11%) |
| 全体 | 3850〜4020 ms | 3670〜3820 ms (−5%) |
| できたコンパイラの .o | 5.20 MB | 4.54 MB (−13%) |

できたコンパイラで同じファイルをコンパイルする時間は変わらない (2.37〜2.52 s)。bench/runtime.py の実行時間も同じか少し速い。

- 呼び出しの戻り値を書かせた一時領域 (と struct / enum を組み立てた一時領域) を「まだ誰も使っていない」と覚え、値渡しの引数と
  `let` ではコピーせずにそのまま使う (`Cx::fresh` / `take_fresh`。文の終わりで drop する一時値に入っているものは除く)。
- 関数の末尾の式と `return` の式が呼び出しか struct / enum の値なら、その戻り値を関数自身の sret の場所に直接書かせる
  (`Cx::ret_call` / `dst_hint`。引数の中の呼び出しには渡さない)。
- clif バックエンド: スロットのアドレス (`stack_addr`) は使う所で作り直す。生きている区間が短くなり、区間をまたぐ Variable
  (ブロックの引数) にもならない。
- clif バックエンド: 飛び込む命令をすべて書いたブロックはその場で閉じる (seal)。変数の値をその場で決められるので、ブロックの
  引数の仮置きが減る。条件が定数の分岐は jump にする。
- 試してやめたもの: 8 byte の load / store の組が 2 つ続くコピーを 16 byte (xmm) の 1 組にした。命令は 10% 減ったが時間は
  変わらず、8 byte で書いた直後に 16 byte で読むとストアからの転送が効かず、できたコンパイラの実行が 20% 遅くなった。
- 残り: 小さい関数でも Cranelift は 1 関数あたり約 30 µs かかる (10 命令未満の関数 1600 個で 50 ms)。時間の多くは
  regalloc2 の生存区間の計算 (Env::init) と割り当て。
- 調べるための環境変数: `CRUSTC_CLIF_STATS=1` (命令の種類ごとの数)、`CRUSTC_CLIF_DUMP=1` (IR)、`CRUSTC_CLIF_TIME=1`
  (関数ごとの compile の時間)。それぞれの変更は `CRUSTC_NO_FRESH` / `CRUSTC_NO_RET_DST` / `CRUSTC_CLIF_NO_REMAT` /
  `CRUSTC_CLIF_LATE_SEAL` / `CRUSTC_CLIF_NO_FOLD_BR` で切れる。

### check を型検査で止めて LSP を速くする (2026-11-17)

- check (`crustc check` と LSP) は、acg のコード生成 (型の検査はここで出る) までにして、機械語 (Cranelift / minclif) を作らない
  (`Inline::check_only`)。検査済みの印は「要求した総称関数のインスタンスへの再配置だけを持つ空の Blob」で、ビルドの Blob と別の
  キー (`CHECK_SALT`) に覚える (ビルドの Blob があればそれも検査済みとして使う)。`CRUSTC_CHECK_CODEGEN=1` で機械語まで作る。
- prelude の選択 (`select_with`) がソースの全体を 80 回あまり `contains` で調べていたのを、アイテムごとに調べてアイテムの
  ハッシュで覚える形にした (`PreludeScan`)。それまで `SelectCache` を呼ぶたびに作り直していて、アイテムごとの記憶が効いて
  いなかったので Session2 に持たせた (`FrontMemo`)。
- `lexer::split` を差分にした (`split_inc`): 前回の入力と結果を覚え、前後の変わっていない部分のアイテムはそのまま (後ろは位置と
  行をずらして) 使い、間だけを split し直す。区切りは空白の後から始まるトップレベルのアイテム (impl / trait は丸ごと)。間の
  終わりがコメント・文字列の途中かもしれないときと、split が失敗するときは全体を split する。tests/split_inc.rs は実際のソースに
  乱数の編集を続けて加え、毎回全体の split と比べる。`CRUSTC_SPLIT_VERIFY=1` で毎回比べる、`CRUSTC_NO_SPLIT_INC=1` で切る。

| | 前 | 後 |
|---|---:|---:|
| `crustc check` compiler_main.rs (38886 行) | 3.6 s | 1.6 s |
| LSP didOpen / didChange、生成した 1000 関数 (17019 行、stdio 越し) | 135 / 12.2 ms | 122 / 9.6 ms |
| LSP didOpen / didChange、compiler_main.rs (1 関数の編集) | 2.09 s / 523 ms | 1.63 s / 476 ms |
| split (生成した 1000 関数の didChange) | 2.0 ms | 0.6 ms |

残り (compiler_main.rs の didChange 476 ms の内訳): ファイル全体を書き換える前処理 (マクロの展開 97 ms・`mod` の平らにする 63 ms・
入れ子のアイテムの移動 47 ms・コンストラクタの書き換え 33 ms。それぞれがファイル全体を字句解析し直す)、Program と参照 47 ms、
全関数 (9115) のキャッシュのキーと読み戻し 100 ms。どれも全体を作り直しているので、次はアイテムごとの差分にする。

### 前処理をアイテムごとの差分にする (2026-11-17)

前処理 (thread_local! の展開・マクロの展開・`mod` を平らにする・入れ子のアイテムの移動・コンストラクタの書き換え) を
`crates/syntax/src/prep.rs` にまとめ、編集をまたいでチャンクごとに覚えるようにした。

- チャンク: アイテムの境界 (`mod` / `impl` / `trait` の中も) で切った行の範囲。チャンクに切るのも前回との差分
  (前後の変わっていない部分の境界はそのまま、間だけを字句解析し直す)。
- どの前処理も、ファイル全体から集めたもの (マクロの定義・タプル struct とバリアントの要素数・モジュールの木と `use`・入れ子の
  アイテムの名前) を使ってトークンごとに書き換え、行の数を変えない。集めるものはアイテムの宣言にしか依らないので、関数の
  本体の中の式と const / static の初期化式を消した「骨組み」から同じものが集まる (本体の中の入れ子のアイテム・`use`・
  `macro_rules!`・ユーザのマクロの呼び出しは残す。残すもののない本体は括弧ごと消す)。
- 骨組みの全体が前回と同じなら、変わっていないチャンクは前回の結果を使い、変わったチャンクだけを「ほかを骨組みにした
  ソース」(compiler_main.rs で 2.1 MB → 0.31 MB) で前処理して行で切り出す。骨組みが変わったとき (宣言の編集)・括弧が
  釣り合っていないとき・移したアイテムがチャンクをまたぐときは全体を前処理する。
- 1 回目の呼び出しは全体を前処理するだけ (1 度しか前処理しないビルドでチャンクの記録を作らない)。
- tests/prep_inc.rs: examples と crates/core/src のファイルを `mod` で束ねたものに乱数の編集 (本体の中と、どこでも) を
  続けて加え、毎回全体の前処理と本文・桁のずれ・成否を比べる (差分の経路 156 回、全体の経路 364 回)。
  `CRUSTC_PREP_VERIFY=1` で毎回比べる、`CRUSTC_NO_PREP_INC=1` で切る、`CRUSTC_PREP_LOG=1` で経路を出す。
- 乱数の編集のテストで、壊れたソースで入れ子のアイテムの名前の変更が止まらず stack overflow になる hoist の不具合が
  見つかったので直した (変えても同じならやり直さない、やり直しは 8 回まで)。

| compiler_main.rs (38886 行) の 1 関数の編集 | 前 | 後 |
|---|---:|---:|
| 前処理 | 約 270 ms | 28 ms |
| LSP didChange (stdio 越し) | 476 ms | 238 ms |

残り (didChange 238 ms): 全関数 (9115) のキャッシュのキーと読み戻し 約 100 ms、参照 30 ms、Program 18 ms、要約 8 ms、
split 6 ms、prelude の選択 7 ms。

### check のキャッシュのキーと読み戻しを差分にする (2026-11-17)

check は毎回、全関数 (compiler_main.rs で 9115) のキャッシュのキーを作り、キャッシュから読み戻して (要求していたインスタンスを
要求し直して) いた。LSP のセッションでは前回の check の記録 (`ChkMemo`) を持ち、次の条件で使う。

- 宣言の指紋 (`Analysis::iface_fp`): fn アイテムは要約 (シグネチャ・クロージャ) の符号のハッシュ、ほかのアイテムはテキストの
  ハッシュを並べたもの。関数の本体だけの編集では変わらない。キーが参照するもの (参照先のシグネチャ・struct・const・
  名前の表) はどれもこれで決まる。
- 指紋が前回と同じなら、本体のアイテムのハッシュが同じ関数は検査済み (キーも読み戻しも要らない)。要求していたインスタンスは
  名前で辿り、本体 (総称関数) のアイテムが変わったものだけを要求し直して検査する。検査に通らなかったものは毎回検査し直す。
- 指紋が変わったら今までどおりキーとキャッシュで検査し、記録を作り直す。記録を作るのはキャッシュを使うセッション
  (LSP) だけ (1 度だけの `crustc check` では作らない)。`CRUSTC_NO_CHECK_MEMO=1` で使わない。
- tests/check.rs: examples の関数の本体に型エラーになる文・ならない文を足したり消したりを続け、毎回新しいセッションで
  検査したものと診断を比べる。

| compiler_main.rs の 1 関数の編集 | 前 | 後 |
|---|---:|---:|
| check (キーと読み戻し、検査) | 約 100 ms | 37 ms (検査し直すのは 13 関数: 編集した関数と、エラーのある関数) |
| LSP didChange (stdio 越し) | 238 ms | 194 ms |

残り (didChange 194 ms): 前処理 30 ms、参照 29 ms、Program 17 ms、要約 9 ms、prelude の選択 7 ms、split 6 ms、
ほか (診断の JSON、cfg の評価など) 約 60 ms。

### LSP のシンボル検索 (2026-11-17)

`textDocument/documentSymbol` (ファイルのアウトライン) と `workspace/symbol` (名前で探す) に対応した。

- シンボルは crustc-syntax の `symbols` で、字句の列から読む (構文木を作らない)。編集の途中で構文が壊れていても、読める所
  までは出す (本体の `{` は波括弧だけで対応を取り、閉じていなければ次の字下げなしのアイテムの前で切る)。位置は
  エディタのテキストそのもの (前処理の前) の UTF-16 の桁。
- 出すもの: mod / struct / enum / union / trait / impl (`impl Tr for S<T>` の名前) / fn (impl・trait の中は Method) / const /
  static / type / macro_rules!、struct とバリアントのフィールド、enum のバリアント、extern ブロックの中の宣言。detail に
  fn のシグネチャやフィールドの型。関数の本体の中は読まない。
- workspace/symbol は、開いているドキュメントと、initialize の rootUri の下の .rs ファイル (隠しディレクトリ・target・out・
  node_modules を除く) から探す。開いていないファイルは更新時刻と大きさで覚え、変わったものだけ読み直す。
- 合い方の順: 同じ → 前方一致 → 語の頭からの部分一致 → 部分一致 → 文字の順に含む (`nrm` → `norm`)。大文字小文字は区別しない。
  256 個まで。containerName は囲む mod / impl / trait。
- 速さ (このディレクトリをワークスペースにして): 1 回目 76 ms (261 ファイルを読む)、2 回目から 3 ms。
  compiler_main.rs (2.1 MB) のシンボル 2854 個を読むのに 40 ms (結果はドキュメントの本文が変わるまで覚える)。
- tests/lsp.rs の symbols、crates/syntax/src/symbols.rs の単体テスト。

### LSP の比較: rust-analyzer / rust-glancer (2026-11-17、`python3 bench/lsp_compare.py --cold`)

同じ手順で 3 つの LSP サーバを動かす (stdio、サーバからの要求には空で答える):
initialize → initialized → didOpen → workspace/symbol が既知の名前を返すまで 0.1 秒ごとに問い合わせ (ready) →
documentSymbol を 10 回 (中央値) → workspace/symbol を 5 つの問い合わせ × 5 回 (中央値) → 関数を 1 つ足す didChange の直後の
documentSymbol (edit→docsym) と、publishDiagnostics が来るまで (edit→diag、20 秒で打ち切り)。peak RSS はサーバのプロセスの
VmHWM と、括弧の中は子孫のプロセスも足した RSS の最大 (50 ms ごと)。

- 対象 synth: 生成した Cargo のクレート (モジュール 40 個・各 25 関数 + struct / enum / impl、約 4400 行)。開くのは src/m7.rs
- 対象 crustc: このディレクトリ (Cargo のワークスペース、依存に cranelift など)。開くのは crates/core/src/driver2.rs
- rust-analyzer 1.100.0 (rustup の component)。rust-glancer は github.com/rust-glancer/rust-glancer (50dd51a) を
  `cargo build --release -p rust-glancer` でビルドしたもの (`RUST_GLANCER=<バイナリ>` で指定。rust-src が要る)。
  `--cold` は rust-glancer の索引 (target/rust_glancer) を消してから測る

| 対象 | サーバ | init | ready | docsym (数) | wsym (見つかった) | edit→docsym | edit→diag | peak RSS (子も) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| synth | crustc | 2.2 ms | 9.6 ms | 0.6 ms (37) | 0.7 ms (5/5) | 2.0 ms | 1.3 ms | 7.5 MB (5.1 MB) |
| synth | crustc (定義・参照を足した後) | 2.0 ms | 9.7 ms | 0.6 ms (37) | 0.6 ms (5/5) | 1.8 ms | 1.6 ms | 10.7 MB (6.4 MB) |
| synth | rust-analyzer | 18.7 ms | 3243 ms | 1.0 ms (37) | 2.3 ms (5/5) | 8.3 ms | 309 ms | 607 MB (878 MB) |
| synth | rust-glancer | 36.4 ms | 5799 ms | 1.9 ms (38) | 27.0 ms (5/5) | 43.3 ms | - | 16.9 MB (888 MB) |
| crustc | crustc | 2.3 ms | 55.0 ms | 1.8 ms (123) | 3.5 ms (5/5) | 9.7 ms | 7.8 ms | 10.9 MB (9.9 MB) |
| crustc | crustc (定義・参照を足した後) | 1.9 ms | 91.4 ms | 2.1 ms (123) | 1.3 ms (5/5) | 13.0 ms | 10.4 ms | 29.9 MB (29.4 MB) |
| crustc | rust-analyzer | 23.0 ms | 5954 ms | 3.6 ms (124) | 2.0 ms (5/5) | 26.2 ms | - | 1360 MB (2723 MB) |
| crustc | rust-glancer | 130 ms | 10593 ms | 7.5 ms (113) | 262 ms (5/5) | 48.6 ms | - | 30.7 MB (1798 MB) |

読み方の注意:
- 機能の幅が違う。rust-analyzer はクレートの依存まで名前解決・型推論をして、補完・定義へのジャンプなどもできる。
  rust-glancer は依存まで索引を作る (ディスクに置く) 軽量版。crustc lsp は開いたファイルを 1 つのクレートとして検査し
  (Cargo の依存・`mod x;` のほかのファイルは読まない)、シンボルは字句の列から読むだけ。crustc の ready が速いのは、依存も
  型の索引も作らず、ワークスペースの .rs のシンボルを読むだけだから。
- edit→diag: crustc は開いたファイルだけの検査の結果 (driver2.rs 単体なのでエラーが出る)。rust-analyzer は自前の診断を
  push する (crustc の対象では 20 秒の間に来なかった)。rust-glancer の診断は既定で切ってある (cargo check を設定で入れる)。
- edit→docsym は didChange を受けた後の処理を含む (crustc は didChange のたびに check するので、その時間も入る)。
- rust-analyzer と rust-glancer は子プロセス (cargo metadata・rustc・索引の作業) が大きい。rust-glancer は本体は 17〜31 MB。
- シンボルの数の違い: rust-glancer は impl の中の関連アイテムなどの数え方が違う (synth の 38 は impl の名前の出し方)。

### LSP の機能の比較 (2026-11-17、`python3 bench/lsp_parity.py`)

速さの比較 (上の lsp_compare) は何を返すかを見ない。同じ対象・同じ位置で 3 つのサーバに問い合わせ、rust-analyzer の答えと
比べる: documentSymbol の名前の集合、workspace/symbol の上位 20 の一致、ファイルの中の識別子 40 か所での definition の行き先・
hover・references の数、`.` の直後 (`x.|name`) と識別子の途中 (`na|me`) の completion にそこに書いてある名前が入るか。

#### synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 100% (種類も 97%) | 名前 100% (種類も 100%) | 名前 100% (種類も 95%); 余分 #0 |
| workspace/symbol (上位 20 の一致) | 87% | 100% | 76% |
| definition (行き先の一致) | 同じ 0% (答えた 0%) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) |
| hover (名前を含む) | 0% | 100% | 100% |
| references (数の一致) | 0% | 100% | 100% |
| completion `x.\|` (書いてある名前を含む) | 0% (候補 0) | 100% (候補 3) | 100% (候補 5) |
| completion `na\|me` (同) | 0% (候補 0) | 91% (候補 145) | 56% (候補 5) |
| 時間の中央値 (ms) | defi 0.1, hove 0.1, comp 0.1, refe 0.1 | defi 0.5, hove 0.6, comp 4.1, refe 1.0 | defi 7.7, hove 8.4, comp 234.5, refe 20.3 |

#### crustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 99% (種類も 97%); 足りない extern "C" | 名前 100% (種類も 100%) | 名前 90% (種類も 82%); 足りない c_close, c_exit, c_fork, c_mkdir |
| workspace/symbol (上位 20 の一致) | 61% | 100% | 35% |
| definition (行き先の一致) | 同じ 0% (答えた 0%) | 同じ 100% (答えた 78%) | 同じ 87% (答えた 68%) |
| hover (名前を含む) | 0% | 88% | 72% |
| references (数の一致) | 0% | 100% | 42% |
| completion `x.\|` (書いてある名前を含む) | 0% (候補 0) | 100% (候補 26) | 90% (候補 37) |
| completion `na\|me` (同) | 0% (候補 0) | 68% (候補 85) | 60% (候補 3) |
| 時間の中央値 (ms) | defi 0.1, hove 0.1, comp 0.1, refe 0.1 | defi 0.9, hove 1.0, comp 7.7, refe 31.4 | defi 14.4, hove 12.3, comp 257.3, refe 325.4 |

分かったこと:
- (この時点では) crustc lsp が持っているのは診断と documentSymbol / workspace/symbol だけ。definition・hover・references・completion
  (rust-analyzer と rust-glancer は持っている) が無いので、速さの比較は同じ機能での比較になっていない。
- シンボルはおおむね同じ。documentSymbol の違いは extern ブロックの扱い (crustc は中の宣言を外に並べる、rust-analyzer は
  `extern "C"` を 1 つのシンボルにする) と種類の付け方。workspace/symbol の違いは並べ方 (問い合わせに合う候補の選び方と
  順) で、上位 20 の一致は 61〜87%。
- rust-glancer は definition・hover・completion を持つが、crustc の対象では rust-analyzer と違う答えも多い (references の
  数の一致 42%)。completion は 1 回 250 ms ほど。

### LSP の定義ジャンプと参照 (2026-11-17)

`textDocument/definition` と `textDocument/references` に対応した (crustc-syntax の `navigate`)。型推論はせず、字句の列と
アイテムの表 (symbols を平らにして、持ち主の型・モジュールのパスを付けたもの) で名前を解決する:

- ローカル: 関数の引数・`let` (`;` の後から見える)・`for` / `if let` / `while let` (次の `{` から)・match の腕・クロージャの引数。
- `x.name`: x の型 (`self` なら囲む impl、ローカルなら型注釈か `T::..` / `T { .. }` / `T(..)` の初期化) のメソッド・フィールド。
  型が分からないときは、同じ名前のメンバが一つだけのときだけ答える。
- `Q::name`: Q のメンバか、Q というモジュールの中のアイテム。`Self` は囲む impl の型。
- アイテム: 同じモジュール → `use` (名前・glob・glob で取り込んだモジュールの use、`as` の別名、`use` の中の名前そのもの) →
  ワークスペースで一つ → std。std の再エクスポート (`std::sync::Mutex` → sync::poison::mutex) は `pub use` をたどる。
  `#[derive(X)]` の X は derive のマクロ。組み込みの型は答えない。
- ワークスペース: initialize の root の Cargo.toml (members) から、各クレートの根 (lib / main / bin / tests / examples /
  benches / build.rs) と `mod x;` (`#[path]`・インラインの mod も) をたどってファイルのモジュールのパスを決める。tests などの
  クレートのアイテムは同じクレートからだけ見える。
- std / core / alloc (`rustc --print sysroot` の rust-src) は initialize の後に別スレッドで索引を作る (定義と use だけを持ち、
  本文は捨てて位置は行と桁で覚える: 約 7 MB)。`CRUSTC_LSP_NO_STD=1` で切る。
- references は、同じ名前の識別子 (ファイルごとの名前の表で引く) のうち、definition が同じ所に解決されるもの。
  `macro_rules!` の本体の中は数えない (rust-analyzer と同じ)。
- メモリ: ワークスペースの全ファイルの字句の列 (u32 で持つ) と名前の表を持つので、crustc の対象で peak RSS 10.9 → 29.9 MB
  (std の索引を切ると 27.5 MB)。ready も 55 → 91 ms (ワークスペースの索引を作る)。

`python3 bench/lsp_parity.py` (rust-analyzer の答えと比べる。`--detail` で合わない所を出す):

#### synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 100% (種類も 97%) | 名前 100% (種類も 100%) | 名前 100% (種類も 95%); 余分 #0 |
| workspace/symbol (上位 20 の一致) | 87% | 100% | 76% |
| definition (行き先の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) |
| hover (名前を含む) | 0% | 100% | 100% |
| references (数の一致) | 100% | 100% | 100% |
| completion `x.\|` (書いてある名前を含む) | 0% (候補 0) | 100% (候補 3) | 100% (候補 5) |
| completion `na\|me` (同) | 0% (候補 0) | 91% (候補 145) | 56% (候補 5) |
| 時間の中央値 (ms) | defi 0.8, hove 0.2, comp 0.1, refe 5.8 | defi 0.4, hove 0.5, comp 2.5, refe 1.0 | defi 6.3, hove 6.3, comp 218.4, refe 18.8 |

#### crustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 99% (種類も 97%); 足りない extern "C" | 名前 100% (種類も 100%) | 名前 90% (種類も 82%); 足りない c_close, c_exit, c_fork, c_mkdir |
| workspace/symbol (上位 20 の一致) | 73% | 100% | 35% |
| definition (行き先の一致) | 同じ 97% (答えた 80%) | 同じ 100% (答えた 78%) | 同じ 87% (答えた 68%) |
| hover (名前を含む) | 0% | 88% | 72% |
| references (数の一致) | 64% | 100% | 42% |
| completion `x.\|` (書いてある名前を含む) | 0% (候補 0) | 100% (候補 26) | 90% (候補 37) |
| completion `na\|me` (同) | 0% (候補 0) | 68% (候補 85) | 60% (候補 3) |
| 時間の中央値 (ms) | defi 1.3, hove 0.3, comp 0.1, refe 13.6 | defi 0.8, hove 1.1, comp 6.6, refe 27.9 | defi 15.4, hove 13.0, comp 243.6, refe 315.0 |

- definition は synth で 100%、crustc で 97% (合わないのは、cfg で外れたアイテム `Backend` に答える所 (rust-analyzer は
  答えない) と、`FxMap::default()` を型の別名の先 (HashMap の impl) までたどれない所)。rust-glancer は 87%。時間は 1 回 1 ms ほど。
- references (数がちょうど同じ) は synth で 100%、crustc で 64% (rust-glancer 42%)。合わないのは主に型推論が要るもの:
  `Default::default()` の呼び出し先 (impl Default のメソッド)、`FxMap::default()` (型の別名をたどった先の std のメソッド)、
  `x.f()` の受け手の型がローカルの注釈から分からないもの。ほかは数が 1〜3 ずれるもの (`Ty` 2290 個など)。組み込みの型
  (`u8`) は rust-analyzer が参照を返すが crustc は返さない。時間は crustc の対象で 1 回 10〜14 ms (rust-analyzer 28 ms)。
- hover と completion は未対応。

### LSP の hover と補完、大きいワークスペースのメモリ (2026-11-17)

`textDocument/hover` と `textDocument/completion` (`.` と `:` で起動) に対応した。どちらも定義・参照と同じ索引
(crustc-syntax の `navigate`) から引く。型検査は使わない。

- hover: 定義のシグネチャ (属性を外し、本体の `{` の前まで)・囲むもの (impl / モジュール)・その前の `///` の doc コメント。
  ローカルは推定した型 (`q: Point`)、組み込みの型はその名前。
- 補完:
  - `x.|`: 受け手の型のフィールドとメソッド。
  - `Q::|`: 型のメンバとモジュールのアイテム。re-export した名前も入れる (`std::sync::Mu|`)。
  - それ以外: 見えるローカル・ワークスペースのアイテム・use した名前・prelude・よく使うマクロ・予約語。
  - 打った途中までで絞る: 前方一致・語の頭。ローカルは文字の順でも合わせる。
- 受け手の型の推定 (文字列で持つ):
  - たどれるもの: ローカルの型注釈・`let` の初期化の式・引数・`self`、フィールドの型、メソッドと関数の戻り値。
  - impl の型引数は受け手の引数で置き換える。例: `Option<Sig>` の `unwrap` → `Sig`、`HashMap<K, V>` の `get` → `Option<&V>`。
  - 式の形: 添字 (`Vec<T>[i]` → T、map → V)、`?`、`for x in v`、`if let Some(x) = ..`、`vec![]`、構造体の式、`as T`。
  - 型の別名は型引数ごと展開する (`LockResult<G>` → `Result<G, ..>`)。Deref は包みの中身を見る (`MutexGuard<T>` → T)。
  - impl / derive された trait のメソッド (`clone`、`Iterator` の `map` など) も見る。
  - `..Default::default()` と `field: Default::default()` は、構造体の式の型・フィールドの型の `default` にする。
- std の索引で直したところ:
  - `pub const unsafe fn` と `const trait` を読めていなかった (core の `impl str` のメソッドの大半が落ちていた)。
  - `sys`・テスト・ほかの OS 向けのファイルを入れない。
  - 位置は定義に、モジュールのパス・囲むものは共有して持つ: 約 21 MB → 10 MB。
- 大きいワークスペース (`CRUSTC_LSP_MEM_MB`、既定 4):
  - ワークスペースの本文と字句の列は、合計がこの大きさまでのファイルの分だけ持つ。
  - 超えた分のファイルは slim にして、定義・`pub use`・出てくる名前のハッシュの集合だけを持つ。
  - 参照・hover で要るときは、名前のハッシュで選んだファイルだけ読み直す。読み直したものは同じ大きさまで LRU で覚える。
  - `CRUSTC_LSP_MEM_MB=0` (全部 slim) でも答えは変わらない (parity は同じ)。参照は遅くなる: crustc の対象で 1 回 9 → 31 ms。

`python3 bench/lsp_parity.py`:

#### synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 100% (種類も 97%) | 名前 100% (種類も 100%) | 名前 100% (種類も 95%); 余分 #0 |
| workspace/symbol (上位 20 の一致) | 87% | 100% | 76% |
| definition (行き先の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) |
| hover (名前を含む) | 100% | 100% | 100% |
| references (数の一致) | 100% | 100% | 100% |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 4) | 100% (候補 3) | 100% (候補 5) |
| completion `na\|me` (同) | 97% (候補 25) | 91% (候補 145) | 56% (候補 5) |
| 時間の中央値 (ms) | defi 0.9, hove 0.8, comp 1.0, refe 3.5 | defi 0.4, hove 0.6, comp 3.1, refe 1.0 | defi 6.7, hove 6.7, comp 224.6, refe 18.9 |

#### crustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 99% (種類も 97%); 足りない extern "C" | 名前 100% (種類も 100%) | 名前 90% (種類も 82%); 足りない c_close, c_exit, c_fork, c_mkdir |
| workspace/symbol (上位 20 の一致) | 73% | 100% | 35% |
| definition (行き先の一致) | 同じ 100% (答えた 80%) | 同じ 100% (答えた 78%) | 同じ 87% (答えた 68%) |
| hover (名前を含む) | 85% | 88% | 72% |
| references (数の一致) | 67% | 100% | 39% |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 40) | 100% (候補 26) | 90% (候補 37) |
| completion `na\|me` (同) | 80% (候補 7) | 68% (候補 85) | 60% (候補 3) |
| 時間の中央値 (ms) | defi 1.0, hove 1.0, comp 1.3, refe 8.8 | defi 1.0, hove 1.1, comp 7.0, refe 30.9 | defi 16.0, hove 13.9, comp 265.5, refe 388.4 |

- crustc の対象で、hover 85% (rust-analyzer 88%) と `x.|` の補完 100% (候補の数の中央値 40、rust-analyzer 26)。
  rust-glancer はそれぞれ 72% と 90% (候補 37)。
- hover で合わないのは文字列の中の名前 (`"CRUSTC_INLINE_LOG"`) の 2 か所だけ。
- `na|me` は、rust-analyzer がほかの候補を多く出す (中央値 85 個) ため、そこに書いてある名前が入らないことがある (68%)。
  crustc は絞るので 80% (候補 7)。この指標で上回っても、補完の質が上とは言えない。
- definition は 100% になった (`FxMap::default()` を別名の先の HashMap の impl にした)。references は 67%。残りの多くは、
  1〜3 個ずれる数え方の違いと、組み込みの型 (`u8`) の参照。
- 時間 (1 回、crustc の対象): definition / hover 1.0 ms、補完 1.3 ms、参照 9 ms。rust-analyzer は 1.0 / 1.1 / 7.0 / 31 ms。

`python3 bench/lsp_compare.py --cold` (参照の列を足した。big はモジュール 1000 個・約 11 万行・4.5 MB の生成クレート):

| 対象 | サーバ | init | ready | docsym (数) | wsym (見つかった) | edit→docsym | edit→diag | references 1 回目 / 2 回目 (数) | peak RSS (子も) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| synth | crustc | 4.9 ms | 14.6 ms | 0.8 ms (37) | 0.9 ms (5/5) | 2.1 ms | 1.5 ms | 207 ms / 3.5 ms (29) | 16.9 MB (15.8 MB) |
| synth | rust-analyzer | 23.9 ms | 3398 ms | 1.4 ms (37) | 1.0 ms (5/5) | 19.7 ms | 299 ms | 84.0 ms / 3.2 ms (29) | 596 MB (865 MB) |
| synth | rust-glancer | 26.0 ms | 5812 ms | 1.9 ms (38) | 31.2 ms (5/5) | 4.7 ms | - | 380 ms / 14.6 ms (29) | 15.7 MB (888 MB) |
| crustc | crustc | 2.5 ms | 86.7 ms | 1.8 ms (123) | 1.2 ms (5/5) | 13.6 ms | 10.9 ms | 71.9 ms / 10.0 ms (36) | 30.1 MB (30.1 MB) |
| crustc | rust-analyzer | 25.7 ms | 6653 ms | 4.4 ms (124) | 1.1 ms (5/5) | 12.3 ms | - | 1698 ms / 15.1 ms (36) | 1571 MB (1633 MB) |
| crustc | rust-glancer | 106 ms | 11194 ms | 7.7 ms (113) | 265 ms (5/5) | 48.0 ms | - | 1702 ms / 122 ms (36) | 29.8 MB (1799 MB) |
| big | crustc | 2.4 ms | 152 ms | 0.8 ms (37) | 14.4 ms (5/5) | 3.7 ms | 3.1 ms | 47.6 ms / 16.6 ms (29) | 55.1 MB (55.1 MB) |
| big | rust-analyzer | 20.1 ms | 4293 ms | 5.7 ms (37) | 14.7 ms (5/5) | 5.2 ms | 4875 ms | 487 ms / 7.1 ms (29) | 825 MB (2038 MB) |
| big | rust-glancer | 35.5 ms | 6206 ms | 1.9 ms (38) | 108 ms (5/5) | 5.0 ms | - | 926 ms / 29.4 ms (29) | 22.0 MB (1033 MB) |

- 参照の 1 回目は std の索引 (initialize の後に別スレッドで作る) を待つ時間を含む: synth で 207 ms。
  rust-analyzer と rust-glancer の 1 回目 (0.5〜1.7 秒) よりは短い。
- peak RSS:
  - crustc の対象: 定義・参照を入れた直後の 29.9 MB から 30.1 MB (hover・補完のための型の情報・std の impl を足しても、
    std の索引を小さくした分で相殺)。
  - big: 55 MB (`CRUSTC_LSP_MEM_MB=0` で 28 MB)。
  - rust-analyzer は 0.6〜1.6 GB。rust-glancer は本体 16〜30 MB だが、子プロセスを足すと 0.9〜1.8 GB。

### LSP を crustc から分ける (2026-11-17)

- symbols と navigate を crustc-syntax から新しいクレート crustc-ide に移した (コンパイラの層にエディタ向けのコードを
  置かない。crustc 自身でコンパイルする層 (base〜core) にも入らない)。
- crustc-lsp をバイナリ `crustc-lsp` にした。`crustc lsp` は無くし、案内を出して終わる。crustc (CLI とライブラリ) は
  crustc-ide / crustc-lsp に依存しない (`cargo tree -p crustc` に出ない)。ベンチの lsp/* だけが dev-dependency で使う。
- LSP のテストは crates/lsp/tests/lsp.rs に移した。

### references の残りの不一致を詰める (2026-11-17)

`python3 bench/lsp_parity.py --detail` の合わない所を見て、数え方を rust-analyzer に合わせた。

- 型の参照に、その型の impl の中の `Self` も入れる (`impl Enc for Vec<T>` の `-> Option<Self>` は Vec の参照)。
- パターンと引数の束縛 (`let (a, concrete) = ..` / `fn f(concrete: T)` / `for (i, x) in` / `|a, b|`) はその束縛自身にする。
  前は、同じ名前の関数 `concrete` の参照に数えていた。
- `Q::name` の Q が知らない名前 (外部クレートの `syn::Pat`、`use proc_macro2::Delimiter` の `Delimiter::None`) なら答えない。
  前は `None` を Option の None に、`Pat` をワークスペースの Pat にしていた。
- cfg:
  - `#[cfg(..)]` で外れるアイテムは、定義から除き、中の名前も数えない。カーソルがその中にあれば答えない。
  - feature は Cargo.toml の `[features]` の default から、クレートごとにたどる。
  - ほかに分かるもの: test、debug_assertions、unix / windows、target_os と all / any / not。
- 組み込みの型 (`u8` など) の参照: ワークスペースの同じ名前の型の位置。`0u8` の接尾辞・`fn u8` の名前・`x.u8` は除く。

#### synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 100% (種類も 97%) | 名前 100% (種類も 100%) | 名前 100% (種類も 95%); 余分 #0 |
| workspace/symbol (上位 20 の一致) | 87% | 100% | 76% |
| definition (行き先の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) |
| hover (名前を含む) | 100% | 98% | 100% |
| references (数の一致) | 100% | 100% | 100% |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 4) | 100% (候補 3) | 100% (候補 5) |
| completion `na\|me` (同) | 97% (候補 25) | 91% (候補 145) | 56% (候補 5) |
| 時間の中央値 (ms) | defi 0.8, hove 0.7, comp 1.0, refe 3.7 | defi 0.5, hove 0.7, comp 3.7, refe 1.8 | defi 7.5, hove 7.6, comp 235.4, refe 19.8 |

#### crustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 99% (種類も 97%); 足りない extern "C" | 名前 100% (種類も 100%) | 名前 90% (種類も 82%); 足りない c_close, c_exit, c_fork, c_mkdir |
| workspace/symbol (上位 20 の一致) | 73% | 100% | 35% |
| definition (行き先の一致) | 同じ 100% (答えた 78%) | 同じ 100% (答えた 78%) | 同じ 87% (答えた 68%) |
| hover (名前を含む) | 82% | 88% | 72% |
| references (数の一致) | 78% | 100% | 39% |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 40) | 100% (候補 26) | 90% (候補 37) |
| completion `na\|me` (同) | 80% (候補 7) | 68% (候補 85) | 60% (候補 3) |
| 時間の中央値 (ms) | defi 1.1, hove 1.0, comp 1.3, refe 16.6 | defi 0.9, hove 1.2, comp 6.2, refe 28.3 | defi 14.9, hove 12.5, comp 252.8, refe 330.1 |

- crustc の対象の references (数がちょうど同じ) は 67% → 78%。
- 残りの 9 か所は、どれも rust-analyzer 側の事情。
  - cfg で外れた `#[cfg(not(feature = "build"))] fn lower_blob` の中:
    - rust-analyzer は中の名前を数える (Vec・Analysis で 1〜2 個)。
    - 同じ cfg の `type Backend` には答えない。
  - `u8`: rust-analyzer は target/ の下に生成した prelude_items.rs (`include!`) の中も数える。
  - `Key`: build.rs の中の `mod key { pub type Key = u128; }` を、rust-analyzer は crates/base の Key の参照に数える。
- 時間: 参照は 1 回 17 ms (前は 9 ms。impl の中の `Self` も解決するため)。rust-analyzer は 28 ms。
- `CRUSTC_LSP_MEM_MB=0` (全部 slim) でも同じ答え。
- hover は 85 → 82%: cfg で外れた `Backend` に答えなくなったため (rust-analyzer も答えない)。

### ミューテーションテスト (cargo-mutants、2026-11-17)

`cargo install cargo-mutants --locked` (27.1.0) を入れ、`bench/mutants.sh` で回す。

- 設定と範囲:
  - 設定は `.cargo/mutants.toml`。ビルドは `profile.mutants` (release と同じ最適化に増分ビルド)。
  - 変異を入れないもの: CLI の引数の処理・build.rs・テストとベンチのコード・Debug / Display。
  - 変異は全部で 18,240 個。1 個にテストで数秒〜1 分かかるので、全部は回さない。
- 範囲の絞り方:
  - `bench/mutants.sh diff [BASE]`: BASE (既定 origin/main) からの差分の行の変異だけ。変更の確認に。
  - `bench/mutants.sh pkg crustc-ide`: クレートごと。
  - `bench/mutants.sh file crates/ide/src/symbols.rs -- -F match_score -j 2`: ファイル・関数の名前・並列の数を指定する。
  - `bench/mutants.sh list ...`: 数を見るだけ。
  - `--` の後ろは cargo mutants にそのまま渡す (`--shard 1/8` など)。
- テストに使うパッケージ:
  - crates/ide・crates/lsp だけの変異: crustc-ide と crustc-lsp のテスト。1 個 2〜3 秒。
  - それ以外: ワークスペース全部のテスト (コンパイラの層のテストの多くはルートの tests/ にある)。1 個 1 分ほど。
- 時間切れは、基準のテストの時間の 3 倍か 180 秒の大きい方。
- 結果は `mutants.out/` (missed.txt がテストで見つからなかった変異)。

試した結果:

| 対象 | 変異 | 見つかった | 見つからない | 時間 (-j 2) |
|---|---:|---:|---:|---:|
| `match_score` (ide) | 25 | 16 → 24 | 9 → 1 | 87 秒 |
| `KeyState::done` (base) | 7 | 5 | 2 | 5 分 |

- `match_score` の見つからなかった 9 個は語の頭の判定 (`_` 以外の区切り) で、テストが `_` しか見ていなかった。
  camelCase・`::`・空白・大文字が続く名前の場合を足した。
- 残りの 1 個 (`i > 0` → `i >= 0`) は等価な変異: i = 0 は前方一致で先に返るので、そこに来ない。
- `KeyState::done` の 2 個は、ハッシュの混ぜ方 (`^` → `|`、`<<` → `>>`)。キャッシュのキーは同じ入力で同じ値になれば
  動くので、機能のテストでは見えない (衝突しにくさの劣化)。

### ide の見つからなかった変異を潰す (2026-11-17)

ide (crates/ide) 全体の 3,142 個を `-j 3` で回し、2 時間で打ち切った (1,897 個まで)。結果は見つかった 1,165・
見つからない 668・時間切れ 44・ビルドできない 20。見つからない 668 個は全部 navigate.rs で、多いのは
ローカルの束縛 (`live` 79・`arm_binding` 48・`local_type` 48・`pat_binding` 20・`fn_param` 19)、`def_at` 60、
use の木 (`use_tree` 27・`resolve_use` 18・`reexport` 18)、文字列の小さな部品 (`targs`・`generic_names`・`captures`・
`cfg_pred`・`macro_bodies` など)。ゴールデン (fixture) はよくある書き方しか通らず、境界や分岐の片側を見ていなかった。

- 足したテスト: `crates/ide/src/navigate_tests.rs` (`#[path]` で navigate の子のモジュールにして、非公開の部品を直接呼ぶ)。
  - 文字列の部品: `captures` / `generic_names` / `targs` / `subst` / `alias_parts` / `ret_type` / `elem_type` /
    `index_type` / `type_head` / `name_hash` (FNV-1a の既知の値) / `mod_score` / `use_module` / `lib_rank`。
  - cfg: `any` / `all` / `not` / `target_os` / `target_family` / `unix` / `windows` / `miri` / 知らない述語。
  - `macro_rules!` の本体の範囲 (`{}` / `()` / 関数の中 / 閉じていないもの)、use の木 (入れ子・`as`・glob・`self`)。
  - ローカル: let / for / if let / while let / match の腕・ガード / or パターン / クロージャ / 引数のパターン /
    ブロックの中の束縛の寿命。束縛の判定 (`pat_binding` / `fn_param` / `arm_binding`) を正と負の両方で。
  - 型: 注釈 (`&'static P`・`&mut a::b::P`)・`T(..)`・`T { .. }`・`U::new()`・`a::b::P`。
  - derive のメソッド 16 個 (`clone` / `default` / `eq` / `ne` / `fmt` / `partial_cmp` / `lt` .. / `hash`)。
  - 書式の取り込み `{x}` / `{y:?}` / `{{x}}` の定義と参照、型引数 (fn / struct / impl / trait / enum / type / union)。
- テストで見つかった不具合 (直した):
  - match のガード `S { f: g } if g > 0 =>` の g が解決されなかった (束縛を `=>` で入れていた)。`if` がガードかを
    先読みし (`guard_arrow`)、ガードから見えるようにした。
  - `let v = a::b::P;` の v の型が `b` になっていた (モジュールの名前を型にしていた)。`a::b::T(..)` / `m::f()` も同じ。
  - 型注釈 `&'static P` の型が `static` になっていた (字句では寿命は `'` の後の識別子)。
  - or パターン `(m, 0) | (0, m)` の m が後ろの方に行っていた。rust-analyzer と同じく前の方に。
  - `apply_cfg` で、cfg で外れたアイテムの derive を数え直すときに、外れた範囲をまだ入れていなかった。
  - LSP がファイルの直下の `#[path = "x.rs"] mod m;` を `<ファイル名>/x.rs` で探していた (rustc はファイルのある
    ディレクトリから)。これで navigate_tests.rs が読まれず、crustc の参照の一致が 78% → 58% に下がって見えていた。
- 速さ: `python3 bench/lsp_parity.py` (crustc を対象) は defi 1.0 / hove 0.9 / comp 1.2 / refe 15.1 ms で、前と同じ
  (rust-analyzer は 0.9 / 0.9 / 5.6 / 27.3 ms)。一致も前と同じ (definition 100%・hover 82%・references 78%)。
  残りの references の違いは、rust-analyzer が数える cfg で外れたアイテムの中・OUT_DIR の生成ファイル・build.rs と
  共有する lexer.rs の中のもの。

テストを足した後、上の部品の関数 (37 個) の変異 967 個に絞って回し直した (`-F` で関数の名前、`-j 3` で 82 分。
見つからなかったものは、テストをさらに足して `--iterate` で回し直した):

| | 見つかった | 見つからない | 時間切れ |
|---|---:|---:|---:|
| 足す前 (全体の途中までの回の、同じ関数の分) | — | 441 | — |
| 1 回目 (967 個) | 715 | 213 | 39 |
| 2 回目 (残りの 249 個を `--iterate`) | 749 | 179 | 39 |

- 時間切れは `+=` → `-=` などで終わらなくなったもの (テストが止まるので見つかったのと同じ)。
- 残りの 179 個のほとんどは結果の変わらない変異:
  - 境界の `<` → `<=` (`sym` / `ident` が範囲の外を偽にするので同じ)。
  - 早い打ち切り (`arm_binding` / `pat_binding` の最初の除外、`guard_arrow` の `{` / `;` での打ち切り): 後ろの走査や
    `arm_pat` の範囲の確認でも同じ答えになる。速さのためのもの。
  - `generic_names` の `>` を探す走査: 結果は `targs` がもう一度区切るので同じ。ファイルの残り全部を写さないためのもの。

### 途中で分かったこと

- **cold 時に要約用の syn AST を全部保持していたため、5000 関数で RSS が 162MB になっていた**。
  要約の parse を並列にし、AST をスレッド内で捨てるようにした結果、RSS は 35MB、cold は 855ms から 526ms になった。
- **初版は syn でファイル全体を parse していた**。そのため no-op でも 114ms かかり、cold では並列スレッドがそれぞれ全体を parse し直して RSS が 191 MiB になっていた。
  自前 lexer と要約キャッシュに変えた結果、no-op は 12ms、cold の RSS は 39 MiB になった。
- **リンクは cc ドライバのプロセス段数が支配的だった**。cc + lld は 21ms、ld.lld の直接起動は 17ms、wild の直接起動は 5ms。
- Cranelift の `single_pass` regalloc はコンパイルは速いが、実行は約 40% 遅くなる (上の表)。

## 次にやること

- **言語**
  - モジュール、`DoubleEndedIterator`（値としての `rev`）、`HashSet` / `BTreeMap`
  - 単相化をレイアウトでキーにして共有する（`Vec<A>` と `Vec<B>` を 1 つにする。blaze の polymorph）
  - 最小の自前 std（relax 方式）
- **インクリメンタル**
  - α 正規化ハッシュ
  - 依存するクレートの解析結果 (要約・Program の一部) を使う側で使い回す (今はソースを解析し直す)
  - 変わった .o だけを差し替えるリンク
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
