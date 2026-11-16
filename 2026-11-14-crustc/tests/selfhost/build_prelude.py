"""crustc 自身の src/prelude.rs (prelude の項目を名前で選ぶ) を、lexer.rs・fxhash.rs・cache.rs の KeyHasher と一緒に
1 ファイルのプログラムにまとめる。
usage: python3 tests/selfhost/build_prelude.py > out/selfhost/prelude_main.rs
main は引数のファイルごとに、ユーザのソースとして prelude::select を呼び、選ばれた prelude の長さ・行数・ハッシュを出す。
(include_str! は out/selfhost/ から見た src/prelude_lib.rs.in を指すように書き換え、build.rs の生成する
 PRELUDE_ITEMS の表 (target/release/build/crustc-*/out/prelude_items.rs の新しいもの) はそのまま埋め込む)"""
import glob, os

lexer = open("src/lexer.rs").read()
fx = open("src/fxhash.rs").read()
items = max(glob.glob("target/release/build/crustc-*/out/prelude_items.rs"), key=os.path.getmtime)
prelude = (open("src/prelude.rs").read()
    .replace('include_str!("prelude_lib.rs.in")', 'include_str!("../../src/prelude_lib.rs.in")')
    .replace('include!(concat!(env!("OUT_DIR"), "/prelude_items.rs"));', open(items).read()))
cache = open("src/key.rs").read()
a = cache.index("pub type Key = u128;")
b = len(cache)
hasher = cache[a:b]
print("""// 自動生成: tests/selfhost/build_prelude.py (crustc の src/prelude.rs をそのまま含む)
mod cache {
use std::hash::{Hash, Hasher};
""" + hasher + """}

mod fxhash {
""" + fx + """}

mod lexer {
""" + lexer + """}

mod prelude {
""" + prelude + """}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut sc = prelude::SelectCache::default();
    for path in &args[1..] {
        let src = std::fs::read_to_string(path).unwrap();
        let Ok(items) = lexer::split(&src) else {
            println!("{}: lex error", path);
            continue;
        };
        let p = prelude::select(&src, &items);
        let q = prelude::select_with(&src, &items, &mut sc);
        let mut h = cache::KeyHasher::new();
        h.add(&p);
        println!("{}: {} bytes {} lines {:032x} same={}", path, p.len(), p.lines().count(), h.finish(), p == q);
    }
}
""")
