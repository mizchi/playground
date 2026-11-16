//! 自前の構文木の上の型推論 (src/ast_infer.rs) を syn 版 (infer::infer) と比べる:
//! probe / examples のユーザの (ジェネリックでない) 関数で、let 文の型を前順に並べたものが同じか。
use crustc::front::{self, Program, Summary, TyEnv};
use crustc::lexer::ItemKind;
use syn::visit::Visit;

struct Locals(Vec<*const syn::Local>);
impl<'ast> Visit<'ast> for Locals {
    fn visit_local(&mut self, l: &'ast syn::Local) {
        self.0.push(l as *const syn::Local);
        syn::visit::visit_local(self, l);
    }
}

#[test]
fn infer_like_syn() {
    let mut files: Vec<std::path::PathBuf> = vec![];
    for d in ["tests/probe", "examples", "src"] {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    files.sort();
    // (crustc 自身のモジュールを 1 ファイルにまとめたもの: tests/selfhost.sh が作る)
    let big = std::path::PathBuf::from("out/selfhost/program_main.rs");
    if big.exists() {
        files.push(big);
    }
    let (mut fns, mut lets, mut bad) = (0, 0, vec![]);
    for p in &files {
        let src0 = std::fs::read_to_string(p).unwrap();
        let mut cache = crustc::prelude::SelectCache::default();
        let Ok((src, items)) = crustc::driver::with_prelude(&src0, &mut cache) else { continue };
        let user = cache.user_items;
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
        for id in 0..prog.names.len() {
            let Some(ii) = prog.item_of[id] else { continue };
            let sig = &prog.sigs[id];
            if ii as usize >= user || sig.is_generic() || !sig.has_body || prog.closures.contains_key(&(id as u32)) {
                continue;
            }
            let it = &items[ii as usize];
            let Ok(syn::Item::Fn(f)) = front::parse_item(&src, it) else { continue };
            let Ok(pf) = crustc::parse::parse_file(&src[it.start..it.end]) else { continue };
            let Some(crustc::parse::ItemKind::Fn(psig, Some(body))) = pf.items.first().map(|i| &i.kind) else { continue };
            if psig.params.iter().any(|(p, _)| !matches!(p, crustc::parse::Pat::Ident(_, _, _, None))) {
                continue;
            }
            let env = TyEnv { self_sid: sig.impl_of, params: vec![], self_ty: sig.self_ty.filter(|t| !prog.has_param(*t)) };
            let a = crustc::infer::infer(&prog, &f, sig, &env);
            let b = crustc::ast_infer::infer(&prog, psig.recv.is_some(), &psig.params, body, sig, &env);
            let mut la = Locals(vec![]);
            la.visit_item_fn(&f);
            let mut lb = vec![];
            crustc::ast_infer::lets_in_order(body, &mut lb);
            fns += 1;
            let ta: Vec<Option<_>> = la.0.iter().map(|&l| a.get(&(l as usize)).copied()).collect();
            let tb: Vec<Option<_>> = lb.iter().map(|&l| b.get(&(l as usize)).copied()).collect();
            lets += ta.len();
            if std::env::var("AST_INFER_SHOW").is_ok_and(|n| p.display().to_string().contains(&n)) {
                for (i, (x, y)) in ta.iter().zip(&tb).enumerate() {
                    eprintln!("  {}#{i}: {} | {}", prog.names[id], x.map(|t| prog.tn(t)).unwrap_or("?".into()), y.map(|t| prog.tn(t)).unwrap_or("?".into()));
                }
            }
            if ta != tb {
                let d: Vec<String> = ta
                    .iter()
                    .zip(&tb)
                    .enumerate()
                    .filter(|(_, (x, y))| x != y)
                    .take(3)
                    .map(|(i, (x, y))| format!("#{i}: {} vs {}", x.map(|t| prog.tn(t)).unwrap_or("?".into()), y.map(|t| prog.tn(t)).unwrap_or("?".into())))
                    .collect();
                bad.push(format!("{}: {} ({} vs {} lets) {}", p.display(), prog.names[id], ta.len(), tb.len(), d.join(", ")));
            }
        }
    }
    eprintln!("{fns} functions, {lets} lets, {} differ", bad.len());
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
    assert!(fns > 300, "only {fns} functions");
}
