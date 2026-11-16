//! 自前の構文解析 (crates/core/src/parse.rs) を syn と比べる: probe / examples / src の全ファイルを解析し、
//! 節の種類ごとの数が syn で数えたものと一致するか。
use crustc::parse::{Counts, parse_file};
use syn::visit::Visit;

#[derive(Default)]
struct SynCounts(Counts);

impl<'a> Visit<'a> for SynCounts {
    fn visit_item_fn(&mut self, f: &'a syn::ItemFn) {
        self.0.fns += 1;
        syn::visit::visit_item_fn(self, f);
    }
    fn visit_impl_item_fn(&mut self, f: &'a syn::ImplItemFn) {
        self.0.fns += 1;
        syn::visit::visit_impl_item_fn(self, f);
    }
    fn visit_trait_item_fn(&mut self, f: &'a syn::TraitItemFn) {
        self.0.fns += 1;
        syn::visit::visit_trait_item_fn(self, f);
    }
    fn visit_foreign_item_fn(&mut self, f: &'a syn::ForeignItemFn) {
        self.0.fns += 1;
        syn::visit::visit_foreign_item_fn(self, f);
    }
    fn visit_item_struct(&mut self, _: &'a syn::ItemStruct) {
        self.0.structs += 1;
    }
    fn visit_item_enum(&mut self, e: &'a syn::ItemEnum) {
        self.0.enums += 1;
        syn::visit::visit_item_enum(self, e);
    }
    fn visit_item_impl(&mut self, i: &'a syn::ItemImpl) {
        self.0.impls += 1;
        syn::visit::visit_item_impl(self, i);
    }
    fn visit_item_trait(&mut self, i: &'a syn::ItemTrait) {
        self.0.traits += 1;
        syn::visit::visit_item_trait(self, i);
    }
    fn visit_local(&mut self, l: &'a syn::Local) {
        self.0.lets += 1;
        syn::visit::visit_local(self, l);
    }
    fn visit_macro(&mut self, _: &'a syn::Macro) {
        self.0.macros += 1;
    }
    // (型・パターン・属性の中の式は数えない: 自前の方も数えない)
    fn visit_type(&mut self, _: &'a syn::Type) {}
    fn visit_pat(&mut self, _: &'a syn::Pat) {}
    fn visit_attribute(&mut self, _: &'a syn::Attribute) {}
    fn visit_generic_argument(&mut self, _: &'a syn::GenericArgument) {}
    fn visit_expr(&mut self, e: &'a syn::Expr) {
        use syn::Expr::*;
        let c = &mut self.0;
        match e {
            Lit(_) => c.lits += 1,
            Path(_) => c.paths += 1,
            Reference(_) | RawAddr(_) => c.refs += 1,
            Binary(_) => c.binaries += 1,
            Call(_) => c.calls += 1,
            MethodCall(_) => c.method_calls += 1,
            Field(_) => c.fields += 1,
            Index(_) => c.indexes += 1,
            Cast(_) => c.casts += 1,
            If(_) => c.ifs += 1,
            Match(_) => c.matches += 1,
            While(_) | Loop(_) | ForLoop(_) => c.loops += 1,
            Return(_) => c.returns += 1,
            Closure(_) => c.closures += 1,
            Struct(_) => c.struct_lits += 1,
            _ => {}
        }
        syn::visit::visit_expr(self, e);
    }
}

#[test]
fn parses_like_syn() {
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
    for p in &files {
        let src = std::fs::read_to_string(p).unwrap();
        let Ok(sf) = syn::parse_file(&src) else { continue };
        let mut sc = SynCounts::default();
        sc.visit_file(&sf);
        match parse_file(&src) {
            Ok(f) => {
                let mut c = Counts::default();
                c.file(&f);
                if c != sc.0 {
                    bad.push(format!("{}:\n  ours {:?}\n  syn  {:?}", p.display(), c, sc.0));
                }
            }
            Err(e) => bad.push(format!("{}: {e}", p.display())),
        }
    }
    assert!(bad.is_empty(), "{} / {} files differ:\n{}", bad.len(), files.len(), bad.join("\n"));
}
