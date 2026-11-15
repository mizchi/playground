//! prelude (src/prelude_lib.rs.in) のアイテム分割をビルド時に済ませる。
//! 実行時に毎回 lexer で prelude 全体 (数千行) を分割すると、小さいプログラムのビルド時間の半分を占めるため。
//! 分割には src/lexer.rs をそのまま使う (ハッシュは prelude のアイテムでは使わないので 0)。

use std::fmt::Write;

#[path = "src/lexer.rs"]
#[allow(dead_code)]
mod lexer;

#[allow(dead_code)]
mod cache {
    pub type Key = u128;
    pub struct KeyHasher;
    impl KeyHasher {
        pub fn new() -> Self {
            KeyHasher
        }
        pub fn add<T: ?Sized>(&mut self, _v: &T) {}
        pub fn finish(&self) -> Key {
            0
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=src/prelude_lib.rs.in");
    println!("cargo:rerun-if-changed=src/lexer.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let src = std::fs::read_to_string("src/prelude_lib.rs.in").unwrap();
    let items = lexer::split(&src).expect("prelude must lex");
    let mut out = String::new();
    out.push_str("// build.rs が生成する: prelude のアイテム (start, end, line, kind, kind の引数, 識別子)\n");
    out.push_str("pub static PRELUDE_ITEMS: &[(u32, u32, u32, u8, u32, &[(u32, u32)])] = &[\n");
    for it in &items {
        let (k, a) = match it.kind {
            lexer::ItemKind::Normal => (0, 0),
            lexer::ItemKind::ImplHeader => (1, 0),
            lexer::ItemKind::ImplMember(h) => (2, h),
            lexer::ItemKind::TraitHeader => (3, 0),
            lexer::ItemKind::TraitMember(h) => (4, h),
            lexer::ItemKind::InnerAttr => (5, 0),
        };
        write!(out, "({}, {}, {}, {k}, {a}, &[", it.start, it.end, it.line).unwrap();
        for (x, y) in &it.idents {
            write!(out, "({x}, {y}),").unwrap();
        }
        out.push_str("]),\n");
    }
    out.push_str("];\n");
    let dst = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("prelude_items.rs");
    std::fs::write(dst, out).unwrap();
}
