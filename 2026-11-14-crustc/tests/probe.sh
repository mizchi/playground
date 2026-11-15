#!/usr/bin/env bash
# よく使う Rust の機能の小さなプログラム (tests/probe/*.rs) を rustc と crustc でビルドして出力を比べ、
# 通った数を出す。機能を足すたびに数が増えるのを見る (落ちたものは理由の 1 行目も出す)。
#   tests/probe.sh          全部
#   tests/probe.sh NAME..   指定したものだけ
set -uo pipefail
cd "$(dirname "$0")/.."
cargo build --release -q || exit 1
out=$(mktemp -d)
pass=0
total=0
names=("$@")
if [ ${#names[@]} -eq 0 ]; then
  for f in tests/probe/*.rs; do names+=("$(basename "$f" .rs)"); done
fi
for n in "${names[@]}"; do
  f=tests/probe/$n.rs
  total=$((total + 1))
  if ! rustc --edition 2024 -o "$out/$n.r" "$f" 2>/dev/null; then
    echo "SKIP $n (rustc がビルドできない)"
    continue
  fi
  # (tests/probe/NAME.in があれば標準入力に渡す)
  input=/dev/null
  [ -f "tests/probe/$n.in" ] && input="tests/probe/$n.in"
  "$out/$n.r" < "$input" > "$out/$n.r.out" 2>/dev/null
  rc_r=$?
  if ! err=$(target/release/crustc "$f" -o "$out/$n.e" 2>&1); then
    echo "FAIL $n: $(echo "$err" | head -1 | sed 's/^error: [^ ]*: //')"
    continue
  fi
  "$out/$n.e" < "$input" > "$out/$n.e.out" 2>/dev/null
  rc_e=$?
  if cmp -s "$out/$n.r.out" "$out/$n.e.out" && [ $rc_r -eq $rc_e ]; then
    pass=$((pass + 1))
  else
    echo "DIFF $n"
  fi
done
echo "probe: $pass / $total"
rm -rf "$out"
