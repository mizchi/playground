"""crustc 自身の crates/core/src/parse.rs と crates/core/src/ast_closure.rs (自前の構文木の上のクロージャ解析) を 1 ファイルのプログラムにまとめる。
usage: python3 tests/selfhost/build_ast_closure.py > out/selfhost/ast_closure_main.rs
main は引数のファイルごとに、関数の本体のクロージャの自由変数とキャプチャを値として使うかを出す。"""

parse = open("crates/core/src/parse.rs").read()
clo = open("crates/core/src/ast_closure.rs").read()
print("""// 自動生成: tests/selfhost/build_ast_closure.py (crustc の crates/core/src/parse.rs と crates/core/src/ast_closure.rs をそのまま含む)
mod parse {
""" + parse + """}

mod ast_closure {
""" + clo + """}

use parse::{Block, Item, ItemKind, parse_file};

fn bodies(items: &[Item], out: &mut Vec<Block>) {
    for it in items {
        match &it.kind {
            ItemKind::Fn(_, Some(b)) => out.push(b.clone()),
            ItemKind::Impl(_, _, _, is) | ItemKind::Trait(_, _, _, is) => {
                for i in is {
                    if let ItemKind::Fn(_, Some(b)) = &i.kind {
                        out.push(b.clone());
                    }
                }
            }
            ItemKind::Mod(_, Some(is)) => bodies(is, out),
            _ => {}
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    for path in &args[1..] {
        let src = std::fs::read_to_string(path).unwrap();
        let f = match parse_file(&src) {
            Ok(f) => f,
            Err(e) => {
                println!("{path}: error {e}");
                continue;
            }
        };
        let mut bs = Vec::new();
        bodies(&f.items, &mut bs);
        let mut n = 0;
        for (k, b) in bs.iter().enumerate() {
            for (j, c) in ast_closure::closures_of(b).iter().enumerate() {
                let fv = ast_closure::free_vars(c);
                let mv = ast_closure::moves_capture(c, &fv);
                println!("{path} {k}.{j}: {} {mv}", fv.join(","));
                n += 1;
            }
        }
        println!("{path}: {} bodies, {n} closures", bs.len());
    }
}
""")
