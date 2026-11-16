"""crustc 自身の src/program.rs (全体の表: 関数シグネチャ・struct / enum のレイアウト・trait 実装) を、要約までの
モジュール (lexer / prelude / types / parse / ast_types / ast_closure / ast_summary) と一緒に 1 ファイルのプログラムにまとめる。
usage: python3 tests/selfhost/build_program.py > out/selfhost/program_main.rs
main は引数のファイルごとに、driver と同じ手順 (モジュールを平らに・入れ子のアイテムを外へ・prelude を後ろに付ける) で
アイテムを作り、全部を要約して Program::build し、関数・型 (サイズ・配置・フィールドのオフセット)・trait 実装・const
(と src/ast_consteval.rs で評価した初期値のデータ) を出す。
(macro_rules! の展開はしない: マクロを使うファイルは展開前のまま扱う)"""
import glob, os

def body(p):
    return open(p).read()

items = max(glob.glob("target/release/build/crustc-*/out/prelude_items.rs"), key=os.path.getmtime)
prelude = (body("src/prelude.rs")
    .replace('include_str!("prelude_lib.rs.in")', 'include_str!("../../src/prelude_lib.rs.in")')
    .replace('include!(concat!(env!("OUT_DIR"), "/prelude_items.rs"));', open(items).read()))
parts = [("key", body("src/key.rs")), ("fxhash", body("src/fxhash.rs")), ("lexer", body("src/lexer.rs")),
         ("prelude", prelude), ("types", body("src/types.rs")), ("parse", body("src/parse.rs")),
         ("ast_types", body("src/ast_types.rs")), ("ast_closure", body("src/ast_closure.rs")),
         ("ast_summary", body("src/ast_summary.rs")), ("program", body("src/program.rs")),
         ("ast_consteval", body("src/ast_consteval.rs"))]
out = ["// 自動生成: tests/selfhost/build_program.py (crustc の src/*.rs をそのまま含む)", "mod cache {\n    pub use crate::key::*;\n}\n"]
for name, src in parts:
    out.append(f"mod {name} {{\n" + src + "}\n")
out.append(r"""
use lexer::{ItemKind, SrcItem};
use types::Summary;

/// driver.rs の with_prelude と同じ: ユーザのアイテムの後ろに prelude のアイテムを付ける
fn with_prelude(src0: &str) -> Result<(String, Vec<SrcItem>, usize), String> {
    let s1 = lexer::flatten_modules(src0).unwrap_or(src0.to_string());
    let src = lexer::hoist_nested_items(&s1).unwrap_or(s1);
    let mut items = lexer::split(&src)?;
    let user = items.len();
    let extra = prelude::select(&src, &items);
    if extra.is_empty() {
        return Ok((src, items, user));
    }
    let base = src.len() + 1;
    let lines = src.bytes().filter(|&c| c == b'\n').count() + 1;
    let n0 = items.len() as u32;
    for mut it in lexer::split(&extra)? {
        it.start += base;
        it.end += base;
        it.line += lines;
        for r in &mut it.idents {
            r.0 += base as u32;
            r.1 += base as u32;
        }
        it.kind = match it.kind {
            ItemKind::ImplMember(h) => ItemKind::ImplMember(h + n0),
            ItemKind::TraitMember(h) => ItemKind::TraitMember(h + n0),
            k => k,
        };
        items.push(it);
    }
    Ok((format!("{src}\n{extra}"), items, user))
}

fn run(path: &str) -> Result<(), String> {
    let src0 = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let (src, items, user) = with_prelude(&src0)?;
    let mut sums: Vec<Summary> = vec![Summary::default(); items.len()];
    for pass in 0..2 {
        for (i, it) in items.iter().enumerate() {
            let header = matches!(it.kind, ItemKind::ImplHeader | ItemKind::TraitHeader);
            if header != (pass == 0) {
                continue;
            }
            let ow = if pass == 1 { ast_summary::owner_of(it, &sums) } else { None };
            sums[i] = ast_summary::parse_and_summarize(&src, it, ow.as_ref())?;
        }
    }
    let p = program::Program::build(&items, &sums)?;
    println!("{path}: {} items ({user} user), {} fns, {} types", items.len(), p.names.len(), p.structs.len());
    for (i, n) in p.names.iter().enumerate() {
        let s = &p.sigs[i];
        println!("  fn {n} {:?} -> {:?} impl_of={:?} trait={:?} generics={:?}", s.params, s.ret, s.impl_of, s.trait_of, s.generics);
    }
    for i in 0..p.structs.len() {
        let d = &p.structs[i];
        let fs: Vec<String> = d.fields.iter().map(|(n, t, o)| format!("{n}:{t:?}@{o}")).collect();
        println!("  type {} size={} align={} deep={:032x} [{}]", d.name, d.size, d.align, d.deep, fs.join(", "));
        for v in &d.variants {
            let fs: Vec<String> = v.fields.iter().map(|(n, t, o)| format!("{n}:{t:?}@{o}")).collect();
            println!("    {} = {} [{}]", v.name, v.discr, fs.join(", "));
        }
    }
    let mut im: Vec<String> = p.impls.iter().map(|(s, t)| format!("{} {t}", p.structs[*s as usize].name)).collect();
    im.sort();
    println!("  impls {}", im.join("; "));
    for c in &p.consts {
        println!("  const {}: {:?} = {}", c.name, c.ty, c.expr);
        // (ast_consteval: 初期値のデータ)
        match ast_consteval::eval_src(&p, &c.expr, c.ty) {
            Ok(dv) => {
                let mut ls = vec![];
                ast_consteval::dump(&dv, 4, &mut ls);
                for l in ls {
                    println!("{l}");
                }
            }
            Err(e) => println!("    error {e}"),
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    for path in &args[1..] {
        if let Err(e) = run(path) {
            println!("{path}: error {e}");
        }
    }
}
""")
print("\n".join(out))
