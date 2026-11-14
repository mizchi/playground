#!/usr/bin/env bash
# 回帰チェック。
#   bench/regress.sh save    今の状態をベースライン "main" として保存する (機能追加の前に)
#   bench/regress.sh         ベースラインと比べる。criterion が「回帰」と判定したもの、
#                            サイズ予算 (bench/budget.txt) 超過、rustc との出力差があれば失敗する
# 環境変数 EXRUSTC_BENCH_LINKER でリンカを固定できる (ベースラインと同じ条件で比べること)。
set -uo pipefail
cd "$(dirname "$0")/.."
cargo build --release -q || exit 1
case "${1:-check}" in
  save)
    cargo bench --bench compile -- --save-baseline main 2>&1 | grep -E "time:|^[a-z]+/" ;;
  check)
    log=$(mktemp)
    cargo bench --bench compile -- --baseline main 2>&1 | tee "$log" | grep -E "time:|change:|regressed|improved|^[a-z]+/"
    fail=0
    if grep -q "Performance has regressed" "$log"; then
      echo "== 速度の回帰あり:"; grep -B3 "Performance has regressed" "$log" | grep -E "^[a-z]+/" ; fail=1
    fi
    cargo test --release -q --test budget 2>&1 | tail -3 || fail=1
    tests/compare.sh || fail=1
    rm -f "$log"
    [ $fail = 0 ] && echo "== 回帰なし"
    exit $fail ;;
  *) echo "usage: bench/regress.sh [save|check]"; exit 2 ;;
esac
