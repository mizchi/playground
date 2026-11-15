#!/usr/bin/env bash
# セルフホストの確認: crustc 自身のソースの一部 (外部クレートに依存しないもの) を crustc でコンパイルし、
# rustc でコンパイルしたものと同じ入力で同じ出力になるかを確かめる。
#   lexer: src/lexer.rs と src/cache.rs の KeyHasher (SipHash) → crustc 自身のソースを字句解析してアイテムとハッシュを出す
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
exit $fail
