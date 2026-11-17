#!/usr/bin/env bash
# examples/*.rs を crustc と rustc の両方でビルドし、実行結果が一致するか確かめる。
# usage: tests/compare.sh   (このディレクトリで、cargo build --release の後に実行)
set -u
cd "$(dirname "$0")/.."
out=out/compare
mkdir -p "$out"
fail=0
for src in examples/*.rs; do
  name=$(basename "$src" .rs)
  # キャッシュなし (cold) とキャッシュからの再ビルド (warm) の両方を確かめる
  rm -f "$out/$name.crcache2" "$out/$name.cold.crcache2"
  if ! ./target/release/crustc "$src" -o "$out/$name.cold" >"$out/$name.log" 2>&1; then
    echo "FAIL $name (crustc)"; cat "$out/$name.log"; fail=1; continue
  fi
  cp "$out/$name.cold.crcache2" "$out/$name.crcache2" 2>/dev/null
  if ! ./target/release/crustc "$src" -o "$out/$name" >"$out/$name.log" 2>&1; then
    echo "FAIL $name (crustc, warm)"; cat "$out/$name.log"; fail=1; continue
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
  "$out/$name.cold" >"$out/$name.out0"; c0=$?
  "$out/$name" >"$out/$name.out1"; c1=$?
  echo "exit=$c0" >>"$out/$name.out0"
  if ! diff -q "$out/$name.out0" <("$out/$name"; echo "exit=$?") >/dev/null; then
    echo "FAIL $name (cold and warm builds differ)"; fail=1; continue
  fi
  "$out/$name.rustc" >"$out/$name.out2"; c2=$?
  echo "exit=$c1" >>"$out/$name.out1"; echo "exit=$c2" >>"$out/$name.out2"
  # valgrind があれば、メモリエラー (二重解放・解放後の使用・未初期化の読み出し) とリークも見る
  if command -v valgrind >/dev/null 2>&1; then
    # (終了コードはプログラムのもの。-q なので何か出力されたら問題あり)
    valgrind -q --leak-check=full --errors-for-leak-kinds=definite --log-file="$out/$name.vg" \
        "$out/$name" >/dev/null 2>/dev/null
    if [ -s "$out/$name.vg" ]; then
      echo "FAIL $name (valgrind)"; head -20 "$out/$name.vg"; fail=1; continue
    fi
  fi
  if diff "$out/$name.out1" "$out/$name.out2" >"$out/$name.diff"; then
    echo "ok   $name"
  else
    echo "FAIL $name (output differs)"; cat "$out/$name.diff"; fail=1
  fi
done
exit $fail
