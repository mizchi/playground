"""crustc 自身の src/types.rs (型の表現と intern) / src/parse.rs / src/ast_types.rs (型の構文 → TySyn) を
1 ファイルのプログラムにまとめる。
usage: python3 tests/selfhost/build_ast_types.py > out/selfhost/ast_types_main.rs
main は引数のファイルごとに、アイテムの型を TySyn にして出し、intern した型の性質 (大きさ・整数か) も出す。"""

types = open("src/types.rs").read()
parse = open("src/parse.rs").read()
at = open("src/ast_types.rs").read()
print("""// 自動生成: tests/selfhost/build_ast_types.py (crustc の src/types.rs / parse.rs / ast_types.rs をそのまま含む)
mod types {
""" + types + """}

mod parse {
""" + parse + """}

mod ast_types {
""" + at + """}

use parse::{Fields, Item, ItemKind, parse_file};
use types::{Ty, TySyn, intern};

fn collect(items: &[Item], self_ty: Option<&TySyn>, out: &mut Vec<String>) {
    let show = |r: Result<TySyn, String>| match r {
        Ok(t) => format!("{t:?}"),
        Err(e) => format!("error {e}"),
    };
    for it in items {
        match &it.kind {
            ItemKind::Fn(sig, _) => {
                for (_, t) in &sig.params {
                    out.push(show(ast_types::tysyn(t, self_ty)));
                }
                if let Some(t) = &sig.ret {
                    out.push(show(ast_types::tysyn(t, self_ty)));
                }
            }
            ItemKind::Struct(_, _, Fields::Named(v)) => {
                for (_, t) in v {
                    out.push(show(ast_types::tysyn(t, None)));
                }
            }
            ItemKind::Struct(_, _, Fields::Tuple(v)) => {
                for t in v {
                    out.push(show(ast_types::tysyn(t, None)));
                }
            }
            ItemKind::Impl(_, _, t, is) => {
                let st = ast_types::tysyn(t, None);
                out.push(show(st.clone()));
                collect(is, st.as_ref().ok(), out);
            }
            ItemKind::Mod(_, Some(is)) => collect(is, None, out),
            _ => {}
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    for path in &args[1..] {
        let src = std::fs::read_to_string(path).unwrap();
        let Ok(f) = parse_file(&src) else {
            println!("{path}: parse error");
            continue;
        };
        let mut out = Vec::new();
        collect(&f.items, None, &mut out);
        println!("{path}: {} types", out.len());
        for l in out {
            println!("  {l}");
        }
    }
    // intern: 同じ型は同じアドレス
    let a = intern(Ty::Ref(false, intern(Ty::U8)));
    let b = intern(Ty::Ref(false, intern(Ty::U8)));
    println!("intern same: {} {:?} {} {}", std::ptr::eq(a, b), Ty::prim("u16"), Ty::U32.bits(), Ty::I64.is_int());
}
""")
