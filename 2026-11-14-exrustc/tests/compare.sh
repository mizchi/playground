#!/usr/bin/env bash
# examples/*.rs を exrustc と rustc の両方でビルドし、実行結果が一致するか確かめる。
# usage: tests/compare.sh   (このディレクトリで、cargo build --release の後に実行)
set -u
cd "$(dirname "$0")/.."
out=out/compare
mkdir -p "$out"
fail=0
for src in examples/*.rs; do
  name=$(basename "$src" .rs)
  if ! ./target/release/exrustc "$src" -o "$out/$name" >"$out/$name.log" 2>&1; then
    echo "FAIL $name (exrustc)"; cat "$out/$name.log"; fail=1; continue
  fi
  rustc -Awarnings "$src" -o "$out/$name.rustc" || { echo "FAIL $name (rustc)"; fail=1; continue; }
  if diff <("$out/$name") <("$out/$name.rustc") >"$out/$name.diff"; then
    echo "ok   $name"
  else
    echo "FAIL $name (output differs)"; cat "$out/$name.diff"; fail=1
  fi
done
exit $fail
