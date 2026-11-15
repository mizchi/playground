//! クロージャの持ち上げ (lambda lifting)。
//!
//! 関数本体の中の k 番目 (前順) のクロージャ式を、要約の段階で次の 2 つにする:
//!
//! - struct `親::{closure#k}<親の型引数.., __C0.., __P0.., __R>`: フィールドはキャプチャ。
//!   `move` なら値そのもの (`__Ci = T`)、そうでなければ変数へのポインタ (`__Ci = *mut T`)
//! - fn `親::{closure#k}::call(__env: *mut その struct, 引数..) -> __R`
//!
//! 型引数のうち __C / __P / __R はクロージャを作る側 (親の codegen) が決める。
//! キャプチャ・引数・戻り値の型がすべて struct の型引数に入るので、クロージャの型だけから
//! 本体のインスタンスが決まり、既存の単相化とキャッシュ (インスタンス単位) にそのまま乗る。
//! 本体は親のアイテムを parse し直して k 番目を取り出し、[`synth`] で普通の fn にして
//! コンパイルする (キャプチャは codegen が `__env` のフィールドとしてスコープに入れる)。

use std::collections::HashSet;

use syn::visit::Visit;
use syn::{Expr, ExprClosure, Pat};

/// クロージャの識別子: 最初の `|` の位置 (行, 列)。マクロの引数は codegen で parse し直すので
/// ノードのアドレスでは同じクロージャを指せないが、トークンの位置は変わらない
pub fn closure_key(c: &ExprClosure) -> (usize, usize) {
    let s = c.or1_token.spans[0].start();
    (s.line, s.column)
}

/// マクロの引数を式の列として読む (クロージャ (`|`) を含むものだけ。`vec![x; n]` も)
pub fn macro_exprs(m: &syn::Macro) -> Vec<Expr> {
    fn has_pipe(ts: &proc_macro2::TokenStream) -> bool {
        ts.clone().into_iter().any(|t| match t {
            proc_macro2::TokenTree::Punct(p) => p.as_char() == '|',
            proc_macro2::TokenTree::Group(g) => has_pipe(&g.stream()),
            _ => false,
        })
    }
    if !has_pipe(&m.tokens) {
        return vec![];
    }
    use syn::punctuated::Punctuated;
    if let Ok(es) = m.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated) {
        return es.into_iter().collect();
    }
    let ts = &m.tokens;
    match syn::parse2::<Expr>(quote::quote!([#ts])) {
        Ok(e) => vec![e],
        Err(_) => vec![],
    }
}

/// 本体の中のクロージャ式 (前順)。入れ子のクロージャは外側の直後に並ぶ。
/// マクロの引数の中も数える (codegen も同じ順に数え、[`closure_key`] で対応を取る)
pub fn closures_of(b: &syn::Block) -> Vec<ExprClosure> {
    struct V(Vec<ExprClosure>);
    impl<'a> Visit<'a> for V {
        fn visit_expr_closure(&mut self, c: &'a ExprClosure) {
            self.0.push(c.clone());
            syn::visit::visit_expr_closure(self, c);
        }
        fn visit_macro(&mut self, m: &'a syn::Macro) {
            for e in macro_exprs(m) {
                self.visit_expr(&e);
            }
        }
        fn visit_item(&mut self, _: &'a syn::Item) {}
    }
    let mut v = V(vec![]);
    v.visit_block(b);
    v.0
}

/// パターンが束縛する名前
pub fn pat_idents(p: &Pat, out: &mut Vec<String>) {
    struct V<'a>(&'a mut Vec<String>);
    impl<'a> Visit<'a> for V<'_> {
        fn visit_pat_ident(&mut self, pi: &'a syn::PatIdent) {
            self.0.push(pi.ident.to_string());
            syn::visit::visit_pat_ident(self, pi);
        }
        fn visit_expr(&mut self, _: &'a Expr) {}
    }
    V(out).visit_pat(p);
}

/// 関数 (入れ子のクロージャを含む) のどこかで束縛される名前。キャプチャ候補の上限
pub fn bound_in_fn(f: &syn::ItemFn) -> HashSet<String> {
    struct V(Vec<String>);
    impl<'a> Visit<'a> for V {
        fn visit_pat(&mut self, p: &'a Pat) {
            pat_idents(p, &mut self.0);
            syn::visit::visit_pat(self, p);
        }
        fn visit_item(&mut self, _: &'a syn::Item) {}
    }
    let mut v = V(vec![]);
    for a in &f.sig.inputs {
        match a {
            syn::FnArg::Receiver(_) => v.0.push("self".into()),
            syn::FnArg::Typed(pt) => v.visit_pat(&pt.pat),
        }
    }
    v.visit_block(&f.block);
    v.0.into_iter().collect()
}

/// クロージャの自由変数 (最初に現れた順)。スコープを辿り、中で束縛された名前は除く
pub fn free_vars(c: &ExprClosure) -> Vec<String> {
    struct V {
        scopes: Vec<Vec<String>>,
        out: Vec<String>,
    }
    impl V {
        fn bound(&self, n: &str) -> bool {
            self.scopes.iter().any(|s| s.iter().any(|x| x == n))
        }
        fn bind(&mut self, p: &Pat) {
            let mut v = vec![];
            pat_idents(p, &mut v);
            self.scopes.last_mut().unwrap().extend(v);
        }
        fn scoped(&mut self, f: impl FnOnce(&mut V)) {
            self.scopes.push(vec![]);
            f(self);
            self.scopes.pop();
        }
    }
    impl<'a> Visit<'a> for V {
        fn visit_expr_path(&mut self, p: &'a syn::ExprPath) {
            if p.qself.is_none() {
                if let Some(id) = p.path.get_ident() {
                    let n = id.to_string();
                    if !self.bound(&n) && !self.out.contains(&n) {
                        self.out.push(n);
                    }
                }
            }
        }
        fn visit_block(&mut self, b: &'a syn::Block) {
            self.scoped(|v| syn::visit::visit_block(v, b));
        }
        fn visit_local(&mut self, l: &'a syn::Local) {
            if let Some(init) = &l.init {
                self.visit_expr(&init.expr);
                if let Some((_, e)) = &init.diverge {
                    self.visit_expr(e);
                }
            }
            self.bind(&l.pat);
        }
        fn visit_expr_closure(&mut self, c: &'a ExprClosure) {
            self.scoped(|v| {
                for p in &c.inputs {
                    v.bind(p);
                }
                v.visit_expr(&c.body);
            });
        }
        fn visit_expr_for_loop(&mut self, f: &'a syn::ExprForLoop) {
            self.visit_expr(&f.expr);
            self.scoped(|v| {
                v.bind(&f.pat);
                v.visit_block(&f.body);
            });
        }
        fn visit_arm(&mut self, a: &'a syn::Arm) {
            self.scoped(|v| {
                v.bind(&a.pat);
                if let Some((_, g)) = &a.guard {
                    v.visit_expr(g);
                }
                v.visit_expr(&a.body);
            });
        }
        fn visit_expr_let(&mut self, l: &'a syn::ExprLet) {
            self.visit_expr(&l.expr);
            self.bind(&l.pat);
        }
        fn visit_expr_if(&mut self, i: &'a syn::ExprIf) {
            self.scoped(|v| {
                v.visit_expr(&i.cond);
                v.visit_block(&i.then_branch);
            });
            if let Some((_, e)) = &i.else_branch {
                self.visit_expr(e);
            }
        }
        fn visit_expr_while(&mut self, w: &'a syn::ExprWhile) {
            self.scoped(|v| {
                v.visit_expr(&w.cond);
                v.visit_block(&w.body);
            });
        }
        fn visit_macro(&mut self, m: &'a syn::Macro) {
            // panic! / assert! などの引数は式の列として見る
            use syn::punctuated::Punctuated;
            if let Ok(es) = m.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)
            {
                for e in &es {
                    self.visit_expr(e);
                }
            }
        }
        fn visit_item(&mut self, _: &'a syn::Item) {}
    }
    let mut v = V {
        scopes: vec![vec![]],
        out: vec![],
    };
    v.visit_expr_closure(c);
    v.out
}

/// キャプチャ = 自由変数のうち、親の関数で束縛されている名前
pub fn captures(c: &ExprClosure, bound: &HashSet<String>) -> Vec<String> {
    free_vars(c)
        .into_iter()
        .filter(|n| bound.contains(n))
        .collect()
}

/// 引数の (パターン, 型注釈)
pub fn param(p: &Pat) -> (&Pat, Option<&syn::Type>) {
    match p {
        Pat::Type(pt) => (&pt.pat, Some(&pt.ty)),
        p => (p, None),
    }
}

/// クロージャ本体を普通の fn にする: `fn call(__env: (), a: (), __p1: ()) { let (x, y) = __p1; 本体 }`。
/// 引数の型はシグネチャ (FnSig) 側で決まるのでここでは `()` を置くだけ
pub fn synth(c: &ExprClosure) -> syn::ItemFn {
    let mut f: syn::ItemFn = syn::parse_quote!(
        fn call(__env: ()) {}
    );
    let mut stmts: Vec<syn::Stmt> = vec![];
    for (i, p) in c.inputs.iter().enumerate() {
        let (pat, _) = param(p);
        let simple = matches!(pat, Pat::Ident(pi) if pi.subpat.is_none() && pi.by_ref.is_none());
        let pat_in: Pat = if simple {
            pat.clone()
        } else {
            let id = syn::Ident::new(&format!("__p{i}"), proc_macro2::Span::call_site());
            stmts.push(syn::Stmt::Local(syn::Local {
                attrs: vec![],
                let_token: Default::default(),
                pat: pat.clone(),
                init: Some(syn::LocalInit {
                    eq_token: Default::default(),
                    expr: Box::new(syn::parse_quote!(#id)),
                    diverge: None,
                }),
                semi_token: Default::default(),
            }));
            syn::parse_quote!(#id)
        };
        f.sig.inputs.push(syn::FnArg::Typed(syn::PatType {
            attrs: vec![],
            pat: Box::new(pat_in),
            colon_token: Default::default(),
            ty: Box::new(syn::parse_quote!(())),
        }));
    }
    match &*c.body {
        Expr::Block(b) if b.label.is_none() && b.attrs.is_empty() => {
            stmts.extend(b.block.stmts.iter().cloned());
            f.block.brace_token = b.block.brace_token;
        }
        e => stmts.push(syn::Stmt::Expr(e.clone(), None)),
    }
    f.block.stmts = stmts;
    f
}

/// クロージャの本体がキャプチャした変数を値として使うか (`|| String::from_utf8(out)` / `|| out`):
/// 関数の引数・本体の値・`let y = x;` の右辺・`return x`。rustc はこのときキャプチャを値で持つ
pub fn moves_capture(c: &ExprClosure, caps: &[String]) -> bool {
    use syn::visit::Visit;
    struct V<'a> {
        caps: &'a [String],
        found: bool,
    }
    impl V<'_> {
        fn is_cap(&self, e: &Expr) -> bool {
            matches!(e, Expr::Path(p) if p.path.get_ident().is_some_and(|i| self.caps.iter().any(|c| i == c)))
        }
    }
    impl<'ast> Visit<'ast> for V<'_> {
        fn visit_expr_call(&mut self, e: &'ast syn::ExprCall) {
            if e.args.iter().any(|a| self.is_cap(a)) {
                self.found = true;
            }
            syn::visit::visit_expr_call(self, e);
        }
        fn visit_local(&mut self, l: &'ast syn::Local) {
            if let Some(init) = &l.init {
                if self.is_cap(&init.expr) {
                    self.found = true;
                }
            }
            syn::visit::visit_local(self, l);
        }
        fn visit_expr_return(&mut self, r: &'ast syn::ExprReturn) {
            if r.expr.as_deref().is_some_and(|e| self.is_cap(e)) {
                self.found = true;
            }
            syn::visit::visit_expr_return(self, r);
        }
        fn visit_expr_closure(&mut self, _: &'ast ExprClosure) {}
    }
    let mut v = V { caps, found: false };
    let body = match &*c.body {
        Expr::Block(b) => b.block.stmts.last().and_then(|s| match s {
            syn::Stmt::Expr(e, None) => Some(e),
            _ => None,
        }),
        e => Some(e),
    };
    if body.is_some_and(|e| v.is_cap(e)) {
        return true;
    }
    v.visit_expr(&c.body);
    v.found
}
