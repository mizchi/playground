#!/usr/bin/env bash
# セルフホストの確認: crustc 自身のソースの一部 (外部クレートに依存しないもの) を crustc でコンパイルし、
# rustc でコンパイルしたものと同じ入力で同じ出力になるかを確かめる。
#   lexer: src/lexer.rs と src/cache.rs の KeyHasher (SipHash) → crustc 自身のソースを字句解析してアイテムとハッシュを出す
#   prelude: src/prelude.rs (prelude の項目を名前で選ぶ。lexer / fxhash / KeyHasher と、build.rs が作る表を含む)
#            → probe / examples をユーザのソースとして、選ばれた prelude の長さとハッシュを出す
#   link:  src/link.rs (リンカを探して直接起動する) と src/testgen.rs → C の .o をリンクして実行する (std::path / process::Command)
#   elfz:  src/elfz.rs (.o のセクションの圧縮。zstd の代わりに同じ API の RLE を付ける) → C の .o を圧縮・解凍して中身を比べる
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
