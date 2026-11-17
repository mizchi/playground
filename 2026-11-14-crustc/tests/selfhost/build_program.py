"""crustc 自身の crates/sema/src/program.rs (全体の表: 関数シグネチャ・struct / enum のレイアウト・trait 実装) を、要約までの
モジュール (lexer / prelude / types / parse / ast_types / ast_closure / ast_summary) と一緒に 1 ファイルのプログラムにまとめる。
usage: python3 tests/selfhost/build_program.py > out/selfhost/program_main.rs
main は引数のファイルごとに、driver と同じ手順 (モジュールを平らに・入れ子のアイテムを外へ・prelude を後ろに付ける) で
アイテムを作り、全部を要約して Program::build し、関数・型 (サイズ・配置・フィールドのオフセット)・trait 実装・const
(と crates/sema/src/ast_consteval.rs で評価した初期値のデータ) を出す。
(macro_rules! の展開はしない: マクロを使うファイルは展開前のまま扱う)"""
import glob, os

def body(p):
    return open(p).read()

items = max(glob.glob("target/release/build/crustc-syntax*/**/prelude_items.rs", recursive=True), key=os.path.getmtime)
prelude = (body("crates/syntax/src/prelude.rs")
    .replace('include_str!("prelude_lib.rs.in")', 'include_str!("../../crates/syntax/src/prelude_lib.rs.in")')
    .replace('include!(concat!(env!("OUT_DIR"), "/prelude_items.rs"));', open(items).read()))
parts = [("key", body("crates/base/src/key.rs")), ("fxhash", body("crates/base/src/fxhash.rs")), ("lexer", body("crates/syntax/src/lexer.rs")),
         ("prelude", prelude), ("types", body("crates/syntax/src/types.rs")), ("parse", body("crates/syntax/src/parse.rs")),
         ("ast_types", body("crates/syntax/src/ast_types.rs")), ("ast_closure", body("crates/syntax/src/ast_closure.rs")),
         ("ast_summary", body("crates/syntax/src/ast_summary.rs")), ("program", body("crates/sema/src/program.rs")),
         ("ast_consteval", body("crates/sema/src/ast_consteval.rs")), ("solver", body("crates/sema/src/solver.rs")), ("ctors", body("crates/syntax/src/ctors.rs")), ("ast_infer", body("crates/sema/src/ast_infer.rs"))]
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
    // (ctors.rs: 値として使ったバリアントをクロージャに)
    let src = match ctors::rewrite_ctor_values(&src) {
        Some(c) => {
            println!("ctors: {} -> {} bytes", src.len(), c.len());
            c
        }
        None => src,
    };
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
    // 型推論の単一化器 (solver.rs): ジェネリックな関数を、型引数を i64 にした実引数で呼んだときの戻り値型
    let env = program::TyEnv::default();
    let mut solved = 0;
    for (i, n) in p.names.iter().enumerate() {
        let sig = &p.sigs[i];
        if sig.generics.is_empty() {
            continue;
        }
        let targs = vec![types::Ty::I64; sig.generics.len()];
        let mut sv = solver::Solver::new(&p, &env);
        let args: Vec<u32> = sig.params.iter().map(|&t| { let t = p.subst(t, &targs); sv.known(t) }).collect();
        let r = sv.apply(Some(i as u32), args, 0);
        sv.finish_late_bounds();
        sv.finish_collects();
        sv.finish_assocs();
        let rt = sv.resolve(r);
        println!("  solve {n} -> {rt:?}");
        solved += 1;
    }
    println!("  {solved} solved");
    // 型推論 (ast_infer.rs): ユーザの (ジェネリックでない) 関数の let の型を前順に
    let mut nf = 0;
    for id in 0..p.names.len() {
        let Some(ii) = p.item_of[id] else { continue };
        let sig = &p.sigs[id];
        if ii as usize >= user || sig.is_generic() || !sig.has_body || p.closures.contains_key(&(id as u32)) {
            continue;
        }
        let it = &items[ii as usize];
        let Ok(pf) = parse::parse_file(&src[it.start..it.end]) else { continue };
        let Some(parse::ItemKind::Fn(psig, Some(body))) = pf.items.first().map(|i| &i.kind) else { continue };
        if psig.params.iter().any(|(q, _)| !matches!(q, parse::Pat::Ident(_, _, _, None))) {
            continue;
        }
        let env = program::TyEnv { self_sid: sig.impl_of, params: vec![], self_ty: sig.self_ty.filter(|t| !p.has_param(*t)) };
        let lt = ast_infer::infer(&p, psig.recv.is_some(), &psig.params, body, sig, &env);
        let mut ls = vec![];
        ast_infer::lets_in_order(body, &mut ls);
        let ts: Vec<String> = ls.iter().map(|&l| lt.get(&(l as usize)).map(|&t| p.tn(t)).unwrap_or("?".into())).collect();
        println!("  infer {} [{}]", p.names[id], ts.join(", "));
        nf += 1;
    }
    println!("  {nf} inferred");
    // パスの検索 (program.rs の fn_by_p / struct_by_p / variant_by_p): 全関数の本体のパスを引く
    if let Ok(f) = parse::parse_file(&src) {
        let mut n = 0;
        for it in &f.items {
            lookups(&p, it, None, &mut n);
        }
        println!("  {n} paths");
    }
    Ok(())
}

fn walk(e: &parse::Expr, f: &mut dyn FnMut(&parse::Expr)) {
    f(e);
    ast_closure::each_child_pub(e, &mut |c| walk(c, f));
}

fn lookups(p: &program::Program, it: &parse::Item, self_sid: Option<u32>, n: &mut usize) {
    match &it.kind {
        parse::ItemKind::Fn(_, Some(b)) => {
            let e = parse::Expr::Block(None, false, b.clone());
            walk(&e, &mut |x| {
                let path = match x {
                    parse::Expr::Path(None, q) | parse::Expr::Struct(q, _, _) => Some(q),
                    _ => None,
                };
                if let Some(q) = path {
                    let pp = program::PPath::of(q);
                    let r = (p.fn_by_p(&pp, self_sid), p.struct_by_p(&pp, self_sid), p.variant_by_p(&pp, self_sid));
                    if r != (None, None, None) {
                        let names: Vec<&str> = pp.segs.iter().map(|s| s.name.as_str()).collect();
                        println!("    {} {:?}", names.join("::"), r);
                    }
                    *n += 1;
                }
            });
        }
        parse::ItemKind::Impl(_, _, parse::Ty::Path(None, q), items) => {
            let sid = p.struct_by_p(&program::PPath::of(q), None);
            for i in items {
                lookups(p, i, sid, n);
            }
        }
        _ => {}
    }
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
