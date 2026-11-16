#!/usr/bin/env bash
# rustc で同じワークスペースを作る (比較用)
set -e
cd "$(dirname "$0")"
OUT=$1
rustc --edition 2021 -Awarnings --crate-type rlib --crate-name util util/src/lib.rs -o $OUT/libutil.rlib
rustc --edition 2021 -Awarnings --crate-type rlib --crate-name geom geom/src/lib.rs -L $OUT -o $OUT/libgeom.rlib --extern util=$OUT/libutil.rlib
rustc --edition 2021 -Awarnings --crate-name app app/src/main.rs -L $OUT -o $OUT/app --extern geom=$OUT/libgeom.rlib --extern util=$OUT/libutil.rlib
