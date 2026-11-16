//! アイテムの要約 (front.rs の `summarize` / `parse_sig` / `lift_closures` / `parse_and_summarize`) を
//! 自前の構文木 (src/parse.rs) の上で行う。tests/ast_summary.rs が全ファイルで syn 版と同じ要約になることを確かめる。
//! 外部クレートに依存しない (parse / types / ast_types / ast_closure / lexer だけ) ので crustc 自身でもコンパイルできる。

use crate::ast_closure;
use crate::ast_types::tysyn;
use crate::lexer::{ItemKind as LexKind, SrcItem};
use crate::parse::{Block, Bound, Expr, Fields, FnSig as PSig, GParam, Generics, Item, ItemKind, Pat, Path, Stmt, Ty as PTy, parse_file};
use crate::types::{
    AssocSyn, CONST_GENERIC, ClosureSyn, IMPL_RET, OPAQUE_RET, Owner, SigSyn, Summary, Ty, TySyn, VariantKind, VariantSyn,
};

pub type R<T> = Result<T, String>;

fn err<T>(msg: &str) -> R<T> {
    Err(msg.to_string())
}

/// 空白を除いた属性のテキスト (`#[derive(Debug,Clone)]`)
fn compact(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// `#[name]` か `#[unsafe(name)]` が付いているか
fn has_attr(attrs: &[String], name: &str) -> bool {
    attrs.iter().any(|a| {
        let c = compact(a);
        c == format!("#[{name}]") || c.starts_with(&format!("#[{name}(")) || c.starts_with(&format!("#[{name}=")) || c == format!("#[unsafe({name})]")
    })
}

/// `#[derive(A, B)]` の名前 (1 つの識別子のものだけ)
fn derive_names(attrs: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for a in attrs {
        let c = compact(a);
        let Some(inner) = c.strip_prefix("#[derive(").and_then(|x| x.strip_suffix(")]")) else { continue };
        for part in inner.split(',') {
            if !part.is_empty() && part.chars().all(|ch| ch == '_' || ch.is_alphanumeric()) {
                out.push(part.to_string());
            }
        }
    }
    out
}

/// `#[link_name = "malloc"]` の名前
fn link_name(attrs: &[String]) -> Option<String> {
    for a in attrs {
        let c = compact(a);
        if let Some(v) = c.strip_prefix("#[link_name=\"").and_then(|x| x.strip_suffix("\"]")) {
            return Some(v.to_string());
        }
    }
    None
}

fn self_of(name: &str, generics: &[String]) -> TySyn {
    if generics.is_empty() {
        TySyn::Named(name.to_string())
    } else {
        TySyn::App(name.to_string(), generics.iter().map(|g| TySyn::Named(g.clone())).collect())
    }
}

fn const_int(e: &Expr) -> R<i64> {
    match e {
        Expr::Lit(t) if t.starts_with(|c: char| c.is_ascii_digit()) => {
            let digits: String = t.chars().filter(|&c| c != '_').take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<i64>().map_err(|e| e.to_string())
        }
        Expr::Unary(op, x, _) if op == "-" => Ok(-const_int(x)?),
        Expr::Paren(x) => const_int(x),
        _ => err("only integer literals are supported as discriminants"),
    }
}

/// 境界の名前 (front.rs の bound_names と同じ規則)
pub fn bound_names(bs: &[Bound]) -> R<Vec<String>> {
    let mut v = Vec::new();
    for b in bs {
        match b {
            // `?Sized` は制約を緩めるだけなので数えない
            Bound::Trait(true, _) => {}
            Bound::Trait(false, p) => {
                let last = p.segs.last().ok_or("empty bound")?;
                if last.fn_args.is_some() || (p.segs.len() == 1 && !last.args.is_empty()) {
                    v.push(last.text.clone());
                } else if last.args.is_empty() {
                    v.push(last.name.clone());
                } else {
                    return err("unsupported trait bound");
                }
            }
            Bound::Lifetime(_) => {}
        }
    }
    Ok(v)
}

fn collect_generics(g: &Generics, out: &mut Vec<(String, Vec<String>)>) -> R<()> {
    for p in &g.params {
        match p {
            GParam::Lifetime(_) => {}
            GParam::Ty(n, bs, _) => out.push((n.clone(), bound_names(bs)?)),
            GParam::Const(n, _) => out.push((n.clone(), vec![CONST_GENERIC.to_string()])),
        }
    }
    for (t, bs) in &g.preds {
        // (型パラメータ以外への境界は読み飛ばす)
        let PTy::Path(None, p) = t else { continue };
        if p.global || p.segs.len() != 1 || !p.segs[0].args.is_empty() || p.segs[0].fn_args.is_some() {
            continue;
        }
        // (ライフタイムの述語 `'a: 'b`)
        if p.segs[0].name.starts_with('\'') {
            continue;
        }
        let id = p.segs[0].name.clone();
        let bs = bound_names(bs)?;
        match out.iter_mut().find(|g| g.0 == id) {
            Some(g) => g.1.extend(bs),
            None if id == "Self" => {}
            None => return err("where clause on an unknown type parameter"),
        }
    }
    Ok(())
}

fn adt_params(g: &Generics) -> R<Vec<String>> {
    let mut v = Vec::new();
    collect_generics(g, &mut v)?;
    Ok(v.into_iter().map(|x| x.0).collect())
}

fn only_lifetimes(g: &Generics) -> R<()> {
    if g.params.iter().any(|p| !matches!(p, GParam::Lifetime(_))) {
        return err("generics are not supported yet (lifetimes only)");
    }
    Ok(())
}

/// 型の空白なしのテキスト (syn のトークン列から空白を除いたものと同じ: `PartialEq<[U;N]>` の比較用)
fn ty_compact(t: &PTy) -> String {
    match t {
        PTy::Path(None, p) => path_compact(p),
        PTy::Path(Some(q), p) => format!("<{}as{}>", ty_compact(q), path_compact(p)),
        PTy::Ref(l, m, i) => format!("&{}{}{}", l.clone().unwrap_or_default(), if *m { "mut" } else { "" }, ty_compact(i)),
        PTy::Ptr(m, i) => format!("*{}{}", if *m { "mut" } else { "const" }, ty_compact(i)),
        PTy::Slice(i) => format!("[{}]", ty_compact(i)),
        PTy::Array(i, n) => format!("[{};{}]", ty_compact(i), expr_compact(n)),
        PTy::Tuple(v) => {
            let v: Vec<String> = v.iter().map(ty_compact).collect();
            if v.len() == 1 { format!("({},)", v[0]) } else { format!("({})", v.join(",")) }
        }
        PTy::Never => "!".to_string(),
        PTy::Infer => "_".to_string(),
        _ => String::new(),
    }
}

fn path_compact(p: &Path) -> String {
    let mut s = String::new();
    if p.global {
        s.push_str("::");
    }
    for (k, seg) in p.segs.iter().enumerate() {
        if k > 0 {
            s.push_str("::");
        }
        s.push_str(&compact(&seg.text));
    }
    s
}

fn expr_compact(e: &Expr) -> String {
    match e {
        Expr::Lit(t) => t.clone(),
        Expr::Path(None, p) => path_compact(p),
        _ => String::new(),
    }
}

/// 関数のシグネチャ (front.rs の parse_sig)
fn parse_sig(sig: &PSig, is_extern: bool, is_main: bool, owner: Option<&Owner>) -> R<SigSyn> {
    let mut generics = owner.map(|o| o.generics.clone()).unwrap_or_default();
    collect_generics(&sig.generics, &mut generics)?;
    if is_extern && !generics.is_empty() {
        return err("extern fns cannot be generic");
    }
    if is_main && !generics.is_empty() {
        return err("`main` cannot be generic");
    }
    let self_ty = owner.map(|o| &o.self_ty);
    let mut params = Vec::new();
    let mut has_self = false;
    if let Some(rt) = &sig.recv_ty {
        if owner.is_none() {
            return err("`self` is only allowed in impls");
        }
        has_self = true;
        params.push(tysyn(rt, self_ty)?);
    }
    for (_, t) in &sig.params {
        // 引数位置の `impl Trait` は名前のない型パラメータ
        let (inner, rf) = match t {
            PTy::Ref(_, m, i) => (&**i, Some(*m)),
            t => (t, None),
        };
        if let PTy::Impl(bs) = inner {
            let g = format!("__impl{}", generics.len());
            generics.push((g.clone(), bound_names(bs)?));
            let n = TySyn::Named(g);
            params.push(match rf {
                Some(m) => TySyn::Ref(m, Box::new(n)),
                None => n,
            });
        } else {
            params.push(tysyn(t, self_ty)?);
        }
    }
    let ret = match &sig.ret {
        None => TySyn::Prim(Ty::Unit),
        Some(PTy::Impl(_)) => TySyn::Named(IMPL_RET.into()),
        Some(t) => tysyn(t, self_ty)?,
    };
    let name = sig.name.clone();
    let symbol = match owner {
        _ if is_extern || is_main => name.clone(),
        Some(o) => match o.impl_trait.as_deref() {
            Some(t) if t == "Display" || t == "Debug" => format!("_x{}.{t}.{name}", o.base),
            Some(t) if t.starts_with("PartialEq<") || t.starts_with("PartialOrd<") => {
                let t: String = t.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
                format!("_x{}.{t}.{name}", o.base)
            }
            _ => format!("_x{}.{name}", o.base),
        },
        None => format!("_x{name}"),
    };
    let is_trait = owner.is_some_and(|o| o.is_trait);
    Ok(SigSyn {
        params: params,
        ret: ret,
        symbol: symbol,
        is_extern: is_extern,
        is_main: is_main,
        impl_of: owner.filter(|o| !o.is_trait).map(|o| o.base.clone()),
        has_self: has_self,
        generics: generics,
        trait_of: owner.filter(|o| o.is_trait).map(|o| o.base.clone()),
        has_body: true,
        impl_trait: owner.and_then(|o| o.impl_trait.clone()),
        self_ty: owner.filter(|_| !is_trait).map(|o| o.self_ty.clone()),
        export: false,
        panic_handler: false,
    })
}

/// `Fn(A, B) -> R` 形式の境界なら (種類, 引数, 戻り値)
pub fn parse_fn_bound(s: &str) -> Option<(String, Vec<TySyn>, TySyn)> {
    let kind = ["FnOnce", "FnMut", "Fn"].into_iter().find(|k| s.starts_with(k) && s[k.len()..].trim_start().starts_with('('))?;
    let f = parse_file(&format!("type __B = dyn {s};")).ok()?;
    let ItemKind::Type(_, _, _, Some(PTy::Dyn(bs))) = &f.items.first()?.kind else { return None };
    let Some(Bound::Trait(_, p)) = bs.first() else { return None };
    let (ps, r) = p.segs.last()?.fn_args.clone()?;
    let mut args = Vec::new();
    for t in &ps {
        args.push(tysyn(t, None).ok()?);
    }
    let ret = match r {
        None => TySyn::Prim(Ty::Unit),
        Some(t) => tysyn(&t, None).ok()?,
    };
    Some((kind.to_string(), args, ret))
}

/// 境界 `Iterator<Item = T>` の関連型の束縛 (名前, 型)
pub fn assoc_bindings(s: &str) -> Vec<(String, TySyn)> {
    let mut out = Vec::new();
    let Ok(f) = parse_file(&format!("type __B = dyn {s};")) else { return out };
    let Some(it) = f.items.first() else { return out };
    let ItemKind::Type(_, _, _, Some(PTy::Dyn(bs))) = &it.kind else { return out };
    let Some(Bound::Trait(_, p)) = bs.first() else { return out };
    let Some(last) = p.segs.last() else { return out };
    for a in &last.args {
        if let crate::parse::GArg::Bind(n, t) = a {
            if let Ok(ts) = tysyn(t, None) {
                out.push((n.clone(), ts));
            }
        }
    }
    out
}

// ---------------------------------------------------------------- クロージャの持ち上げ

/// パターンの中のすべてのパターン (束縛の名前を集める)
fn pats_in_block(b: &Block, out: &mut Vec<String>) {
    for s in &b.stmts {
        match s {
            Stmt::Let(p, _, init, els) => {
                ast_closure::pat_idents(p, out);
                if let Some(e) = init {
                    pats_in_expr(e, out);
                }
                if let Some(b) = els {
                    pats_in_block(b, out);
                }
            }
            Stmt::Expr(e, _) => pats_in_expr(e, out),
            Stmt::Item(_) => {}
        }
    }
}

fn pats_in_expr(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Let(p, x) => {
            ast_closure::pat_idents(p, out);
            pats_in_expr(x, out);
        }
        Expr::Match(x, arms) => {
            pats_in_expr(x, out);
            for a in arms {
                ast_closure::pat_idents(&a.pat, out);
                if let Some(g) = &a.guard {
                    pats_in_expr(g, out);
                }
                pats_in_expr(&a.body, out);
            }
        }
        Expr::For(_, p, x, b) => {
            ast_closure::pat_idents(p, out);
            pats_in_expr(x, out);
            pats_in_block(b, out);
        }
        Expr::Closure(_, ps, _, body) => {
            for (p, _) in ps {
                ast_closure::pat_idents(p, out);
            }
            pats_in_expr(body, out);
        }
        Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => pats_in_block(b, out),
        Expr::If(c, t, els) => {
            pats_in_expr(c, out);
            pats_in_block(t, out);
            if let Some(x) = els {
                pats_in_expr(x, out);
            }
        }
        Expr::While(_, c, b) => {
            pats_in_expr(c, out);
            pats_in_block(b, out);
        }
        Expr::Macro(..) => {}
        _ => {
            let mut kids: Vec<Expr> = Vec::new();
            ast_closure::each_child_pub(e, &mut |c| kids.push(c.clone()));
            for k in &kids {
                pats_in_expr(k, out);
            }
        }
    }
}

/// 関数 (入れ子のクロージャを含む) のどこかで束縛される名前
fn bound_in_fn(sig: &PSig, body: &Block) -> Vec<String> {
    let mut v = Vec::new();
    if sig.recv_ty.is_some() {
        v.push("self".to_string());
    }
    for (p, _) in &sig.params {
        ast_closure::pat_idents(p, &mut v);
    }
    pats_in_block(body, &mut v);
    v
}

/// 本体の `let n = ..` から、返すクロージャがキャプチャした変数の型 (注釈か初期化式のリテラル)
fn local_ty_syn(b: &Block, n: &str, int_hint: Option<Ty>) -> Option<TySyn> {
    let mut found = None;
    for st in &b.stmts {
        let Stmt::Let(pat, ann, init, _) = st else { continue };
        let Pat::Ident(_, _, name, None) = pat else { continue };
        if name != n {
            continue;
        }
        if let Some(t) = ann {
            found = tysyn(t, None).ok();
            continue;
        }
        found = match init {
            Some(Expr::Lit(t)) => lit_ty(t, int_hint),
            Some(Expr::Call(f, _)) => match &**f {
                Expr::Path(None, p) if p.segs.len() == 2 && p.segs[0].name == "String" => Some(TySyn::Named("String".into())),
                _ => None,
            },
            Some(Expr::MethodCall(_, m, _, _, _)) if m == "to_string" => Some(TySyn::Named("String".into())),
            _ => None,
        };
    }
    found
}

/// リテラルのテキストから型 (整数は接尾辞 → 文脈 → i32)
fn lit_ty(t: &str, int_hint: Option<Ty>) -> Option<TySyn> {
    if t == "true" || t == "false" {
        return Some(TySyn::Prim(Ty::Bool));
    }
    if t.starts_with('\'') {
        return Some(TySyn::Prim(Ty::Char));
    }
    if t.starts_with('"') || t.starts_with("r\"") || t.starts_with("r#") {
        return Some(TySyn::Ref(false, Box::new(TySyn::Str)));
    }
    if !t.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let hex = t.starts_with("0x") || t.starts_with("0b") || t.starts_with("0o");
    // 接尾辞: 数字・`_`・(16 進なら a-f) の後ろの英字
    let body = if hex { &t[2..] } else { t };
    let suf_at = body
        .char_indices()
        .find(|&(_, c)| !(c.is_ascii_digit() || c == '_' || c == '.' || (hex && c.is_ascii_hexdigit()) || (!hex && (c == 'e' || c == 'E'))))
        .map(|(i, _)| i)
        .unwrap_or(body.len());
    let suffix = &body[suf_at..];
    let is_float = !hex && (body[..suf_at].contains('.') || body[..suf_at].contains('e') || body[..suf_at].contains('E') || suffix == "f32" || suffix == "f64");
    if is_float {
        return Some(TySyn::Prim(if suffix == "f32" { Ty::F32 } else { Ty::F64 }));
    }
    Some(TySyn::Prim(Ty::from_suffix(suffix).or(int_hint).unwrap_or(Ty::I32)))
}

/// 引数の名前 (識別子のパターンなら)
fn pat_name(p: &Pat) -> Option<&str> {
    match p {
        Pat::Ident(_, _, n, None) => Some(n),
        _ => None,
    }
}

fn lift_closures(sig_ast: &PSig, body: &Block, fname: &str, sig: &mut SigSyn, self_ty: Option<&TySyn>, sm: &mut Summary) -> R<()> {
    let cs = ast_closure::closures_of(body);
    if cs.is_empty() {
        if sig.ret == TySyn::Named(IMPL_RET.into()) {
            sig.ret = TySyn::Named(OPAQUE_RET.into());
        }
        return Ok(());
    }
    let bound = bound_in_fn(sig_ast, body);
    let mut out: Vec<ClosureSyn> = Vec::new();
    for (k, c) in cs.iter().enumerate() {
        let Expr::Closure(mv, cps, cret, _) = c else { continue };
        let name = format!("{fname}::{{closure#{k}}}");
        let mut generics: Vec<String> = sig.generics.iter().map(|g| g.0.clone()).collect();
        let n_parent = generics.len() as u64;
        let caps: Vec<String> = ast_closure::free_vars(c).into_iter().filter(|n| bound.contains(n)).collect();
        let mut fields = Vec::new();
        for (i, n) in caps.iter().enumerate() {
            let g = format!("__C{i}");
            fields.push((n.clone(), TySyn::Named(g.clone())));
            generics.push(g);
        }
        let mut params = Vec::new();
        for (i, (_, t)) in cps.iter().enumerate() {
            match t {
                Some(t) => params.push(tysyn(t, self_ty)?),
                None => {
                    let g = format!("__P{i}");
                    params.push(TySyn::Named(g.clone()));
                    generics.push(g);
                }
            }
        }
        generics.push("__R".into());
        let ret_ann = match cret {
            None => None,
            Some(t) => Some(tysyn(t, self_ty)?),
        };
        let env = TySyn::App(name.clone(), generics.iter().map(|g| TySyn::Named(g.clone())).collect());
        params.insert(0, TySyn::Ptr(true, Box::new(env)));
        let mut gs: Vec<(String, Vec<String>)> = sig.generics.clone();
        for g in &generics[n_parent as usize..] {
            gs.push((g.clone(), Vec::new()));
        }
        let csig = SigSyn {
            params: params,
            ret: TySyn::Named("__R".into()),
            symbol: format!("{}.c{k}", sig.symbol),
            impl_of: sig.impl_of.clone(),
            generics: gs,
            has_body: true,
            self_ty: sig.self_ty.clone(),
            ..Default::default()
        };
        let assigns = ast_closure::assigns_capture(c, &caps);
        let mut moved: Vec<bool> = Vec::new();
        for n in &caps {
            moved.push(*mv || (ast_closure::moves_capture(c, &[n.clone()]) && !assigns));
        }
        let by_move = *mv || (!moved.is_empty() && moved.iter().all(|&m| m));
        out.push(ClosureSyn {
            name: name,
            generics: generics,
            fields: fields,
            sig: csig,
            by_move: by_move,
            moved: moved,
            ret_ann: ret_ann,
            n_parent: n_parent,
        });
    }
    // `-> impl Fn(A) -> B { move |x| .. }`: 型は末尾のクロージャの struct
    if sig.ret == TySyn::Named(IMPL_RET.into()) {
        let mut tail = match body.stmts.last() {
            Some(Stmt::Expr(e, false)) => Some(e),
            _ => None,
        };
        while let Some(Expr::Paren(p)) = tail {
            tail = Some(&**p);
        }
        let Some(tc @ Expr::Closure(_, tps, _, _)) = tail else {
            sig.ret = TySyn::Named(OPAQUE_RET.into());
            sm.closures = out;
            return Ok(());
        };
        let key = format!("{tc:?}");
        let k = cs.iter().position(|c| format!("{c:?}") == key).ok_or("internal: tail closure")?;
        let Some(PTy::Impl(ibs)) = &sig_ast.ret else { return err("internal: impl return") };
        let bs = bound_names(ibs)?;
        let Some((_, bargs, bret)) = bs.iter().find_map(|b| parse_fn_bound(b)) else {
            return err("`impl Trait` return types must be `impl Fn(..) -> ..`");
        };
        let cl = &out[k];
        let mut args: Vec<TySyn> = sig.generics.iter().map(|g| TySyn::Named(g.0.clone())).collect();
        let skip = sig.has_self as usize;
        for (n, _) in &cl.fields {
            if cl.by_move {
                let ty = sig_ast.params.iter().zip(sig.params.iter().skip(skip)).find_map(|((p, _), t)| {
                    if pat_name(p) == Some(n.as_str()) { Some(t.clone()) } else { None }
                });
                let int_hint = match &bret {
                    TySyn::Prim(t) if t.is_int() => Some(*t),
                    _ => None,
                };
                match ty.or_else(|| local_ty_syn(body, n, int_hint)) {
                    Some(t) => args.push(t),
                    None => {
                        return Err(format!(
                            "a returned closure can only capture parameters and locals with an obvious type (`{n}`; annotate it: `let {n}: T = ..`)"
                        ));
                    }
                }
            } else {
                return err("a returned closure that captures must be `move`");
            }
        }
        for (i, (_, t)) in tps.iter().enumerate() {
            if t.is_none() {
                match bargs.get(i) {
                    Some(t) => args.push(t.clone()),
                    None => return err("closure has more parameters than the bound"),
                }
            }
        }
        args.push(cl.ret_ann.clone().unwrap_or(bret));
        sig.ret = TySyn::App(cl.name.clone(), args);
    }
    sm.closures = out;
    Ok(())
}

// ---------------------------------------------------------------- 要約

/// 1 つのアイテムの要約 (front.rs の summarize)
pub fn summarize(item: &Item, owner: Option<&Owner>) -> R<Summary> {
    let mut sm = Summary::default();
    match &item.kind {
        ItemKind::Type(n, _, _, Some(t)) if owner.is_some() => {
            let ow = owner.unwrap();
            sm.assocs.push(AssocSyn {
                name: n.clone(),
                self_ty: ow.self_ty.clone(),
                generics: ow.generics.iter().map(|g| g.0.clone()).collect(),
                value: tysyn(t, Some(&ow.self_ty))?,
            });
        }
        ItemKind::Fn(fs, Some(body)) if owner.is_some() => {
            let ow = owner.unwrap();
            let mut sig = parse_sig(fs, false, false, Some(ow))?;
            let name = match ow.impl_trait.as_deref() {
                Some(t) if t == "Display" || t == "Debug" => format!("{}::<{t}>::{}", ow.base, fs.name),
                Some(t) if t.starts_with("PartialEq<") || t.starts_with("PartialOrd<") => format!("{}::<{t}>::{}", ow.base, fs.name),
                _ if matches!(ow.self_ty, TySyn::Ref(..)) => format!("{}::__ref_{}", ow.base, fs.name),
                _ => format!("{}::{}", ow.base, fs.name),
            };
            lift_closures(fs, body, &name, &mut sig, Some(&ow.self_ty), &mut sm)?;
            sm.fns.push((name, sig));
        }
        // 型の別名 (`type Key = u128;`)
        ItemKind::Type(n, g, _, Some(t)) => {
            let mut gs = Vec::new();
            collect_generics(g, &mut gs)?;
            sm.aliases.push((n.clone(), gs.into_iter().map(|x| x.0).collect(), tysyn(t, None)?));
        }
        ItemKind::Trait(n, g, _, _) => {
            only_lifetimes(g)?;
            sm.trait_decl = Some(n.clone());
        }
        ItemKind::Impl(g, tr, self_ty, _) => {
            if let Some((neg, tp)) = tr {
                if *neg {
                    return err("negative impls are not supported");
                }
                let last = tp.segs.last().ok_or("unsupported trait path")?;
                sm.impl_trait = Some(last.name.clone());
                // `impl PartialEq<[U; N]> for Vec<T>`: 右辺の型が self 型と違う比較は trait 名に型引数を残す
                if (last.name == "PartialEq" || last.name == "PartialOrd") && !last.args.is_empty() {
                    let rhs = compact(&last.text[last.name.len()..]);
                    let selfs = ty_compact(self_ty);
                    if rhs != format!("<{selfs}>") && rhs != "<Self>" {
                        sm.impl_trait = Some(format!("{}{rhs}", last.name));
                    }
                }
            }
            collect_generics(g, &mut sm.impl_generics)?;
            // `impl Add for &M`: M のメソッド `__ref_add` (self は &M) として持つ
            let self_path = match self_ty {
                PTy::Ref(_, _, i) if tr.is_some() => &**i,
                t => t,
            };
            if let (PTy::Tuple(v), true) = (self_path, tr.is_some()) {
                sm.impl_self = Some(format!("#tuple{}", v.len()));
                sm.impl_self_ty = Some(tysyn(self_ty, None)?);
                return Ok(sm);
            }
            if matches!(self_path, PTy::Array(..) | PTy::Slice(_)) && tr.is_some() {
                sm.impl_self = Some(if matches!(self_path, PTy::Array(..)) { "__array" } else { "__slice" }.to_string());
                sm.impl_self_ty = Some(tysyn(self_ty, None)?);
                return Ok(sm);
            }
            let PTy::Path(None, tp) = self_path else {
                return err("unsupported impl self type");
            };
            if tp.segs.len() != 1 {
                return err("unsupported impl self type");
            }
            sm.impl_self = Some(tp.segs[0].name.clone());
            sm.impl_self_ty = Some(tysyn(self_ty, None)?);
        }
        ItemKind::Fn(fs, Some(body)) => {
            let name = fs.name.clone();
            let export = has_attr(&item.attrs, "no_mangle");
            let is_main = name == "main" && !export;
            let mut sig = parse_sig(fs, false, is_main, None)?;
            let result_ret = matches!(&sig.ret, TySyn::App(n, _) if n == "Result");
            if is_main && (!sig.params.is_empty() || (sig.ret != TySyn::Prim(Ty::Unit) && !result_ret)) {
                return err("`main` must be `fn main()`");
            }
            if export {
                sig.export = true;
                sig.symbol = name.clone();
            }
            if has_attr(&item.attrs, "panic_handler") {
                sig.panic_handler = true;
                if sig.ret != TySyn::Prim(Ty::Never) {
                    return err("`#[panic_handler]` function must return `!`");
                }
            }
            lift_closures(fs, body, &name, &mut sig, None, &mut sm)?;
            sm.fns.push((name, sig));
        }
        ItemKind::Use(_) => {}
        ItemKind::Const(n, Some(t), Some(_)) => {
            let name = match owner {
                Some(ow) => format!("{}::{}", ow.base, n),
                None => n.clone(),
            };
            sm.consts.push((name, tysyn(t, owner.map(|o| &o.self_ty))?, item.init_text.clone()));
        }
        ItemKind::Static(m, n, t, _) => {
            sm.statics.push((n.clone(), tysyn(t, None)?, item.init_text.clone(), *m));
        }
        ItemKind::Extern(items) => {
            for fi in items {
                let ItemKind::Fn(fs, None) = &fi.kind else {
                    return err("only fns are supported in extern blocks");
                };
                let mut sig = parse_sig(fs, true, false, None)?;
                if let Some(l) = link_name(&fi.attrs) {
                    sig.symbol = l;
                }
                sm.fns.push((fs.name.clone(), sig));
            }
        }
        ItemKind::Enum(sn, g, vars) => {
            sm.adt_generics = adt_params(g)?;
            let selft = self_of(sn, &sm.adt_generics);
            let mut vs = Vec::new();
            for (vn, fields, d) in vars {
                let (kind, fs) = match fields {
                    Fields::Unit => (VariantKind::Unit, Vec::new()),
                    Fields::Tuple(v) => {
                        let mut fs = Vec::new();
                        for (i, t) in v.iter().enumerate() {
                            fs.push((i.to_string(), tysyn(t, Some(&selft))?));
                        }
                        (VariantKind::Tuple, fs)
                    }
                    Fields::Named(v) => {
                        let mut fs = Vec::new();
                        for (n, t) in v {
                            fs.push((n.clone(), tysyn(t, Some(&selft))?));
                        }
                        (VariantKind::Struct, fs)
                    }
                };
                let discr = match d {
                    None => None,
                    Some(e) => Some(const_int(e)?),
                };
                vs.push(VariantSyn { name: vn.clone(), kind: kind, fields: fs, discr: discr });
            }
            let d = derive_names(&item.attrs);
            if !d.is_empty() {
                sm.derives.push((sn.clone(), d));
            }
            sm.enums.push((sn.clone(), vs));
        }
        ItemKind::Struct(sn, g, fields) => {
            sm.adt_generics = adt_params(g)?;
            let selft = self_of(sn, &sm.adt_generics);
            let mut fs = Vec::new();
            match fields {
                Fields::Named(v) => {
                    for (n, t) in v {
                        fs.push((n.clone(), tysyn(t, Some(&selft))?));
                    }
                }
                Fields::Tuple(v) => {
                    for (i, t) in v.iter().enumerate() {
                        fs.push((i.to_string(), tysyn(t, Some(&selft))?));
                    }
                }
                Fields::Unit => {}
            }
            let d = derive_names(&item.attrs);
            if !d.is_empty() {
                sm.derives.push((sn.clone(), d));
            }
            sm.structs.push((sn.clone(), fs));
        }
        _ => return err("unsupported item"),
    }
    Ok(sm)
}

/// 字句解析のアイテム (lexer) 1 つを構文解析して要約する (front.rs の parse_and_summarize)
pub fn parse_and_summarize(src: &str, it: &SrcItem, owner: Option<&Owner>) -> R<Summary> {
    let text = &src[it.start..it.end];
    if let LexKind::InnerAttr = it.kind {
        return Ok(Summary::default());
    }
    let text = match it.kind {
        LexKind::ImplHeader | LexKind::TraitHeader => format!("{text}}}"),
        _ => text.to_string(),
    };
    let f = parse_file(&text)?;
    let Some(item) = f.items.first() else { return Ok(Summary::default()) };
    if let LexKind::TraitMember(_) = it.kind {
        let ow = owner.ok_or("trait member without trait")?;
        match &item.kind {
            // `type Item;` (関連型の宣言) は何も持たない
            ItemKind::Type(..) => return Ok(Summary::default()),
            // `const K: i32 = 3;` (既定値があれば `Trait::K` という const)
            ItemKind::Const(n, t, e) => {
                let mut sm = Summary::default();
                if let (Some(t), Some(_)) = (t, e) {
                    sm.consts.push((format!("{}::{}", ow.base, n), tysyn(t, None)?, item.init_text.clone()));
                }
                return Ok(sm);
            }
            ItemKind::Fn(fs, body) => {
                let mut sig = parse_sig(fs, false, false, Some(ow))?;
                sig.has_body = body.is_some();
                let mut sm = Summary::default();
                sm.fns.push((format!("{}::{}", ow.base, fs.name), sig));
                return Ok(sm);
            }
            _ => return err("unsupported trait member"),
        }
    }
    summarize(item, owner)
}

/// impl / trait のヘッダの要約からメンバの所有者を作る (driver.rs と同じ)
pub fn owner_of(it: &SrcItem, sums: &[Summary]) -> Option<Owner> {
    match it.kind {
        LexKind::ImplMember(h) => {
            let hs = &sums[h as usize];
            hs.impl_self.clone().map(|n| Owner {
                self_ty: hs.impl_self_ty.clone().unwrap_or(TySyn::Named(n.clone())),
                base: n,
                impl_trait: hs.impl_trait.clone(),
                generics: hs.impl_generics.clone(),
                is_trait: false,
            })
        }
        LexKind::TraitMember(h) => sums[h as usize].trait_decl.clone().map(|n| Owner {
            self_ty: TySyn::Named("Self".into()),
            generics: vec![("Self".into(), vec![n.clone()])],
            base: n,
            impl_trait: None,
            is_trait: true,
        }),
        _ => None,
    }
}
