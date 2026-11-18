# TODO: rust-analyzer との残りの違い

`bench/lsp_parity.py` と、std のメソッドの references を 1 件ずつ比べた結果 (README の「std のメソッドの参照の差を詰める」)
から残ったもの。

## 型推論

- [ ] `&*ptr` の型 (生ポインタの参照外し `*const T` → T と、その前の `&`)。
  例: `crates/sema/src/ast_infer.rs` の `let tf = unsafe { &*tf };` の後の `tf.iter()`
- [ ] 関連 const の型 (`usize::BITS` → u32)。例: `(usize::BITS).to_string()`
- [ ] std の関数の引数のクロージャの引数の型が、エラーの型などで決まるもの
  (`std::fs::write(..).map_err(|e| e.to_string())` の e は `io::Error`)
- [ ] パターンの束縛の型を、束縛そのものの位置でも答える (`crustc/typeAt` は使う側の位置でしか答えない)
- [ ] `match` の腕のタプルのパターン `(TokenTree::Literal(x), TokenTree::Literal(y))` の x / y
- [ ] `p.as_object_mut().unwrap()` のような、依存クレートの型 (serde_json の `Value`) のメソッドの戻り値の鎖
- [ ] 数の型の値 `k.to_string()` (for の変数が範囲 `0..n` のとき、要素の型が決まらないもの)
- [ ] str の `Split<'_, P>` の `next` の定義の行き先 (slice の `Split` の impl と取り違える。str の繰り返しの impl は
  core のマクロ `generate_pattern_iterators!` の中にある)

## 定義・参照

- [ ] rust-analyzer は `"x".to_string()` を特殊化の impl (`impl ToString for str`) でも数える (string.rs の別の行)。
  crustc は trait の宣言だけ
- [ ] `ast_summary.rs` の `s.starts_with(k)` (k: &&str) を crustc だけが `str::starts_with` に数える (rust-analyzer は
  別の解決)
- [ ] crustc の target で references の数の一致が 97% (rust-analyzer 100%) の 1 か所
- [ ] fixture: definition 465/467、references 609/629 の残り

## rename

- [ ] `Default::default` の rename (rust-analyzer は依存クレートの中まで書き換える。今はワークスペースだけ: 意図した違い)
- [ ] `use a::Point as Pt` の別名の rename (意図した違い)

## 速さ・メモリ

- [ ] references の p90 (crustc の target で 65 ms) の遅いもの: 名前の多いメソッドの全ファイルの解決
- [ ] 依存クレートの索引でメモリが 55 MB → 66 MB。使わない定義を落とす / 遅延で読む
