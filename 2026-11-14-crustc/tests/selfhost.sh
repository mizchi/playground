#!/usr/bin/env bash
# セルフホストの確認: crustc 自身のソースの一部 (外部クレートに依存しないもの) を crustc でコンパイルし、
# rustc でコンパイルしたものと同じ入力で同じ出力になるかを確かめる。
#   lexer: src/lexer.rs と src/cache.rs の KeyHasher (SipHash) → crustc 自身のソースを字句解析してアイテムとハッシュを出す
#   prelude: src/prelude.rs (prelude の項目を名前で選ぶ。lexer / fxhash / KeyHasher と、build.rs が作る表を含む)
#            → probe / examples をユーザのソースとして、選ばれた prelude の長さとハッシュを出す
#   link:  src/link.rs (リンカを探して直接起動する) と src/testgen.rs → C の .o をリンクして実行する (std::path / process::Command)
#   elfz:  src/elfz.rs (.o のセクションの圧縮。zstd の代わりに同じ API の RLE を付ける) → C の .o を圧縮・解凍して中身を比べる
#   macros: src/macros.rs (macro_rules! の展開と include_str!。proc_macro2 の代わりに tests/selfhost/mini_pm2.rs を付ける)
#           → probe / examples / src を展開して、結果の長さとハッシュを出す
#   parse: src/parse.rs (syn を使わない構文解析) → probe / examples / src を解析して節の数とアイテムを出す
#   ast_closure: src/parse.rs + src/ast_closure.rs (自前の構文木の上のクロージャ解析) → 全関数のクロージャの自由変数
#   ast_types: src/types.rs + src/parse.rs + src/ast_types.rs (型の表現・intern と、型の構文 → TySyn。front.rs から分けたもの)
#   ast_summary: src/ast_summary.rs (front.rs の summarize の移植) + lexer / types / parse / ast_types / ast_closure → 全アイテムの要約
#   program: src/program.rs (全体の表: シグネチャ・レイアウト・trait 実装) + 要約までのモジュール + prelude
#            + ast_consteval / solver / ctors → probe / examples / src の Program を作り、関数・型 (サイズ・オフセット・深いハッシュ)・
#            impl・const とそのデータ・パスの検索・型推論の単一化の結果を出す
#   minclif: src/minclif.rs (Cranelift の代わりの最小のバックエンド) → 関数を組み立てた ELF の .o が rustc 版と同じバイト列か。
#            crustc 版が書いた .o を C の main とリンクして動かす
#   enc:   src/enc.rs (キャッシュの直列化。マクロ・trait の静的呼び出し) と src/fxhash.rs (FxHasher / FxMap)
set -uo pipefail
cd "$(dirname "$0")/.."
cargo build --release -q || exit 1
out=out/selfhost
mkdir -p "$out"
fail=0
python3 tests/selfhost/build_lexer.py > "$out/lexer_main.rs"
rustc --edition 2024 -O -Awarnings "$out/lexer_main.rs" -o "$out/lexer_rustc" || exit 1
if ! target/release/crustc "$out/lexer_main.rs" -o "$out/lexer_crustc" -O; then
  echo "FAIL lexer (crustc)"; exit 1
fi
inputs=(src/*.rs src/prelude_lib.rs.in examples/*.rs tests/probe/*.rs)
"$out/lexer_rustc" "${inputs[@]}" > "$out/lexer.rustc.out"
"$out/lexer_crustc" "${inputs[@]}" > "$out/lexer.crustc.out"
if cmp -s "$out/lexer.rustc.out" "$out/lexer.crustc.out"; then
  echo "ok   lexer ($(wc -l < "$out/lexer.rustc.out") lines over ${#inputs[@]} files)"
else
  echo "DIFF lexer"; diff "$out/lexer.rustc.out" "$out/lexer.crustc.out" | head; fail=1
fi
python3 tests/selfhost/build_prelude.py > "$out/prelude_main.rs"
rustc --edition 2024 -O -Awarnings "$out/prelude_main.rs" -o "$out/prelude_rustc" || exit 1
pin=(tests/probe/*.rs examples/*.rs)
"$out/prelude_rustc" "${pin[@]}" > "$out/prelude.rustc.out"
for opt in "" -O; do
  if ! target/release/crustc "$out/prelude_main.rs" -o "$out/prelude_crustc" $opt; then
    echo "FAIL prelude $opt (crustc)"; fail=1; continue
  fi
  "$out/prelude_crustc" "${pin[@]}" > "$out/prelude.crustc.out"
  if cmp -s "$out/prelude.rustc.out" "$out/prelude.crustc.out"; then
    echo "ok   prelude $opt (${#pin[@]} files)"
  else
    echo "DIFF prelude $opt"; diff "$out/prelude.rustc.out" "$out/prelude.crustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_link.py > "$out/link_main.rs"
printf '#include <stdio.h>\nint main(void){puts("hello from obj");return 0;}\n' > "$out/hello.c"
cc -c "$out/hello.c" -o "$out/hello.o" || exit 1
rustc --edition 2024 -O -Awarnings "$out/link_main.rs" -o "$out/link_rustc" || exit 1
rm -f "$out/hello_x" "$out/hello_x.link"
"$out/link_rustc" "$out/hello.o" "$out/hello_x" > "$out/link.rustc.out"
for opt in "" -O; do
  if ! target/release/crustc "$out/link_main.rs" -o "$out/link_crustc" $opt; then
    echo "FAIL link $opt (crustc)"; fail=1; continue
  fi
  rm -f "$out/hello_x" "$out/hello_x.link"
  "$out/link_crustc" "$out/hello.o" "$out/hello_x" > "$out/link.crustc.out"
  if cmp -s "$out/link.rustc.out" "$out/link.crustc.out"; then
    echo "ok   link $opt ($(wc -l < "$out/link.rustc.out") lines)"
  else
    echo "DIFF link $opt"; diff "$out/link.rustc.out" "$out/link.crustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_elfz.py > "$out/elfz_main.rs"
cat > "$out/many.c" <<'EOF2'
#include <stdio.h>
static int tbl[256];
int f1(int x) { return x * 3 + 1; }
int f2(int x) { return f1(x) ^ 7; }
double g(double a) { return a * 0.5; }
const char *names[] = {"a", "bb", "ccc"};
const int zeros[2048] = {1, 2, 3};
int main(void) { for (int i = 0; i < 256; i++) tbl[i] = f2(i); printf("%d %f %s %d\n", tbl[9], g(3.0), names[2], zeros[1]); return 0; }
EOF2
cc -O1 -c "$out/many.c" -o "$out/many.o" || exit 1
rustc --edition 2024 -O -Awarnings "$out/elfz_main.rs" -o "$out/elfz_rustc" || exit 1
"$out/elfz_rustc" "$out/hello.o" "$out/many.o" > "$out/elfz.rustc.out"
for opt in "" -O; do
  if ! target/release/crustc "$out/elfz_main.rs" -o "$out/elfz_crustc" $opt; then
    echo "FAIL elfz $opt (crustc)"; fail=1; continue
  fi
  "$out/elfz_crustc" "$out/hello.o" "$out/many.o" > "$out/elfz.crustc.out"
  if cmp -s "$out/elfz.rustc.out" "$out/elfz.crustc.out"; then
    echo "ok   elfz $opt ($(wc -l < "$out/elfz.rustc.out") lines)"
  else
    echo "DIFF elfz $opt"; diff "$out/elfz.rustc.out" "$out/elfz.crustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_macros.py > "$out/macros_main.rs"
rustc --edition 2024 -O -Awarnings "$out/macros_main.rs" -o "$out/macros_rustc" || exit 1
min=(tests/probe/*.rs examples/*.rs src/*.rs)
"$out/macros_rustc" "${min[@]}" > "$out/macros.rustc.out"
for opt in "" -O; do
  if ! target/release/crustc "$out/macros_main.rs" -o "$out/macros_crustc" $opt; then
    echo "FAIL macros $opt (crustc)"; fail=1; continue
  fi
  "$out/macros_crustc" "${min[@]}" > "$out/macros.crustc.out"
  if cmp -s "$out/macros.rustc.out" "$out/macros.crustc.out"; then
    echo "ok   macros $opt (${#min[@]} files, $(grep -c expanded "$out/macros.rustc.out") expanded)"
  else
    echo "DIFF macros $opt"; diff "$out/macros.rustc.out" "$out/macros.crustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_parse.py > "$out/parse_main.rs"
rustc --edition 2024 -O -Awarnings "$out/parse_main.rs" -o "$out/parse_rustc" || exit 1
"$out/parse_rustc" "${min[@]}" > "$out/parse.rustc.out"
for opt in "" -O; do
  if ! target/release/crustc "$out/parse_main.rs" -o "$out/parse_crustc" $opt; then
    echo "FAIL parse $opt (crustc)"; fail=1; continue
  fi
  "$out/parse_crustc" "${min[@]}" > "$out/parse.crustc.out"
  if cmp -s "$out/parse.rustc.out" "$out/parse.crustc.out"; then
    echo "ok   parse $opt (${#min[@]} files, $(wc -l < "$out/parse.rustc.out") lines)"
  else
    echo "DIFF parse $opt"; diff "$out/parse.rustc.out" "$out/parse.crustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_ast_closure.py > "$out/ast_closure_main.rs"
rustc --edition 2024 -O -Awarnings "$out/ast_closure_main.rs" -o "$out/ast_closure_rustc" || exit 1
"$out/ast_closure_rustc" "${min[@]}" > "$out/ast_closure.rustc.out"
for opt in "" -O; do
  if ! target/release/crustc "$out/ast_closure_main.rs" -o "$out/ast_closure_crustc" $opt; then
    echo "FAIL ast_closure $opt (crustc)"; fail=1; continue
  fi
  "$out/ast_closure_crustc" "${min[@]}" > "$out/ast_closure.crustc.out"
  if cmp -s "$out/ast_closure.rustc.out" "$out/ast_closure.crustc.out"; then
    echo "ok   ast_closure $opt ($(grep -c ': .* closures$' "$out/ast_closure.rustc.out") files)"
  else
    echo "DIFF ast_closure $opt"; diff "$out/ast_closure.rustc.out" "$out/ast_closure.crustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_ast_types.py > "$out/ast_types_main.rs"
rustc --edition 2024 -O -Awarnings "$out/ast_types_main.rs" -o "$out/ast_types_rustc" || exit 1
"$out/ast_types_rustc" "${min[@]}" > "$out/ast_types.rustc.out"
for opt in "" -O; do
  if ! target/release/crustc "$out/ast_types_main.rs" -o "$out/ast_types_crustc" $opt; then
    echo "FAIL ast_types $opt (crustc)"; fail=1; continue
  fi
  "$out/ast_types_crustc" "${min[@]}" > "$out/ast_types.crustc.out"
  if cmp -s "$out/ast_types.rustc.out" "$out/ast_types.crustc.out"; then
    echo "ok   ast_types $opt ($(wc -l < "$out/ast_types.rustc.out") lines)"
  else
    echo "DIFF ast_types $opt"; diff "$out/ast_types.rustc.out" "$out/ast_types.crustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_ast_summary.py > "$out/ast_summary_main.rs"
rustc --edition 2024 -O -Awarnings "$out/ast_summary_main.rs" -o "$out/ast_summary_rustc" || exit 1
"$out/ast_summary_rustc" "${min[@]}" > "$out/ast_summary.rustc.out"
for opt in "" -O; do
  if ! target/release/crustc "$out/ast_summary_main.rs" -o "$out/ast_summary_crustc" $opt; then
    echo "FAIL ast_summary $opt (crustc)"; fail=1; continue
  fi
  "$out/ast_summary_crustc" "${min[@]}" > "$out/ast_summary.crustc.out"
  if cmp -s "$out/ast_summary.rustc.out" "$out/ast_summary.crustc.out"; then
    echo "ok   ast_summary $opt ($(wc -l < "$out/ast_summary.rustc.out") lines)"
  else
    echo "DIFF ast_summary $opt"; diff "$out/ast_summary.rustc.out" "$out/ast_summary.crustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_program.py > "$out/program_main.rs"
rustc --edition 2024 -O -Awarnings "$out/program_main.rs" -o "$out/program_rustc" || exit 1
pin=(examples/*.rs tests/probe/*.rs src/*.rs)
"$out/program_rustc" "${pin[@]}" > "$out/program.rustc.out"
for opt in "" -O; do
  if ! target/release/crustc "$out/program_main.rs" -o "$out/program_crustc" $opt; then
    echo "FAIL program $opt (crustc)"; fail=1; continue
  fi
  "$out/program_crustc" "${pin[@]}" > "$out/program.crustc.out"
  if cmp -s "$out/program.rustc.out" "$out/program.crustc.out"; then
    echo "ok   program $opt ($(wc -l < "$out/program.rustc.out") lines)"
  else
    echo "DIFF program $opt"; diff "$out/program.rustc.out" "$out/program.crustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_minclif.py > "$out/minclif_main.rs"
rustc --edition 2024 -O -Awarnings "$out/minclif_main.rs" -o "$out/minclif_rustc" || exit 1
"$out/minclif_rustc" "$out/minclif.rustc.o" "$out/minclif2.rustc.o" > "$out/minclif2.want"
printf 'long mc_ext(void);\nint main(){return (int)mc_ext();}\n' > "$out/minclif2_c_main.c"
printf '#include <stdio.h>\nlong mc_sum(long);\nint mc_fib(int);\nlong mc_misc(long,long,long,long,long,long,long,long);\nint main(){printf("%%ld %%d %%ld\\n", mc_sum(10), mc_fib(20), mc_misc(1,2,3,4,5,6,7,8));return 0;}\n' > "$out/minclif_c_main.c"
for opt in "" -O; do
  if ! target/release/crustc "$out/minclif_main.rs" -o "$out/minclif_crustc" $opt; then
    echo "FAIL minclif $opt (crustc)"; fail=1; continue
  fi
  "$out/minclif_crustc" "$out/minclif.crustc.o" "$out/minclif2.crustc.o" > "$out/minclif2.crustc.want"
  if cmp -s "$out/minclif.rustc.o" "$out/minclif.crustc.o" && cmp -s "$out/minclif2.rustc.o" "$out/minclif2.crustc.o" \
     && cmp -s "$out/minclif2.want" "$out/minclif2.crustc.want" \
     && cc "$out/minclif_c_main.c" "$out/minclif.crustc.o" -o "$out/minclif_run" && [ "$("$out/minclif_run")" = "45 6765 -1005" ] \
     && cc "$out/minclif2_c_main.c" "$out/minclif2.crustc.o" -o "$out/minclif2_run" && "$out/minclif2_run" | cmp -s - "$out/minclif2.want"; then
    echo "ok   minclif $opt ($(wc -c < "$out/minclif.rustc.o") + $(wc -c < "$out/minclif2.rustc.o") bytes, $(wc -l < "$out/minclif2.want") checks run)"
  else
    echo "DIFF minclif $opt"; fail=1
  fi
done
python3 tests/selfhost/build_enc.py > "$out/enc_main.rs"
rustc --edition 2024 -O -Awarnings "$out/enc_main.rs" -o "$out/enc_rustc" || exit 1
for opt in "" -O; do
  if ! target/release/crustc "$out/enc_main.rs" -o "$out/enc_crustc" $opt; then
    echo "FAIL enc $opt (crustc)"; fail=1; continue
  fi
  if cmp -s <("$out/enc_rustc") <("$out/enc_crustc"); then
    echo "ok   enc $opt ($("$out/enc_rustc" | wc -l) lines)"
  else
    echo "DIFF enc $opt"; diff <("$out/enc_rustc") <("$out/enc_crustc") | head; fail=1
  fi
done
exit $fail
