#!/usr/bin/env bash
# セルフホストの確認: crustc 自身のソースの一部 (外部クレートに依存しないもの) を crustc でコンパイルし、
# rustc でコンパイルしたものと同じ入力で同じ出力になるかを確かめる。
#   lexer: src/lexer.rs と src/cache.rs の KeyHasher (SipHash) → crustc 自身のソースを字句解析してアイテムとハッシュを出す
#   prelude: src/prelude.rs (prelude の項目を名前で選ぶ。lexer / fxhash / KeyHasher と、build.rs が作る表を含む)
#            → probe / examples をユーザのソースとして、選ばれた prelude の長さとハッシュを出す
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
