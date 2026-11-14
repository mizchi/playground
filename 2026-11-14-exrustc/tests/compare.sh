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
  # no_std (#![no_std] + #![no_main]) は libcore だけで、panic=abort・libc をリンクしてビルドする
  # (rust_eh_personality は panic=abort では呼ばれないので 0 に定義しておく)
  if grep -q '^#!\[no_std\]' "$src"; then
    rflags=(--edition 2024 -C panic=abort -C link-arg=-lc -C link-arg=-Wl,--defsym=rust_eh_personality=0)
  else
    rflags=()
  fi
  rustc -Awarnings "${rflags[@]}" "$src" -o "$out/$name.rustc" || { echo "FAIL $name (rustc)"; fail=1; continue; }
  # 終了コードも比べる (no_std の main は戻り値が終了コード)
  "$out/$name" >"$out/$name.out1"; c1=$?
  "$out/$name.rustc" >"$out/$name.out2"; c2=$?
  echo "exit=$c1" >>"$out/$name.out1"; echo "exit=$c2" >>"$out/$name.out2"
  if diff "$out/$name.out1" "$out/$name.out2" >"$out/$name.diff"; then
    echo "ok   $name"
  else
    echo "FAIL $name (output differs)"; cat "$out/$name.diff"; fail=1
  fi
done
exit $fail
