//! 自前の構文木の上のクロージャ解析 (crates/core/src/ast_closure.rs) を syn 版 (crates/legacy/src/closure.rs) と比べる:
//! 全ファイルの関数の本体ごとに、クロージャの並び・自由変数・キャプチャを値として使うかが同じか。
use crustc::parse::{ItemKind, parse_file};

fn syn_bodies(items: &[syn::Item], out: &mut Vec<syn::Block>) {
    for it in items {
        match it {
            syn::Item::Fn(f) => out.push((*f.block).clone()),
            syn::Item::Impl(i) => {
                for ii in &i.items {
                    if let syn::ImplItem::Fn(f) = ii {
                        out.push(f.block.clone());
                    }
                }
            }
            syn::Item::Trait(t) => {
                for ti in &t.items {
                    if let syn::TraitItem::Fn(f) = ti {
                        if let Some(b) = &f.default {
                            out.push(b.clone());
                        }
                    }
                }
            }
            syn::Item::Mod(m) => {
                if let Some((_, is)) = &m.content {
                    syn_bodies(is, out);
                }
            }
            _ => {}
        }
    }
}

fn our_bodies(items: &[crustc::parse::Item], out: &mut Vec<crustc::parse::Block>) {
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
            ItemKind::Mod(_, Some(is)) => our_bodies(is, out),
            _ => {}
        }
    }
}

#[test]
fn closures_like_syn() {
    let mut files: Vec<std::path::PathBuf> = vec![];
    for d in ["tests/probe", "examples", "crates/core/src", "crates/legacy/src", "crates/minclif/src", "crates/clif/src"] {
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
        let (mut sb, mut ob) = (vec![], vec![]);
        syn_bodies(&sf.items, &mut sb);
        our_bodies(&ours.items, &mut ob);
        if sb.len() != ob.len() {
            bad.push(format!("{}: {} vs {} bodies", p.display(), sb.len(), ob.len()));
            continue;
        }
        for (k, (s, o)) in sb.iter().zip(&ob).enumerate() {
            let sc = crustc::closure::closures_of(s);
            let oc = crustc::ast_closure::closures_of(o);
            if sc.len() != oc.len() {
                bad.push(format!("{} body {k}: {} vs {} closures", p.display(), sc.len(), oc.len()));
                continue;
            }
            for (j, (a, b)) in sc.iter().zip(&oc).enumerate() {
                total += 1;
                let fa = crustc::closure::free_vars(a);
                let fb = crustc::ast_closure::free_vars(b);
                let ma = crustc::closure::moves_capture(a, &fa);
                let mb = crustc::ast_closure::moves_capture(b, &fb);
                if fa != fb || ma != mb {
                    bad.push(format!("{} body {k} closure {j}: syn {fa:?} {ma} / ours {fb:?} {mb}", p.display()));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{} differences over {total} closures:\n{}", bad.len(), bad.join("\n"));
    eprintln!("{total} closures checked");
}
