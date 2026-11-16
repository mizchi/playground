//! 自前の構文木の上の定数評価 (src/ast_consteval.rs) を syn 版 (consteval::eval) と比べる:
//! probe / examples の全 const / static の初期値で、バイト列・配置・リロケーションが同じか。
use crustc::ast_consteval as ac;
use crustc::consteval as sc;
use crustc::front::{self, Program, Summary};
use crustc::lexer::ItemKind;

fn dump_syn(dv: &sc::DataVal, indent: usize, out: &mut Vec<String>) {
    let hex: String = dv.bytes.iter().map(|b| format!("{b:02x}")).collect();
    out.push(format!("{}align={} {hex}", " ".repeat(indent), dv.align));
    for (o, r) in &dv.relocs {
        match r {
            sc::DRef::Static(c) => out.push(format!("{}@{o} static {c}", " ".repeat(indent + 1))),
            sc::DRef::Anon(inner) => {
                out.push(format!("{}@{o} anon", " ".repeat(indent + 1)));
                dump_syn(inner, indent + 2, out);
            }
        }
    }
}

#[test]
fn consteval_like_syn() {
    let mut files: Vec<std::path::PathBuf> = vec![];
    for d in ["tests/probe", "examples"] {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    files.sort();
    let (mut total, mut bad) = (0, vec![]);
    for p in &files {
        let src0 = std::fs::read_to_string(p).unwrap();
        let mut cache = crustc::prelude::SelectCache::default();
        let Ok((src, items)) = crustc::driver::with_prelude(&src0, &mut cache) else { continue };
        let mut sums: Vec<Summary> = vec![Summary::default(); items.len()];
        for pass in 0..2 {
            for (i, it) in items.iter().enumerate() {
                let header = matches!(it.kind, ItemKind::ImplHeader | ItemKind::TraitHeader);
                if header != (pass == 0) {
                    continue;
                }
                let ow = if pass == 1 { crustc::ast_summary::owner_of(it, &sums) } else { None };
                if let Ok((s, _)) = front::parse_and_summarize(&src, it, ow.as_ref()) {
                    sums[i] = s;
                }
            }
        }
        let Ok(prog) = Program::build(&items, &sums) else { continue };
        for c in &prog.consts {
            total += 1;
            let a = syn::parse_str::<syn::Expr>(&c.expr).map_err(|e| e.to_string()).and_then(|e| sc::eval(&prog, &e, c.ty).map_err(|e| e.to_string()));
            let b = ac::eval_src(&prog, &c.expr, c.ty);
            let (mut x, mut y) = (vec![], vec![]);
            match (&a, &b) {
                (Ok(da), Ok(db)) => {
                    dump_syn(da, 0, &mut x);
                    ac::dump(db, 0, &mut y);
                    if x != y {
                        bad.push(format!("{}: {}: {x:?} vs {y:?}", p.display(), c.name));
                    }
                }
                (Err(_), Err(_)) => {}
                _ => bad.push(format!("{}: {}: {:?} vs {:?}", p.display(), c.name, a.as_ref().err(), b.as_ref().err())),
            }
        }
    }
    assert!(bad.is_empty(), "{} / {total} differ:\n{}", bad.len(), bad.join("\n"));
    assert!(total > 500, "only {total} constants");
    eprintln!("{total} constants");
}
