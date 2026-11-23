#!/bin/sh
# 同梱のパッケージ (app.json + .wasm) と、glue の生成器 (jco の wasm) を www/ に置く。どの段階で失敗しても止まる
set -eu
rm -rf www/packages
mkdir -p www/packages/todo-rs www/vendor
cp packages/todo-rs/app.json www/packages/todo-rs/
cp component/todo.wasm www/packages/todo-rs/todo.wasm
# jco-transpile は jco の依存 (pnpm では、jco の実体と同じ node_modules の中に並ぶ)
vendor="$(realpath node_modules/@bytecodealliance/jco)/../jco-transpile/vendor"
test -d "$vendor" || { echo "jco-transpile の vendor が見つからない: $vendor" >&2; exit 1; }
cp "$vendor"/*.wasm www/vendor/
