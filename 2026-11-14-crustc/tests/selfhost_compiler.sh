#!/usr/bin/env bash
# セルフホストの仕上げ: syn・Cranelift を使わないコンパイラ全体 (tests/selfhost/build_compiler.py が 1 ファイルにまとめたもの) を
# rustc と crustc (driver2) の両方でコンパイルし、できた 2 つのコンパイラで tests/probe / examples / tests/acg をコンパイルして
#   - 2 つのコンパイラが出す .o が同じバイト列か
#   - それを実行した出力が同じか
# を確かめる。さらに crustc 版のコンパイラで自分自身 (compiler_main.rs) をもう一度コンパイルし (2 段目)、
# 1 段目と同じ .o になるか (不動点) も確かめる。
set -uo pipefail
cd "$(dirname "$0")/.."
cargo build --release -q || exit 1
out=out/selfhost
mkdir -p "$out/cc/r" "$out/cc/c"
python3 tests/selfhost/build_compiler.py > "$out/compiler_main.rs"
rustc --edition 2024 -O -Awarnings "$out/compiler_main.rs" -o "$out/compiler_rustc" || exit 1
# (1 段目は minclif: できたコンパイラ (中も minclif) が自分自身をコンパイルした結果と同じになるはず)
if ! target/release/crustc --backend=minclif "$out/compiler_main.rs" -o "$out/compiler_crustc" -O; then
  echo "FAIL compiler (crustc)"; exit 1
fi
same=0; diff_o=0; diff_run=0; fail=0
for f in tests/probe/*.rs examples/*.rs tests/acg/*.rs; do
  n=$(basename "$f" .rs)
  a="$out/cc/r/$n"; b="$out/cc/c/$n"
  ra=0; rb=0
  "$out/compiler_rustc" "$f" -o "$a" 2> "$a.err" || ra=1
  "$out/compiler_crustc" "$f" -o "$b" 2> "$b.err" || rb=1
  if [ $ra -ne 0 ] || [ $rb -ne 0 ]; then
    if [ $ra -ne $rb ] || ! cmp -s "$a.err" "$b.err"; then
      echo "DIFF $n: build status differs"; fail=1
    fi
    continue
  fi
  if ! cmp -s "$a.o" "$b.o"; then
    echo "DIFF $n: .o differs"; diff_o=$((diff_o + 1)); fail=1
    continue
  fi
  oa=$("$a" < /dev/null 2>/dev/null; echo "exit $?")
  ob=$("$b" < /dev/null 2>/dev/null; echo "exit $?")
  if [ "$oa" != "$ob" ]; then
    echo "DIFF $n: output differs"; diff_run=$((diff_run + 1)); fail=1
    continue
  fi
  same=$((same + 1))
done
echo "compiled by both compilers: $same same, $diff_o .o differ, $diff_run outputs differ"
# 2 段目: crustc 版のコンパイラで自分自身をコンパイルする
if "$out/compiler_crustc" "$out/compiler_main.rs" -o "$out/compiler_crustc2" -O; then
  if cmp -s "$out/compiler_crustc.o" "$out/compiler_crustc2.o"; then
    echo "ok   stage 2: the crustc-built compiler reproduces itself ($(wc -c < "$out/compiler_crustc2.o") bytes .o)"
  else
    echo "DIFF stage 2: .o differs from stage 1"; fail=1
  fi
else
  echo "FAIL stage 2 (compiler_crustc could not compile itself)"; fail=1
fi
# Cranelift のバックエンドでビルドしたコンパイラも同じ .o を出すか
if target/release/crustc "$out/compiler_main.rs" -o "$out/compiler_clif" -O && "$out/compiler_clif" "$out/compiler_main.rs" -o "$out/compiler_clif2" -O; then
  if cmp -s "$out/compiler_crustc.o" "$out/compiler_clif2.o"; then
    echo "ok   the Cranelift-built compiler produces the same .o"
  else
    echo "DIFF the Cranelift-built compiler: .o differs"; fail=1
  fi
else
  echo "FAIL the Cranelift-built compiler"; fail=1
fi
exit $fail
