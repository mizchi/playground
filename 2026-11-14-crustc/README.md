# crustc

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
./target/release/crustc examples/hello.rs -o out/hello --time
./out/hello
```

```bash
./target/release/crustc check examples/traits.rs
```

型検査だけを行い、全関数の診断を表示する。

```bash
./target/release/crustc lsp
```

LSP サーバ（stdio）。対応は initialize / didOpen / didChange（全文同期）/ didClose / shutdown で、publishDiagnostics を返す。

| オプション | 意味 |
|---|---|
| `-o PATH` | 出力パス。同じ場所に `PATH.o`、`PATH.crcache`（キャッシュ）、`PATH.link`（リンク行）を置く |
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

`examples/*.rs` を crustc と rustc の両方でビルドし、実行結果が一致するかを確かめる。

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
| `lsp/did_change_1fn` | LSP 応答性：1 関数編集の didChange から publishDiagnostics まで | 3.9 ms |
| `lsp/did_change_noop` | 内容が同じ didChange（保存など） | 0.19 ms |
| `cold/lsp_did_open` | 初めて開いたとき（全関数を検査） | 41 ms |
| `build/noop` | 無変更ビルド（リンク省略） | 7.1 ms |
| `build/edit_1fn` | 1 関数編集ビルド（リンク込み） | 28 ms |
| `cold/build` | キャッシュなしのビルド | 84 ms |
| `front/lex` | 字句解析だけ（no-op の下限） | 1.7 ms |

（値はマシンの状態で変わる。比べるときは同じ時間帯に、ベースラインのコミットと並べて測る。`build/edit_1fn` はリンク（この環境では ld.lld）が大半を占める）

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
| `src/infer.rs` | 関数単位の型推論（union-find。参照・クロージャの形も単一化する） |
| `src/closure.rs` | クロージャの持ち上げ：キャプチャの抽出（スコープを辿る自由変数解析）と、本体を fn にする合成 |
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

### 途中で分かったこと

- **cold 時に要約用の syn AST を全部保持していたため、5000 関数で RSS が 162MB になっていた**。
  要約の parse を並列にし、AST をスレッド内で捨てるようにした結果、RSS は 35MB、cold は 855ms から 526ms になった。
- **初版は syn でファイル全体を parse していた**。そのため no-op でも 114ms かかり、cold では並列スレッドがそれぞれ全体を parse し直して RSS が 191 MiB になっていた。
  自前 lexer と要約キャッシュに変えた結果、no-op は 12ms、cold の RSS は 39 MiB になった。
- **リンクは cc ドライバのプロセス段数が支配的だった**。cc + lld は 21ms、ld.lld の直接起動は 17ms、wild の直接起動は 5ms。
- Cranelift の `single_pass` regalloc は、開発ビルドならこれで十分。

## 次にやること

- **言語**
  - モジュール、`DoubleEndedIterator`（値としての `rev`）、`HashSet` / `BTreeMap`
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
