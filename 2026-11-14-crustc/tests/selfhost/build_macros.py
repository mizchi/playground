"""crustc 自身の crates/syntax/src/macros.rs (macro_rules! の展開と include_str!) を 1 ファイルのプログラムにまとめる。
proc_macro2 クレートは使えないので、使う分だけの代わり (tests/selfhost/mini_pm2.rs) を付ける
(rustc 版も同じ代わりを使う)。
usage: python3 tests/selfhost/build_macros.py > out/selfhost/macros_main.rs
main は引数のファイルごとに macros::expand を呼び、展開した結果の長さ・行数・ハッシュ (とエラー) を出す。"""

macros = open("crates/syntax/src/macros.rs").read()
pm2 = open("tests/selfhost/mini_pm2.rs").read()
# (expand は lexer::tokenize を使う。lexer は cache の Key / KeyHasher を使う)
lexer = open("crates/syntax/src/lexer.rs").read()
key = open("crates/base/src/key.rs").read()
cfg = open("crates/syntax/src/cfg.rs").read()
print("""// 自動生成: tests/selfhost/build_macros.py (crustc の crates/syntax/src/macros.rs をそのまま含む)
""" + pm2 + """
mod cache {
""" + key + """
}

mod lexer {
""" + lexer + """
}

mod cfg {
""" + cfg + """
}

mod macros {
""" + macros + """
use crate::proc_macro2;
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    for path in &args[1..] {
        let src = std::fs::read_to_string(path).unwrap();
        match macros::expand(&src) {
            Ok(Some(t)) => println!("{path}: expanded {} bytes {} lines {:016x}", t.len(), t.lines().count(), fnv(&t)),
            Ok(None) => println!("{path}: no macros"),
            Err(e) => println!("{path}: error {e}"),
        }
        let dir = std::path::Path::new(path).parent().unwrap_or(std::path::Path::new("."));
        match macros::expand_includes(&src, dir) {
            Ok(Some(t)) => println!("  includes: {} bytes {:016x}", t.len(), fnv(&t)),
            Ok(None) => {}
            Err(e) => println!("  includes error: {e}"),
        }
    }
}
""")
# (lexer は crate::key を使う: cache と同じもの)
print("mod key {\n    pub use crate::cache::*;\n}")
