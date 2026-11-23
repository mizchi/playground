#!/usr/bin/env bash
# ネイティブの描画プロセス (native/renderer) をビルドする。
#   ODIN=odin (既定)、SDL3_LIB=SDL3 の共有ライブラリのあるディレクトリ (無ければシステムのもの)
#   wgpu-native は Odin の vendor/wgpu/lib/wgpu-<os>-<arch>-release/lib/ に置く (Odin の vendor/wgpu/doc.odin を参照)
set -eu
cd "$(dirname "$0")/.."
ODIN="${ODIN:-odin}"
flags=()
if [ -n "${SDL3_LIB:-}" ]; then flags+=("-extra-linker-flags:-L$SDL3_LIB -Wl,-rpath,$SDL3_LIB"); fi
mkdir -p native/bin
"$ODIN" build native/renderer -o:speed -out:native/bin/renderer "${flags[@]}"
echo "native/bin/renderer"
