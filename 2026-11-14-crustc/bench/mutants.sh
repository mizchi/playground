#!/usr/bin/env bash
# ミューテーションテスト (cargo-mutants)。設定は .cargo/mutants.toml、ビルドは profile.mutants。
# 変異は全部で 1 万 8 千個ほどあり、1 個にビルドとテストで数十秒かかるので、範囲を絞って回す:
#
#   bench/mutants.sh diff [BASE]      BASE (既定 origin/main) からの差分の行の変異だけ (変更の確認に)
#   bench/mutants.sh pkg NAME         クレート NAME の変異 (例: crustc-ide, crustc-base)
#   bench/mutants.sh file PATH...     ファイルの変異 (例: crates/ide/src/symbols.rs)
#   bench/mutants.sh list [...]       変異の一覧だけ (数を見る)。後ろは cargo mutants の引数
#
# テストに使うパッケージは、変異を入れるクレートから決める:
#   crates/ide・crates/lsp だけ → crustc-ide と crustc-lsp のテスト (速い)
#   それ以外 → ワークスペース全部のテスト (コンパイラの層のテストの多くは crustc (ルート) の tests/ にある)
# `--` の後ろは cargo mutants にそのまま渡す (例: -- -j 2 --shard 1/8 -F 'match_score')。
# 結果は mutants.out/ (missed.txt: テストで見つからなかった変異、caught.txt、timeout.txt、unviable.txt)。
#
# 要るもの: cargo install cargo-mutants --locked
set -uo pipefail
cd "$(dirname "$0")/.."
command -v cargo-mutants >/dev/null || { echo "cargo-mutants がない: cargo install cargo-mutants --locked"; exit 2; }

mode="${1:-}"
shift || true
args=()
extra=()
while [ $# -gt 0 ]; do
  if [ "$1" = "--" ]; then shift; extra=("$@"); break; fi
  args+=("$1"); shift
done

# 変異を入れるファイルの列 → テストのパッケージの指定
tests_for() {
  local only_ide=1
  for f in "$@"; do
    case "$f" in
      crates/ide/*|crates/lsp/*) ;;
      *) only_ide=0 ;;
    esac
  done
  if [ $only_ide = 1 ] && [ $# -gt 0 ]; then
    echo "--test-package crustc-ide --test-package crustc-lsp"
  else
    echo "--test-workspace true"
  fi
}

case "$mode" in
  diff)
    base="${args[0]:-origin/main}"
    d=$(mktemp)
    git diff "$base" -- . > "$d"
    mapfile -t files < <(git diff --name-only --relative "$base" -- . | grep '\.rs$')
    if [ ${#files[@]} = 0 ]; then echo "$base からの .rs の差分がない"; rm -f "$d"; exit 0; fi
    # shellcheck disable=SC2046
    cargo mutants --workspace --in-diff "$d" $(tests_for "${files[@]}") "${extra[@]}"
    st=$?; rm -f "$d"; exit $st ;;
  pkg)
    [ ${#args[@]} -ge 1 ] || { echo "usage: bench/mutants.sh pkg NAME"; exit 2; }
    case "${args[0]}" in
      crustc-ide|crustc-lsp) t="--test-package crustc-ide --test-package crustc-lsp" ;;
      *) t="--test-workspace true" ;;
    esac
    # shellcheck disable=SC2086
    cargo mutants -p "${args[0]}" $t "${extra[@]}" ;;
  file)
    [ ${#args[@]} -ge 1 ] || { echo "usage: bench/mutants.sh file PATH..."; exit 2; }
    fs=()
    for f in "${args[@]}"; do fs+=(-f "$f"); done
    # shellcheck disable=SC2046
    cargo mutants --workspace "${fs[@]}" $(tests_for "${args[@]}") "${extra[@]}" ;;
  list)
    cargo mutants --list --workspace "${args[@]}" "${extra[@]}" ;;
  *)
    sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
