//! 自前の構文木からの型の変換 (src/ast_types.rs) を syn 版 (front::parse_tysyn) と比べる:
//! 全ファイルのアイテムのシグネチャ・フィールド・impl の self 型・別名・const / static の型。
use crustc::front::{TySyn, parse_tysyn};
use crustc::parse::{Fields, ItemKind, parse_file};

fn syn_types(items: &[syn::Item], self_ty: Option<&TySyn>, out: &mut Vec<Result<TySyn, String>>) {
    let mut fn_sig = |sig: &syn::Signature, st: Option<&TySyn>, out: &mut Vec<Result<TySyn, String>>| {
        for a in &sig.inputs {
            if let syn::FnArg::Typed(pt) = a {
                out.push(parse_tysyn(&pt.ty, st));
            }
        }
        if let syn::ReturnType::Type(_, t) = &sig.output {
            out.push(parse_tysyn(t, st));
        }
    };
    for it in items {
        match it {
            syn::Item::Fn(f) => fn_sig(&f.sig, self_ty, out),
            syn::Item::Struct(s) => {
                for f in &s.fields {
                    out.push(parse_tysyn(&f.ty, None));
                }
            }
            syn::Item::Enum(e) => {
                for v in &e.variants {
                    for f in &v.fields {
                        out.push(parse_tysyn(&f.ty, None));
                    }
                }
            }
            syn::Item::Impl(i) => {
                let st = parse_tysyn(&i.self_ty, None);
                out.push(st.clone());
                for ii in &i.items {
                    match ii {
                        syn::ImplItem::Fn(f) => fn_sig(&f.sig, st.as_ref().ok(), out),
                        syn::ImplItem::Type(t) => out.push(parse_tysyn(&t.ty, st.as_ref().ok())),
                        syn::ImplItem::Const(c) => out.push(parse_tysyn(&c.ty, st.as_ref().ok())),
                        _ => {}
                    }
                }
            }
            syn::Item::Type(t) => out.push(parse_tysyn(&t.ty, None)),
            syn::Item::Const(c) => out.push(parse_tysyn(&c.ty, None)),
            syn::Item::Static(s) => out.push(parse_tysyn(&s.ty, None)),
            syn::Item::Mod(m) => {
                if let Some((_, is)) = &m.content {
                    syn_types(is, None, out);
                }
            }
            _ => {}
        }
    }
}

fn our_types(items: &[crustc::parse::Item], self_ty: Option<&TySyn>, out: &mut Vec<Result<TySyn, String>>) {
    use crustc::ast_types::tysyn;
    let fn_sig = |sig: &crustc::parse::FnSig, st: Option<&TySyn>, out: &mut Vec<Result<TySyn, String>>| {
        for (_, t) in &sig.params {
            out.push(tysyn(t, st));
        }
        if let Some(t) = &sig.ret {
            out.push(tysyn(t, st));
        }
    };
    let fields = |f: &Fields, out: &mut Vec<Result<TySyn, String>>| match f {
        Fields::Unit => {}
        Fields::Tuple(v) => {
            for t in v {
                out.push(tysyn(t, None));
            }
        }
        Fields::Named(v) => {
            for (_, t) in v {
                out.push(tysyn(t, None));
            }
        }
    };
    for it in items {
        match &it.kind {
            ItemKind::Fn(sig, _) => fn_sig(sig, self_ty, out),
            ItemKind::Struct(_, _, f) => fields(f, out),
            ItemKind::Enum(_, _, vs) => {
                for v in vs {
                    fields(&v.1, out);
                }
            }
            ItemKind::Impl(_, _, t, is) => {
                let st = tysyn(t, None);
                out.push(st.clone());
                for i in is {
                    match &i.kind {
                        ItemKind::Fn(sig, _) => fn_sig(sig, st.as_ref().ok(), out),
                        ItemKind::Type(_, _, _, Some(t)) => out.push(tysyn(t, st.as_ref().ok())),
                        ItemKind::Const(_, Some(t), _) => out.push(tysyn(t, st.as_ref().ok())),
                        _ => {}
                    }
                }
            }
            ItemKind::Type(_, _, _, Some(t)) => out.push(tysyn(t, None)),
            ItemKind::Const(_, Some(t), _) => out.push(tysyn(t, None)),
            ItemKind::Static(_, _, t, _) => out.push(tysyn(t, None)),
            ItemKind::Mod(_, Some(is)) => our_types(is, None, out),
            _ => {}
        }
    }
}

#[test]
fn types_like_syn() {
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
    let mut bad = vec![];
    let mut total = 0;
    for p in &files {
        let src = std::fs::read_to_string(p).unwrap();
        let Ok(sf) = syn::parse_file(&src) else { continue };
        let ours = parse_file(&src).unwrap();
        let (mut a, mut b) = (vec![], vec![]);
        syn_types(&sf.items, None, &mut a);
        our_types(&ours.items, None, &mut b);
        if a.len() != b.len() {
            bad.push(format!("{}: {} vs {} types", p.display(), a.len(), b.len()));
            continue;
        }
        for (k, (x, y)) in a.iter().zip(&b).enumerate() {
            total += 1;
            let same = match (x, y) {
                (Ok(x), Ok(y)) => x == y,
                (Err(_), Err(_)) => true,
                _ => false,
            };
            if !same {
                bad.push(format!("{} #{k}: syn {x:?}\n    ours {y:?}", p.display()));
            }
        }
    }
    assert!(bad.is_empty(), "{} differences over {total} types:\n{}", bad.len(), bad.join("\n"));
    eprintln!("{total} types checked");
}
