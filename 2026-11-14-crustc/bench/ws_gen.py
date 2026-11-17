"""複数クレート・複数ファイルのワークスペースを作る (driver2 の `crustc build` と rustc の比較用)。

usage: python3 bench/ws_gen.py <dir> [--crates K] [--files F] [--fns N]

- lib c0..c{K-1}: c{i} は c{i-1} と c0 に依存する。各クレートは lib.rs + F 個のモジュールのファイル (m{j}.rs)、
  ファイルごとに N 個の関数・struct・総称関数・trait の impl
- bin app: 全部のクレートに依存し、各クレートの run を呼んで合計を出す
- workspace.txt (crustc build 用) と build_rustc.sh (rustc で同じものを rlib + リンクで作る)
"""
import os
import sys


def gen_module(ci, j, n, dep):
    out = []
    out.append(f"use crate::Acc;\n")
    out.append(f"#[derive(Debug, Clone, Copy, PartialEq)]\npub struct S{j} {{ pub a: u64, pub b: u64 }}\n")
    out.append(f"impl S{j} {{\n    pub fn new(a: u64) -> S{j} {{ S{j} {{ a, b: a ^ {ci * 131 + j} }} }}\n"
               f"    pub fn mix(&self) -> u64 {{ self.a.wrapping_mul(0x9E37) ^ self.b }}\n}}\n")
    out.append(f"impl Acc for S{j} {{\n    fn acc(&self) -> u64 {{ self.mix() + {j} }}\n}}\n")
    out.append(f"pub fn pick{j}<T: Copy>(v: &[T], i: usize) -> T {{ v[i % v.len()] }}\n")
    for k in range(n):
        body = [f"pub fn f{j}_{k}(x: u64) -> u64 {{",
                f"    let mut s = x.wrapping_add({k});",
                f"    for i in 0..{k % 5 + 2}u64 {{",
                f"        s = s.wrapping_mul(31).wrapping_add(i ^ {ci * 7 + j});",
                "    }"]
        if k > 0:
            body.append(f"    s = s.rotate_left(7) ^ f{j}_{k - 1}(x >> 1);")
        if k == 0 and dep is not None:
            body.append(f"    s = s.wrapping_add({dep}::m0::f0_1(x & 0xFF));")
        if k % 4 == 1:
            body.append(f"    let v: Vec<u64> = (0..{k % 3 + 2}).map(|i| s.wrapping_add(i)).collect();")
            body.append(f"    s = s.wrapping_mul(3).wrapping_add(pick{j}(&v, {k}));")
        if k % 4 == 2:
            body.append(f"    s = s.wrapping_add(S{j}::new(s).acc());")
        body.append("    s")
        body.append("}")
        out.append("\n".join(body) + "\n")
    return "".join(out)


def main():
    args = sys.argv[1:]
    d = args[0]
    k_crates, files, fns = 4, 4, 25
    i = 1
    while i < len(args):
        if args[i] == "--crates":
            k_crates = int(args[i + 1])
        elif args[i] == "--files":
            files = int(args[i + 1])
        elif args[i] == "--fns":
            fns = int(args[i + 1])
        i += 2
    ws = []
    rs = ["#!/usr/bin/env bash", "# rustc で同じワークスペースを作る (rlib を作ってからリンク)", "set -e", 'cd "$(dirname "$0")"',
          'OUT=${OUT:-target_rustc}', 'mkdir -p $OUT', 'FLAGS=${FLAGS:-}']
    for ci in range(k_crates):
        name = f"c{ci}"
        deps = []
        if ci >= 1:
            deps.append(f"c{ci - 1}")
        if ci >= 2:
            deps.append("c0")
        src = os.path.join(d, name, "src")
        os.makedirs(src, exist_ok=True)
        lib = [f"// crate {name}\n"]
        for j in range(files):
            lib.append(f"pub mod m{j};\n")
        lib.append("pub trait Acc { fn acc(&self) -> u64; }\n")
        lib.append(f"pub fn run(x: u64) -> u64 {{\n    let mut s = x;\n")
        for j in range(files):
            lib.append(f"    s = s.wrapping_add(m{j}::f{j}_{fns - 1}(s));\n")
        lib.append("    s\n}\n")
        with open(os.path.join(src, "lib.rs"), "w") as f:
            f.write("".join(lib))
        for j in range(files):
            with open(os.path.join(src, f"m{j}.rs"), "w") as f:
                f.write(gen_module(ci, j, fns, deps[0] if deps else None))
        ws.append(f"lib {name} {name}/src/lib.rs {' '.join(deps)}".rstrip())
        ext = " ".join(f"--extern {x}=$OUT/lib{x}.rlib" for x in deps)
        rs.append(f"rustc --edition 2021 $FLAGS -Awarnings --crate-type rlib --crate-name {name} {name}/src/lib.rs -L $OUT -o $OUT/lib{name}.rlib {ext}")
    src = os.path.join(d, "app", "src")
    os.makedirs(src, exist_ok=True)
    body = ["fn main() {", "    let mut s = std::env::args().count() as u64;"]
    for ci in range(k_crates):
        body.append(f"    let r = c{ci}::run(s);")
        body.append(f'    println!("c{ci} {{}}", r);')
        body.append("    s = s.wrapping_add(r);")
    body.append('    println!("{}", s);')
    body.append("}")
    with open(os.path.join(src, "main.rs"), "w") as f:
        f.write("\n".join(body) + "\n")
    all_deps = " ".join(f"c{ci}" for ci in range(k_crates))
    ws.append(f"bin app app/src/main.rs {all_deps}")
    ext = " ".join(f"--extern c{ci}=$OUT/libc{ci}.rlib" for ci in range(k_crates))
    rs.append(f"rustc --edition 2021 $FLAGS -Awarnings --crate-name app app/src/main.rs -L $OUT -o $OUT/app {ext}")
    with open(os.path.join(d, "workspace.txt"), "w") as f:
        f.write("\n".join(ws) + "\n")
    with open(os.path.join(d, "build_rustc.sh"), "w") as f:
        f.write("\n".join(rs) + "\n")
    os.chmod(os.path.join(d, "build_rustc.sh"), 0o755)


main()
