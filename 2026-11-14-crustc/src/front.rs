//! フロントエンド: アイテム要約 (シグネチャ・struct 定義) から型表と参照グラフを作る。
//!
//! ここは「関数本体を見ない」軽い処理だけにしている。本体の型検査・lowering は
//! codegen 側で関数ごとに行い、キャッシュヒットした関数は本体に一切触れない。


use syn::spanned::Spanned;

use crate::enc::{Enc, Rd};
use crate::lexer::{ItemKind, SrcItem};

pub type R<T> = Result<T, String>;

pub use crate::program::*;

pub fn err<T>(span: proc_macro2::Span, msg: impl std::fmt::Display) -> R<T> {
    let s = span.start();
    Err(format!("{}:{}: {}", s.line, s.column + 1, msg))
}

pub use crate::types::*;

pub fn bound_string(t: &syn::TraitBound) -> String {
    use quote::ToTokens;
    t.path
        .segments
        .last()
        .map(|s| s.to_token_stream().to_string())
        .unwrap_or_default()
}

/// `Fn(A, B) -> R` 形式の境界なら (種類, 引数, 戻り値)
pub fn parse_fn_bound(s: &str) -> Option<(&'static str, Vec<TySyn>, TySyn)> {
    let kind = ["FnOnce", "FnMut", "Fn"]
        .into_iter()
        .find(|k| s.starts_with(k) && s[k.len()..].trim_start().starts_with('('))?;
    let tb: syn::TraitBound = syn::parse_str(s).ok()?;
    let syn::PathArguments::Parenthesized(pa) = &tb.path.segments.last()?.arguments else {
        return None;
    };
    let args = pa
        .inputs
        .iter()
        .map(|t| parse_tysyn(t, None).ok())
        .collect::<Option<Vec<_>>>()?;
    let ret = match &pa.output {
        syn::ReturnType::Default => TySyn::Prim(Ty::Unit),
        syn::ReturnType::Type(_, t) => parse_tysyn(t, None).ok()?,
    };
    Some((kind, args, ret))
}

/// 型の intern 表。種類数はプログラム中の型の数で抑えられるので解放しない。
/// 型の構文を TySyn にする。`Self` は self_ty (impl の self 型) に置き換える。
/// 本体の中では self_ty に `Named("Self")` を渡し、解決時に TyEnv から引く
pub fn parse_tysyn(t: &syn::Type, self_ty: Option<&TySyn>) -> R<TySyn> {
    Ok(match t {
        syn::Type::Never(_) => TySyn::Prim(Ty::Never),
        syn::Type::Tuple(tt) if tt.elems.is_empty() => TySyn::Prim(Ty::Unit),
        syn::Type::Tuple(tt) => TySyn::Tuple(
            tt.elems
                .iter()
                .map(|e| parse_tysyn(e, self_ty))
                .collect::<R<_>>()?,
        ),
        syn::Type::Paren(p) => parse_tysyn(&p.elem, self_ty)?,
        syn::Type::BareFn(bf) => {
            if bf.variadic.is_some() || bf.abi.is_some() {
                return err(bf.span(), "only Rust ABI fn pointers are supported");
            }
            let mut v = vec![];
            for a in &bf.inputs {
                v.push(parse_tysyn(&a.ty, self_ty)?);
            }
            v.push(match &bf.output {
                syn::ReturnType::Default => TySyn::Prim(Ty::Unit),
                syn::ReturnType::Type(_, t) => parse_tysyn(t, self_ty)?,
            });
            TySyn::FnPtr(v)
        }
        syn::Type::TraitObject(to) => {
            let mut tr = None;
            for b in &to.bounds {
                match b {
                    // (`dyn Error + Send + Sync`: 自動 trait は読み飛ばす)
                    syn::TypeParamBound::Trait(t)
                        if t.path.segments.last().is_some_and(|s| matches!(s.ident.to_string().as_str(), "Send" | "Sync")) => {}
                    syn::TypeParamBound::Trait(t) if tr.is_none() => tr = Some(bound_string(t)),
                    syn::TypeParamBound::Lifetime(_) => {}
                    _ => return err(b.span(), "only one trait is supported in `dyn`"),
                }
            }
            match tr {
                Some(s) => TySyn::Dyn(s),
                None => return err(to.span(), "`dyn` needs a trait"),
            }
        }
        syn::Type::Group(g) => parse_tysyn(&g.elem, self_ty)?,
        // ライフタイムはコード生成に影響しないので消去する
        syn::Type::Reference(r) => TySyn::Ref(
            r.mutability.is_some(),
            Box::new(parse_tysyn(&r.elem, self_ty)?),
        ),
        syn::Type::Ptr(p) => TySyn::Ptr(
            p.mutability.is_some(),
            Box::new(parse_tysyn(&p.elem, self_ty)?),
        ),
        syn::Type::Slice(sl) => TySyn::Slice(Box::new(parse_tysyn(&sl.elem, self_ty)?)),
        syn::Type::Array(a) => {
            let len = match &a.len {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Int(i),
                    ..
                }) => ArrLen::Lit(i.base10_parse().map_err(|e| e.to_string())?),
                syn::Expr::Path(p) if p.path.get_ident().is_some() => {
                    ArrLen::Const(p.path.get_ident().unwrap().to_string())
                }
                // 関連定数 `[u8; Buf::CAP]` / `[u8; Self::CAP]`
                syn::Expr::Path(p) if p.qself.is_none() && p.path.segments.len() == 2 => {
                    let a = p.path.segments[0].ident.to_string();
                    let a = match (a.as_str(), self_ty) {
                        ("Self", Some(TySyn::Named(n))) => n.clone(),
                        _ => a,
                    };
                    ArrLen::Const(format!("{a}::{}", p.path.segments[1].ident))
                }
                e => return err(e.span(), "array length must be a literal or a const"),
            };
            TySyn::Array(Box::new(parse_tysyn(&a.elem, self_ty)?), len)
        }
        syn::Type::Infer(_) => TySyn::Infer,
        // `Self::Item` / `I::Item` (大文字で始まる 2 セグメントのパス)
        syn::Type::Path(p)
            if p.qself.is_none()
                && p.path.segments.len() == 2
                && p.path.segments[0].arguments.is_none()
                && p.path.segments[1].arguments.is_none()
                && p.path.segments[0]
                    .ident
                    .to_string()
                    .starts_with(|c: char| c.is_ascii_uppercase()) =>
        {
            let b = p.path.segments[0].ident.to_string();
            let base = if b == "Self" {
                self_ty.cloned().unwrap_or(TySyn::Named("Self".into()))
            } else {
                TySyn::Named(b)
            };
            TySyn::Assoc(Box::new(base), p.path.segments[1].ident.to_string())
        }
        // `<T as Iterator>::Item`
        syn::Type::Path(p) if p.qself.is_some() && p.path.segments.len() >= 2 => {
            let q = p.qself.as_ref().unwrap();
            TySyn::Assoc(
                Box::new(parse_tysyn(&q.ty, self_ty)?),
                p.path.segments.last().unwrap().ident.to_string(),
            )
        }
        // `io::Error` / `io::Result<T>` (prelude の IoError)
        syn::Type::Path(p)
            if p.qself.is_none()
                && p.path.segments.len() >= 2
                && p.path.segments[p.path.segments.len() - 2].ident == "io"
                && matches!(p.path.segments.last().unwrap().ident.to_string().as_str(), "Result" | "Error") =>
        {
            let last = p.path.segments.last().unwrap();
            let io_err = TySyn::Named("IoError".into());
            if last.ident == "Error" {
                io_err
            } else {
                let syn::PathArguments::AngleBracketed(ab) = &last.arguments else {
                    return err(t.span(), "`io::Result` needs a type argument");
                };
                let Some(syn::GenericArgument::Type(a)) = ab.args.first() else {
                    return err(t.span(), "`io::Result` needs a type argument");
                };
                TySyn::App("Result".into(), vec![parse_tysyn(a, self_ty)?, io_err])
            }
        }
        // `fmt::Result` / `fmt::Error` (`std::fmt::Result` も)
        syn::Type::Path(p)
            if p.qself.is_none()
                && p.path.segments.len() >= 2
                && p.path.segments[p.path.segments.len() - 2].ident == "fmt"
                && matches!(p.path.segments.last().unwrap().ident.to_string().as_str(), "Result" | "Error") =>
        {
            let err = TySyn::Named("__FmtError".into());
            if p.path.segments.last().unwrap().ident == "Error" {
                err
            } else {
                TySyn::App("Result".into(), vec![TySyn::Prim(Ty::Unit), err])
            }
        }
        syn::Type::Path(p) if p.qself.is_none() => {
            // `core::ffi::c_int` などは最後のセグメントで見る
            let seg = p.path.segments.last().unwrap();
            let s = seg.ident.to_string();
            let generic_args = match &seg.arguments {
                syn::PathArguments::None => vec![],
                syn::PathArguments::AngleBracketed(ab) => {
                    let mut v = vec![];
                    for a in &ab.args {
                        match a {
                            syn::GenericArgument::Type(t) => v.push(parse_tysyn(t, self_ty)?),
                            syn::GenericArgument::Lifetime(_) => {}
                            // `Buf<4>`: const ジェネリクスの引数 (長さを持つ配列型 `[(); 4]` として持つ)
                            syn::GenericArgument::Const(e) => v.push(const_arg_syn(e)?),
                            _ => return err(a.span(), "unsupported generic argument"),
                        }
                    }
                    v
                }
                _ => return err(t.span(), "unsupported type path"),
            };
            if !generic_args.is_empty() {
                return Ok(TySyn::App(s, generic_args));
            }
            match Ty::prim(&s) {
                Some(t) => TySyn::Prim(t),
                None if s == "Self" => match self_ty {
                    Some(t) => t.clone(),
                    None => return err(t.span(), "`Self` is only available in impls"),
                },
                None if s == "str" => TySyn::Str,
                None => match ffi_alias(&s) {
                    Some(t) => TySyn::Prim(t),
                    None => TySyn::Named(s),
                },
            }
        }
        _ => return err(t.span(), "unsupported type"),
    })
}

/// `core::ffi` の C 型の別名 (x86-64 / AArch64 Linux)
fn ffi_alias(s: &str) -> Option<Ty> {
    Some(match s {
        "c_char" | "c_schar" => Ty::I8,
        "c_uchar" => Ty::U8,
        "c_short" => Ty::I16,
        "c_ushort" => Ty::U16,
        "c_int" => Ty::I32,
        "c_uint" => Ty::U32,
        "c_long" | "c_longlong" => Ty::I64,
        "c_ulong" | "c_ulonglong" => Ty::U64,
        _ => return None,
    })
}

/// syn の式をソースに戻す (const の式を要約に保存するため)
mod quote_like {
    pub trait ToSrc {
        fn to_src(&self) -> String;
    }
    impl ToSrc for syn::Expr {
        fn to_src(&self) -> String {
            use quote::ToTokens;
            self.to_token_stream().to_string()
        }
    }
}

/// `#[name]` か `#[unsafe(name)]` が付いているか
fn has_attr(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs.iter().any(|a| {
        if a.path().is_ident(name) {
            return true;
        }
        if a.path().is_ident("unsafe") {
            let mut found = false;
            let _ = a.parse_nested_meta(|m| {
                found |= m.path.is_ident(name);
                Ok(())
            });
            return found;
        }
        false
    })
}

/// `#[derive(A, B)]` の名前
fn derive_names(attrs: &[syn::Attribute]) -> Vec<String> {
    let mut out = vec![];
    for a in attrs {
        if a.path().is_ident("derive") {
            let _ = a.parse_nested_meta(|m| {
                if let Some(id) = m.path.get_ident() {
                    out.push(id.to_string());
                }
                Ok(())
            });
        }
    }
    out
}


/// 関数 body の AST。クロージャなら親の本体から k 番目を取り出して fn にする
pub fn fn_ast(prog: &Program, f: syn::ItemFn, body: u32) -> syn::ItemFn {
    match prog.closures.get(&body) {
        Some(cd) => crate::closure::synth(&crate::closure::closures_of(&f.block)[cd.k as usize]),
        None => f,
    }
}

/// syn で 1 アイテムを parse する。エラー位置はファイル全体での行に補正する。
/// impl のヘッダは空の impl として、メンバは `Item::Fn` に直して返す。
pub fn parse_item(src: &str, it: &SrcItem) -> R<syn::Item> {
    let mut item = parse_item_raw(src, it)?;
    if let syn::Item::Fn(f) = &mut item {
        desugar_param_patterns(f);
    }
    Ok(item)
}

/// 引数のパターン (`fn f((i, j): (usize, usize))`) は名前の引数にして、本体の先頭で `let (i, j) = 引数;` にする
fn desugar_param_patterns(f: &mut syn::ItemFn) {
    let mut lets: Vec<syn::Stmt> = vec![];
    for (k, a) in f.sig.inputs.iter_mut().enumerate() {
        let syn::FnArg::Typed(pt) = a else { continue };
        if matches!(&*pt.pat, syn::Pat::Ident(_) | syn::Pat::Wild(_)) {
            continue;
        }
        let name = syn::Ident::new(&format!("__exr_arg{k}"), proc_macro2::Span::call_site());
        let pat = std::mem::replace(&mut *pt.pat, syn::parse_quote!(#name));
        lets.push(syn::parse_quote!(let #pat = #name;));
    }
    if !lets.is_empty() {
        lets.append(&mut f.block.stmts);
        f.block.stmts = lets;
    }
}

fn parse_item_raw(src: &str, it: &SrcItem) -> R<syn::Item> {
    let text = &src[it.start..it.end];
    let fix = |e: syn::Error| {
        let s = e.span().start();
        format!("{}:{}: {e}", s.line + it.line - 1, s.column + 1)
    };
    match it.kind {
        ItemKind::Normal => syn::parse_str::<syn::Item>(text).map_err(fix),
        // クレート属性 (`#![no_std]` など) は意味を持たない
        ItemKind::InnerAttr => Ok(syn::Item::Verbatim(Default::default())),
        ItemKind::ImplHeader | ItemKind::TraitHeader => {
            syn::parse_str::<syn::Item>(&format!("{text}}}")).map_err(fix)
        }
        ItemKind::TraitMember(_) => {
            let f = parse_trait_member(src, it)?;
            match f.default {
                Some(block) => Ok(syn::Item::Fn(syn::ItemFn {
                    attrs: f.attrs,
                    vis: syn::Visibility::Inherited,
                    sig: f.sig,
                    block: Box::new(block),
                })),
                None => err(f.sig.span(), "trait method has no default body")
                    .map_err(|e| relocate_err(e, it)),
            }
        }
        ItemKind::ImplMember(_) => match syn::parse_str::<syn::ImplItem>(text).map_err(fix)? {
            syn::ImplItem::Fn(f) => Ok(syn::Item::Fn(syn::ItemFn {
                attrs: f.attrs,
                vis: f.vis,
                sig: f.sig,
                block: Box::new(f.block),
            })),
            syn::ImplItem::Type(t) => Ok(syn::Item::Type(syn::ItemType {
                attrs: t.attrs,
                vis: t.vis,
                type_token: t.type_token,
                ident: t.ident,
                generics: t.generics,
                eq_token: t.eq_token,
                ty: Box::new(t.ty),
                semi_token: t.semi_token,
            })),
            // 関連定数 `const N: usize = 4;` (要約で `S::N` という名前の const にする)
            syn::ImplItem::Const(c) => Ok(syn::Item::Const(syn::ItemConst {
                attrs: c.attrs,
                vis: c.vis,
                const_token: c.const_token,
                ident: c.ident,
                generics: c.generics,
                colon_token: c.colon_token,
                ty: Box::new(c.ty),
                eq_token: c.eq_token,
                expr: Box::new(c.expr),
                semi_token: c.semi_token,
            })),
            other => err(other.span(), "only fns, consts and associated types are supported in impl blocks")
                .map_err(|e| relocate_err(e, it)),
        },
    }
}

pub fn parse_trait_member(src: &str, it: &SrcItem) -> R<syn::TraitItemFn> {
    let text = &src[it.start..it.end];
    match syn::parse_str::<syn::TraitItem>(text) {
        Ok(syn::TraitItem::Fn(f)) => Ok(f),
        Ok(other) => {
            err(other.span(), "only fns are supported in traits").map_err(|e| relocate_err(e, it))
        }
        Err(e) => {
            let s = e.span().start();
            Err(format!("{}:{}: {e}", s.line + it.line - 1, s.column + 1))
        }
    }
}

/// アイテムを parse して要約する。owner は impl / trait のメンバのとき、その self 型 / trait 名。
/// codegen でも使う AST (本体のある関数) があれば一緒に返す
pub fn parse_and_summarize(
    src: &str,
    it: &SrcItem,
    owner: Option<&Owner>,
) -> R<(Summary, Option<syn::Item>)> {
    if let ItemKind::TraitMember(_) = it.kind {
        let ow = owner.ok_or("trait member without trait")?;
        // `type Item;` (関連型の宣言) は何も持たない
        match syn::parse_str::<syn::TraitItem>(&src[it.start..it.end]) {
            Ok(syn::TraitItem::Type(_)) => return Ok((Summary::default(), None)),
            // `const K: i32;` (既定値があれば `Trait::K` という const にする: impl が持たないときに使う)
            Ok(syn::TraitItem::Const(c)) => {
                let mut sm = Summary::default();
                if let Some((_, e)) = &c.default {
                    use quote_like::ToSrc;
                    sm.consts.push((format!("{}::{}", ow.base, c.ident), parse_tysyn(&c.ty, None)?, e.to_src()));
                }
                return Ok((sm, None));
            }
            _ => {}
        }
        let f = parse_trait_member(src, it)?;
        let mut sig = parse_sig(&f.sig, false, false, Some(ow)).map_err(|e| relocate_err(e, it))?;
        sig.has_body = f.default.is_some();
        let sm = Summary {
            fns: vec![(format!("{}::{}", ow.base, f.sig.ident), sig)],
            ..Default::default()
        };
        let item = f.default.map(|block| {
            syn::Item::Fn(syn::ItemFn {
                attrs: f.attrs,
                vis: syn::Visibility::Inherited,
                sig: f.sig,
                block: Box::new(block),
            })
        });
        return Ok((sm, item));
    }
    let item = parse_item(src, it)?;
    let sm = summarize(&item, owner).map_err(|e| relocate_err(e, it))?;
    let keep = matches!(item, syn::Item::Fn(_));
    Ok((sm, keep.then_some(item)))
}

/// ジェネリックな struct / enum の型パラメータ名 (境界は使わない)
fn adt_params(g: &syn::Generics) -> R<Vec<String>> {
    let mut v = vec![];
    collect_generics(g, &mut v)?;
    Ok(v.into_iter().map(|x| x.0).collect())
}

/// "L:C: msg" 形式のエラーの行をアイテム開始行だけずらす。
pub fn relocate_err(e: String, it: &SrcItem) -> String {
    if let Some((l, rest)) = e.split_once(':') {
        if let Ok(l) = l.parse::<usize>() {
            return format!("{}:{rest}", l + it.line - 1);
        }
    }
    e
}

/// `owner` は impl のメンバのとき、その impl
pub fn summarize(item: &syn::Item, owner: Option<&Owner>) -> R<Summary> {
    let mut sm = Summary::default();
    match item {
        syn::Item::Type(t) if owner.is_some() => {
            let ow = owner.unwrap();
            sm.assocs.push(AssocSyn {
                name: t.ident.to_string(),
                self_ty: ow.self_ty.clone(),
                generics: ow.generics.iter().map(|g| g.0.clone()).collect(),
                value: parse_tysyn(&t.ty, Some(&ow.self_ty))?,
            });
        }
        syn::Item::Fn(f) if owner.is_some() => {
            let ow = owner.unwrap();
            let mut sig = parse_sig(&f.sig, false, false, Some(ow))?;
            // Display と Debug はどちらも fmt なので、trait 名で区別する (`P::<Display>::fmt`)
            let name = match ow.impl_trait.as_deref() {
                Some(t @ ("Display" | "Debug")) => format!("{}::<{t}>::{}", ow.base, f.sig.ident),
                Some(t) if t.starts_with("PartialEq<") || t.starts_with("PartialOrd<") => {
                    format!("{}::<{t}>::{}", ow.base, f.sig.ident)
                }
                // (参照への impl のメソッドは名前を分ける: `impl Add for &M` の add は `M::__ref_add`)
                _ if matches!(ow.self_ty, TySyn::Ref(..)) => format!("{}::__ref_{}", ow.base, f.sig.ident),
                _ => format!("{}::{}", ow.base, f.sig.ident),
            };
            lift_closures(f, &name, &mut sig, Some(&ow.self_ty), &mut sm)?;
            sm.fns.push((name, sig));
        }
        // 型の別名 (`type Key = u128;`)
        syn::Item::Type(t) => {
            let mut g = vec![];
            collect_generics(&t.generics, &mut g)?;
            sm.aliases.push((t.ident.to_string(), g.into_iter().map(|x| x.0).collect(), parse_tysyn(&t.ty, None)?));
        }
        syn::Item::Trait(t) => {
            only_lifetimes(&t.generics)?;
            sm.trait_decl = Some(t.ident.to_string());
        }
        syn::Item::Impl(im) => {
            if let Some((bang, tr, _)) = &im.trait_ {
                if bang.is_some() {
                    return err(tr.span(), "negative impls are not supported");
                }
                // `fmt::Display` などは最後のセグメント
                let Some(last) = tr.segments.last() else {
                    return err(tr.span(), "unsupported trait path");
                };
                let id = &last.ident;
                sm.impl_trait = Some(id.to_string());
                // `impl PartialEq<[U; N]> for Vec<T>`: 右辺の型が self 型と違う比較は trait 名に型引数を残す
                // (`PartialEq<[U;N]>`。メソッドは `Vec::<PartialEq<[U;N]>>::eq` になり、`==` が右辺の型で選ぶ)
                // (`impl PartialOrd<Feet> for Meters` も同じ: `Meters::<PartialOrd<Feet>>::partial_cmp`)
                if id == "PartialEq" || id == "PartialOrd" {
                    if let syn::PathArguments::AngleBracketed(ab) = &last.arguments {
                        use quote::ToTokens;
                        let rhs: String = ab.to_token_stream().to_string().chars().filter(|c| !c.is_whitespace()).collect();
                        let selfs: String = im.self_ty.to_token_stream().to_string().chars().filter(|c| !c.is_whitespace()).collect();
                        if rhs != format!("<{selfs}>") && rhs != "<Self>" {
                            sm.impl_trait = Some(format!("{id}{rhs}"));
                        }
                    }
                }
            }
            collect_generics(&im.generics, &mut sm.impl_generics)?;
            // `impl Add for &M`: M のメソッド `__ref_add` (self は &M) として持つ
            let self_path = match &*im.self_ty {
                syn::Type::Reference(r) if im.trait_.is_some() => &*r.elem,
                t => t,
            };
            // `impl<A, B> Tr for (A, B)`: タプルの struct (`#tuple2`) のメソッド
            if let (syn::Type::Tuple(tt), true) = (self_path, im.trait_.is_some()) {
                sm.impl_self = Some(format!("#tuple{}", tt.elems.len()));
                sm.impl_self_ty = Some(parse_tysyn(&im.self_ty, None)?);
                return Ok(sm);
            }
            // `impl .. for [T; N]` / `for [T]`: 名前は `__array` / `__slice` (比較の impl だけが使う)
            if matches!(self_path, syn::Type::Array(_) | syn::Type::Slice(_)) && im.trait_.is_some() {
                sm.impl_self = Some(if matches!(self_path, syn::Type::Array(_)) { "__array" } else { "__slice" }.to_string());
                sm.impl_self_ty = Some(parse_tysyn(&im.self_ty, None)?);
                return Ok(sm);
            }
            let syn::Type::Path(tp) = self_path else {
                return err(im.self_ty.span(), "unsupported impl self type");
            };
            if tp.path.segments.len() != 1 {
                return err(im.self_ty.span(), "unsupported impl self type");
            }
            sm.impl_self = Some(tp.path.segments[0].ident.to_string());
            sm.impl_self_ty = Some(parse_tysyn(&im.self_ty, None)?);
        }
        syn::Item::Fn(f) => {
            let name = f.sig.ident.to_string();
            let export = has_attr(&f.attrs, "no_mangle");
            // `#[no_mangle] extern "C" fn main() -> i32` (no_main) は C の main そのもの
            let is_main = name == "main" && !export;
            let mut sig = parse_sig(&f.sig, false, is_main, None)?;
            // (`fn main() -> Result<(), E>` も: Err なら `Error: {:?}` を出して 1 で終わる)
            let result_ret = matches!(&sig.ret, TySyn::App(n, _) if n == "Result");
            if is_main && (!sig.params.is_empty() || (sig.ret != TySyn::Prim(Ty::Unit) && !result_ret)) {
                return err(f.sig.span(), "`main` must be `fn main()`");
            }
            if export {
                sig.export = true;
                sig.symbol = name.clone();
            }
            if has_attr(&f.attrs, "panic_handler") {
                sig.panic_handler = true;
                if sig.ret != TySyn::Prim(Ty::Never) {
                    return err(f.sig.span(), "`#[panic_handler]` function must return `!`");
                }
            }
            lift_closures(f, &name, &mut sig, None, &mut sm)?;
            sm.fns.push((name, sig));
        }
        // `use` は名前の最後のセグメントで解決するので読み飛ばす。クレート属性も同じ
        syn::Item::Use(_) | syn::Item::Verbatim(_) => {}
        syn::Item::Const(c) => {
            use quote_like::ToSrc;
            // (impl の中なら `S::N`)
            let name = match owner {
                Some(ow) => format!("{}::{}", ow.base, c.ident),
                None => c.ident.to_string(),
            };
            sm.consts.push((name, parse_tysyn(&c.ty, owner.map(|o| &o.self_ty))?, c.expr.to_src()));
        }
        syn::Item::Static(st) => {
            use quote_like::ToSrc;
            let m = matches!(st.mutability, syn::StaticMutability::Mut(_));
            sm.statics.push((
                st.ident.to_string(),
                parse_tysyn(&st.ty, None)?,
                st.expr.to_src(),
                m,
            ));
        }
        syn::Item::ForeignMod(m) => {
            for fi in &m.items {
                let syn::ForeignItem::Fn(f) = fi else {
                    return err(fi.span(), "only fns are supported in extern blocks");
                };
                let mut sig = parse_sig(&f.sig, true, false, None)?;
                // `#[link_name = "malloc"]`: シンボル名だけ変える
                for a in &f.attrs {
                    if let syn::Meta::NameValue(nv) = &a.meta {
                        if nv.path.is_ident("link_name") {
                            if let syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Str(l), ..
                            }) = &nv.value
                            {
                                sig.symbol = l.value();
                            }
                        }
                    }
                }
                sm.fns.push((f.sig.ident.to_string(), sig));
            }
        }
        syn::Item::Enum(e) => {
            sm.adt_generics = adt_params(&e.generics)?;
            let sn = e.ident.to_string();
            let selft = self_of(&sn, &sm.adt_generics);
            let sn_ty = Some(&selft);
            let mut vs = vec![];
            for v in &e.variants {
                let (kind, fields) = match &v.fields {
                    syn::Fields::Unit => (VariantKind::Unit, vec![]),
                    syn::Fields::Unnamed(u) => {
                        let mut fs = vec![];
                        for (i, f) in u.unnamed.iter().enumerate() {
                            fs.push((i.to_string(), parse_tysyn(&f.ty, sn_ty)?));
                        }
                        (VariantKind::Tuple, fs)
                    }
                    syn::Fields::Named(nf) => {
                        let mut fs = vec![];
                        for f in &nf.named {
                            fs.push((
                                f.ident.as_ref().unwrap().to_string(),
                                parse_tysyn(&f.ty, sn_ty)?,
                            ));
                        }
                        (VariantKind::Struct, fs)
                    }
                };
                let discr = match &v.discriminant {
                    None => None,
                    Some((_, ex)) => Some(const_int(ex)?),
                };
                vs.push(VariantSyn {
                    name: v.ident.to_string(),
                    kind,
                    fields,
                    discr,
                });
            }
            let d = derive_names(&e.attrs);
            if !d.is_empty() {
                sm.derives.push((sn.clone(), d));
            }
            sm.enums.push((sn, vs));
        }
        syn::Item::Struct(s) => {
            sm.adt_generics = adt_params(&s.generics)?;
            let selft = self_of(&s.ident.to_string(), &sm.adt_generics);
            let mut fields = vec![];
            match &s.fields {
                syn::Fields::Named(nf) => {
                    for f in &nf.named {
                        let t = parse_tysyn(&f.ty, Some(&selft))?;
                        fields.push((f.ident.as_ref().unwrap().to_string(), t));
                    }
                }
                // タプル構造体: フィールド名は "0", "1", ...
                syn::Fields::Unnamed(u) => {
                    for (i, f) in u.unnamed.iter().enumerate() {
                        fields.push((i.to_string(), parse_tysyn(&f.ty, Some(&selft))?));
                    }
                }
                syn::Fields::Unit => {}
            }
            let d = derive_names(&s.attrs);
            if !d.is_empty() {
                sm.derives.push((s.ident.to_string(), d));
            }
            sm.structs.push((s.ident.to_string(), fields));
        }
        _ => return err(item.span(), "unsupported item"),
    }
    Ok(sm)
}

/// fn 本体のクロージャを持ち上げる ([`crate::closure`])。戻り値位置の `impl Fn` も
/// ここで末尾のクロージャの型に置き換える
fn lift_closures(
    f: &syn::ItemFn,
    fname: &str,
    sig: &mut SigSyn,
    self_ty: Option<&TySyn>,
    sm: &mut Summary,
) -> R<()> {
    use crate::closure;
    let cs = closure::closures_of(&f.block);
    if cs.is_empty() {
        if sig.ret == TySyn::Named(IMPL_RET.into()) {
            sig.ret = TySyn::Named(OPAQUE_RET.into());
        }
        return Ok(());
    }
    let bound = closure::bound_in_fn(f);
    let mut out = vec![];
    for (k, c) in cs.iter().enumerate() {
        let name = format!("{fname}::{{closure#{k}}}");
        let mut generics: Vec<String> = sig.generics.iter().map(|g| g.0.clone()).collect();
        let n_parent = generics.len() as u64;
        let caps = closure::captures(c, &bound);
        let mut fields = vec![];
        for (i, n) in caps.iter().enumerate() {
            let g = format!("__C{i}");
            fields.push((n.clone(), TySyn::Named(g.clone())));
            generics.push(g);
        }
        let mut params = vec![];
        for (i, p) in c.inputs.iter().enumerate() {
            match closure::param(p).1 {
                Some(t) => params.push(parse_tysyn(t, self_ty)?),
                None => {
                    let g = format!("__P{i}");
                    params.push(TySyn::Named(g.clone()));
                    generics.push(g);
                }
            }
        }
        generics.push("__R".into());
        let ret_ann = match &c.output {
            syn::ReturnType::Default => None,
            syn::ReturnType::Type(_, t) => Some(parse_tysyn(t, self_ty)?),
        };
        let env = TySyn::App(
            name.clone(),
            generics.iter().map(|g| TySyn::Named(g.clone())).collect(),
        );
        params.insert(0, TySyn::Ptr(true, Box::new(env)));
        let mut gs: Vec<(String, Vec<String>)> = sig.generics.clone();
        gs.extend(generics[n_parent as usize..].iter().map(|g| (g.clone(), vec![])));
        let csig = SigSyn {
            params,
            ret: TySyn::Named("__R".into()),
            symbol: format!("{}.c{k}", sig.symbol),
            impl_of: sig.impl_of.clone(),
            generics: gs,
            has_body: true,
            self_ty: sig.self_ty.clone(),
            ..Default::default()
        };
        let assigns = closure::assigns_capture(c, &caps);
        let moved: Vec<bool> = caps
            .iter()
            .map(|n| c.capture.is_some() || (closure::moves_capture(c, std::slice::from_ref(n)) && !assigns))
            .collect();
        out.push(ClosureSyn {
            name,
            generics,
            fields,
            sig: csig,
            // (`move` がなくても、本体がキャプチャを値として渡す・返すなら、そのキャプチャは値で持つ: rustc と同じく FnOnce)
            by_move: by_move_of(&moved, c.capture.is_some()),
            moved,
            ret_ann,
            n_parent,
        });
    }
    // `-> impl Fn(A) -> B { move |x| .. }`: 型は末尾のクロージャの struct。
    // キャプチャは引数だけ (型が注釈から分かる) に限る
    if sig.ret == TySyn::Named(IMPL_RET.into()) {
        // (末尾がクロージャでなければ、本体の型推論で決める: `-> impl Iterator<Item = u32>`)
        let tail = match f.block.stmts.last() {
            Some(syn::Stmt::Expr(e, None)) => Some(e),
            _ => None,
        };
        let mut tail = tail;
        while let Some(syn::Expr::Paren(p)) = tail {
            tail = Some(&p.expr);
        }
        let Some(syn::Expr::Closure(tc)) = tail else {
            sig.ret = TySyn::Named(OPAQUE_RET.into());
            sm.closures = out;
            return Ok(());
        };
        let key = closure::closure_key(tc);
        let k = cs.iter().position(|c| closure::closure_key(c) == key).unwrap();
        let syn::ReturnType::Type(_, rt) = &f.sig.output else {
            unreachable!()
        };
        let syn::Type::ImplTrait(it) = &**rt else {
            unreachable!()
        };
        let bs = bound_names(&it.bounds)?;
        let Some((_, bargs, bret)) = bs.iter().find_map(|b| parse_fn_bound(b)) else {
            return err(rt.span(), "`impl Trait` return types must be `impl Fn(..) -> ..`");
        };
        let cl = &out[k];
        let mut args: Vec<TySyn> = sig.generics.iter().map(|g| TySyn::Named(g.0.clone())).collect();
        for (n, _) in &cl.fields {
            if cl.by_move {
                let ty = f.sig.inputs.iter().zip(&sig.params).find_map(|(a, t)| match a {
                    syn::FnArg::Typed(pt) => match &*pt.pat {
                        syn::Pat::Ident(pi) if pi.ident == n => Some(t.clone()),
                        _ => None,
                    },
                    _ => None,
                });
                // (引数でなければ、本体の `let` の注釈か初期化式のリテラルから型を決める)
                // (接尾辞のない整数リテラルは、戻り値の型が整数ならそれ: `let mut c = 0; move || { c += 1; c }`)
                let int_hint = match &bret {
                    TySyn::Prim(t) if t.is_int() => Some(*t),
                    _ => None,
                };
                match ty.or_else(|| local_ty_syn(&f.block, n, int_hint)) {
                    Some(t) => args.push(t),
                    None => {
                        return err(
                            tc.span(),
                            format!("a returned closure can only capture parameters and locals with an obvious type (`{n}`; annotate it: `let {n}: T = ..`)"),
                        );
                    }
                }
            } else {
                return err(tc.span(), "a returned closure that captures must be `move`");
            }
        }
        for (i, p) in tc.inputs.iter().enumerate() {
            if closure::param(p).1.is_none() {
                match bargs.get(i) {
                    Some(t) => args.push(t.clone()),
                    None => return err(p.span(), "closure has more parameters than the bound"),
                }
            }
        }
        args.push(cl.ret_ann.clone().unwrap_or(bret));
        sig.ret = TySyn::App(cl.name.clone(), args);
    }
    sm.closures = out;
    Ok(())
}

impl Program {
    pub fn eval_int(&self, e: &syn::Expr, depth: u32) -> R<i128> {
        use syn::{BinOp, Expr, Lit, UnOp};
        Ok(match e {
            Expr::Lit(l) => match &l.lit {
                Lit::Int(i) => i.base10_parse::<i128>().or_else(|_| i.base10_parse::<u128>().map(|v| v as i128)).map_err(|e| e.to_string())?,
                Lit::Bool(b) => b.value as i128,
                Lit::Byte(b) => b.value() as i128,
                Lit::Char(c) => c.value() as i128,
                _ => return Err("not an integer constant".into()),
            },
            Expr::Paren(p) => self.eval_int(&p.expr, depth)?,
            Expr::Group(g) => self.eval_int(&g.expr, depth)?,
            Expr::Cast(c) => self.eval_int(&c.expr, depth)?,
            Expr::Unary(u) => {
                let v = self.eval_int(&u.expr, depth)?;
                match u.op {
                    UnOp::Neg(_) => -v,
                    UnOp::Not(_) => !v,
                    _ => return Err("not an integer constant".into()),
                }
            }
            Expr::Binary(b) => {
                let (l, r) = (
                    self.eval_int(&b.left, depth)?,
                    self.eval_int(&b.right, depth)?,
                );
                match b.op {
                    BinOp::Add(_) => l + r,
                    BinOp::Sub(_) => l - r,
                    BinOp::Mul(_) => l * r,
                    BinOp::Div(_) if r != 0 => l / r,
                    BinOp::Rem(_) if r != 0 => l % r,
                    BinOp::Shl(_) => l << r,
                    BinOp::Shr(_) => l >> r,
                    BinOp::BitAnd(_) => l & r,
                    BinOp::BitOr(_) => l | r,
                    BinOp::BitXor(_) => l ^ r,
                    _ => return Err("unsupported operator in constant".into()),
                }
            }
            Expr::Path(p) if p.path.get_ident().is_some() => {
                self.const_int(&p.path.get_ident().unwrap().to_string(), depth)?
            }
            Expr::Path(p) if p.path.segments.len() == 2 => {
                let t = Ty::prim(&p.path.segments[0].ident.to_string())
                    .ok_or("unsupported path in constant")?;
                prim_assoc_const(t, &p.path.segments[1].ident.to_string())
                    .ok_or("unsupported constant")?
            }
            _ => return Err("unsupported expression in constant".into()),
        })
    }

    /// ジェネリックな struct / enum に型引数を入れる。型引数がすべて具体型ならレイアウトも決める。
    /// 同じ (定義, 型引数) には同じ id を返す
    /// `-> impl Iterator<..>` の戻り値を本体の型推論で決める (呼び出し先も同じ形なら数回まわす)
    pub fn resolve_opaque(&mut self, src: &str, items: &[SrcItem]) -> R<()> {
        if self.opaque.is_empty() {
            return Ok(());
        }
        let ids = self.opaque.clone();
        let mut done = vec![false; ids.len()];
        for _ in 0..4 {
            let mut progress = false;
            for (k, &id) in ids.iter().enumerate() {
                if done[k] {
                    continue;
                }
                let sig = self.sigs[id as usize].clone();
                if sig.is_generic() {
                    return Err(format!("`{}`: generic functions returning `impl Trait` are not supported", self.names[id as usize]));
                }
                let Some(ii) = self.item_of[id as usize] else { continue };
                let it = &items[ii as usize];
                let syn::Item::Fn(f) = parse_item(src, it)? else { continue };
                let env = TyEnv { self_sid: sig.impl_of, params: vec![], self_ty: sig.self_ty };
                let (_, r, _) = crate::infer::infer_ctx(self, &f, &sig, &env, &[], None, Some((id, 0)));
                if let Some(t) = r.filter(|&t| t != Ty::Unit && !self.has_param(t)) {
                    self.sigs[id as usize].ret = t;
                    done[k] = true;
                    progress = true;
                }
            }
            if !progress {
                break;
            }
        }
        if let Some(k) = done.iter().position(|d| !d) {
            return Err(format!("cannot infer the `impl Trait` return type of `{}`", self.names[ids[k] as usize]));
        }
        Ok(())
    }

    /// 関数本体の中の型を解決する (`Self` と型パラメータは env から)
    pub fn parse_ty(&self, t: &syn::Type, env: &TyEnv) -> R<Ty> {
        let ts = parse_tysyn(t, Some(&TySyn::Named("Self".into())))?;
        self.resolve_env(&ts, env).or_else(|e| err(t.span(), e))
    }

    /// 型名として書かれたパス (`S` / `Self`) → struct id
    /// モジュールのパスの先頭 (`crate::` / `self::` / `super::` / 小文字のモジュール名) を外す。
    /// モジュールは平らにしてあるので、アイテムは最後の名前で引ける (`geo::inner::k` → `k`、`geo::P::new` → `P::new`)
    pub fn strip_mods(&self, p: &syn::Path) -> Option<syn::Path> {
        let mut k = 0;
        let n = p.segments.len();
        while k + 1 < n {
            let s = p.segments[k].ident.to_string();
            let module = matches!(s.as_str(), "crate" | "self" | "super")
                || (s.starts_with(|c: char| c.is_ascii_lowercase())
                    && !matches!(s.as_str(), "std" | "core" | "alloc" | "str" | "char" | "bool")
                    && Ty::prim(&s).is_none()
                    && !self.struct_index.contains_key(&s)
                    && p.segments[k].arguments.is_none());
            if !module {
                break;
            }
            k += 1;
        }
        if k == 0 {
            return None;
        }
        let mut q = p.clone();
        q.leading_colon = None;
        q.segments = p.segments.iter().skip(k).cloned().collect();
        Some(q)
    }

    /// `i32::pow(2, 10)` / `u64::count_ones(x)` / `f64::abs(x)`: プリミティブのメソッドを関数の形で呼んだもの。
    /// 関数として見つからなければ、メソッド呼び出し `(2i32).pow(10)` に書き換える
    pub fn prim_method_call(&self, c: &syn::ExprCall, self_sid: Option<u32>) -> Option<syn::Expr> {
        let syn::Expr::Path(p) = &*c.func else { return None };
        if c.args.is_empty() || p.qself.is_some() {
            return None;
        }
        // `Trait::m(&x, a)` / `crate::m::Trait::m(..)` (self を取るユーザ trait のメソッド): `x.m(a)` に
        {
            let q = self.strip_mods(&p.path);
            let q = q.as_ref().unwrap_or(&p.path);
            if q.segments.len() == 2 && q.segments.iter().all(|s| s.arguments.is_none()) {
                let tr = q.segments[0].ident.to_string();
                let m = &q.segments[1].ident;
                let ms = m.to_string();
                if let Some(ds) = self.traits.get(&tr) {
                    let selfm = ds.iter().any(|&d| {
                        self.sigs[d as usize].has_self && self.names[d as usize].rsplit("::").next() == Some(ms.as_str())
                    });
                    if selfm && !matches!(tr.as_str(), "Ord" | "PartialOrd" | "PartialEq" | "Eq") {
                        let mut args = c.args.iter();
                        let a0 = args.next()?;
                        let rest: Vec<&syn::Expr> = args.collect();
                        let recv: syn::Expr = match a0 {
                            syn::Expr::Reference(r) => {
                                let e = &r.expr;
                                syn::parse_quote!((#e))
                            }
                            _ => syn::parse_quote!((#a0)),
                        };
                        return Some(syn::parse_quote!(#recv.#m(#(#rest),*)));
                    }
                }
            }
        }
        if p.path.segments.len() != 2 {
            return None;
        }
        let ty = p.path.segments[0].ident.to_string();
        // `HashMap::from([(1, 2), ..])`: 配列を vec![..] にして `from(Vec)` を呼ぶ
        if p.path.segments[1].ident == "from"
            && matches!(ty.as_str(), "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "VecDeque" | "BinaryHeap")
            && c.args.len() == 1
        {
            if let syn::Expr::Array(a) = &c.args[0] {
                let es = &a.elems;
                let f = &c.func;
                return Some(syn::parse_quote!(#f(vec![#es])));
            }
        }
        // `Ord::cmp(&a, &b)` / `PartialOrd::partial_cmp(..)` / `PartialEq::eq(..)`: メソッド呼び出しの形に
        if matches!(ty.as_str(), "Ord" | "PartialOrd" | "PartialEq" | "Eq") && c.args.len() == 2 {
            let m = &p.path.segments[1].ident;
            let (a0, a1) = (&c.args[0], &c.args[1]);
            return Some(syn::parse_quote!((#a0).#m(#a1)));
        }
        let t = Ty::prim(&ty)?;
        if t == Ty::Char || t == Ty::Bool {
            return None;
        }
        let m = &p.path.segments[1].ident;
        let ms = m.to_string();
        // `i64::from(7u32)` / `u32::from('a')`: 数値への変換はキャスト (広げる変換だけが書ける)
        if ms == "from" && c.args.len() == 1 && self.fn_by_path(&p.path, self_sid).is_none() {
            let a0 = &c.args[0];
            let tyi = &p.path.segments[0].ident;
            return Some(syn::parse_quote!(((#a0) as #tyi)));
        }
        if ms.starts_with("from") || ms.starts_with("try_from") || ms == "default" || ms == "new" {
            return None;
        }
        if self.fn_by_path(&p.path, self_sid).is_some() {
            return None;
        }
        let mut args = c.args.iter();
        let a0 = args.next()?;
        let rest: Vec<&syn::Expr> = args.collect();
        let tyi = &p.path.segments[0].ident;
        // (リテラルは型を付ける: `i32::pow(2, 10)` の 2 は i32)
        let recv: syn::Expr = if matches!(a0, syn::Expr::Lit(_)) {
            syn::parse_quote!((#a0 as #tyi))
        } else {
            syn::parse_quote!((#a0))
        };
        Some(syn::parse_quote!(#recv.#m(#(#rest),*)))
    }



    pub fn variant_by_path(&self, p: &syn::Path, self_sid: Option<u32>) -> Option<(u32, usize)> {
        self.variant_by_p(&ppath(p), self_sid)
    }

    pub fn struct_by_path(&self, p: &syn::Path, self_sid: Option<u32>) -> Option<u32> {
        self.struct_by_p(&ppath(p), self_sid)
    }

    pub fn fn_by_path(&self, p: &syn::Path, self_sid: Option<u32>) -> Option<u32> {
        self.fn_by_p(&ppath(p), self_sid)
    }
}

// --- Summary のシリアライズ ------------------------------------------------


impl Enc for TySyn {
    fn enc(&self, out: &mut Vec<u8>) {
        match self {
            TySyn::Prim(t) => out.push(TYS.iter().position(|x| x == t).unwrap() as u8),
            TySyn::Ref(m, i) => {
                out.push(20 + *m as u8);
                i.enc(out);
            }
            TySyn::Named(n) => {
                out.push(22);
                n.enc(out);
            }
            TySyn::App(n, a) => {
                out.push(23);
                n.enc(out);
                a.enc(out);
            }
            TySyn::Tuple(a) => {
                out.push(24);
                a.enc(out);
            }
            TySyn::Array(i, ArrLen::Lit(n)) => {
                out.push(25);
                i.enc(out);
                n.enc(out);
            }
            TySyn::Array(i, ArrLen::Const(c)) => {
                out.push(26);
                i.enc(out);
                c.enc(out);
            }
            TySyn::Slice(i) => {
                out.push(27);
                i.enc(out);
            }
            TySyn::Str => out.push(28),
            TySyn::Ptr(m, i) => {
                out.push(29 + *m as u8);
                i.enc(out);
            }
            TySyn::Dyn(n) => {
                out.push(31);
                n.enc(out);
            }
            TySyn::FnPtr(v) => {
                out.push(32);
                v.enc(out);
            }
            TySyn::Assoc(b, n) => {
                out.push(33);
                b.enc(out);
                n.enc(out);
            }
            TySyn::Infer => out.push(34),
        }
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some(match r.u8()? {
            c @ 0..18 => TySyn::Prim(TYS[c as usize]),
            c @ (20 | 21) => TySyn::Ref(c == 21, Box::new(TySyn::dec(r)?)),
            22 => TySyn::Named(String::dec(r)?),
            23 => TySyn::App(String::dec(r)?, Vec::dec(r)?),
            24 => TySyn::Tuple(Vec::dec(r)?),
            25 => TySyn::Array(Box::new(TySyn::dec(r)?), ArrLen::Lit(u64::dec(r)?)),
            26 => TySyn::Array(Box::new(TySyn::dec(r)?), ArrLen::Const(String::dec(r)?)),
            27 => TySyn::Slice(Box::new(TySyn::dec(r)?)),
            28 => TySyn::Str,
            c @ (29 | 30) => TySyn::Ptr(c == 30, Box::new(TySyn::dec(r)?)),
            31 => TySyn::Dyn(String::dec(r)?),
            32 => TySyn::FnPtr(Vec::dec(r)?),
            33 => TySyn::Assoc(Box::new(TySyn::dec(r)?), String::dec(r)?),
            34 => TySyn::Infer,
            _ => return None,
        })
    }
}

impl Enc for Box<TySyn> {
    fn enc(&self, out: &mut Vec<u8>) {
        (**self).enc(out)
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some(Box::new(TySyn::dec(r)?))
    }
}

impl Enc for VariantKind {
    fn enc(&self, out: &mut Vec<u8>) {
        out.push(*self as u8)
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        [VariantKind::Unit, VariantKind::Tuple, VariantKind::Struct]
            .get(r.u8()? as usize)
            .copied()
    }
    fn is_empty_val(&self) -> bool {
        *self == VariantKind::Unit
    }
}

crate::enc_struct!(VariantSyn {
    name,
    kind,
    fields,
    discr
});
crate::enc_struct!(SigSyn {
    params,
    ret,
    symbol,
    is_extern,
    is_main,
    impl_of,
    has_self,
    generics,
    trait_of,
    has_body,
    impl_trait,
    self_ty,
    export,
    panic_handler,
});
crate::enc_struct!(Summary {
    fns,
    structs,
    impl_self,
    enums,
    trait_decl,
    impl_trait,
    derives,
    adt_generics,
    impl_generics,
    impl_self_ty,
    consts,
    statics,
    closures,
    assocs,
    aliases,
});
crate::enc_struct!(AssocSyn {
    name,
    self_ty,
    generics,
    value,
});
crate::enc_struct!(ClosureSyn {
    name,
    generics,
    fields,
    sig,
    by_move,
    moved,
    ret_ann,
    n_parent,
});

/// すべてのキャプチャを値で持つか (`move`、または値で使うキャプチャだけのもの)
pub fn by_move_of(moved: &[bool], kw: bool) -> bool {
    kw || (!moved.is_empty() && moved.iter().all(|&m| m))
}

impl Enc for (String, TySyn, String, bool) {
    fn enc(&self, out: &mut Vec<u8>) {
        self.0.enc(out);
        self.1.enc(out);
        self.2.enc(out);
        self.3.enc(out);
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some((
            String::dec(r)?,
            TySyn::dec(r)?,
            String::dec(r)?,
            bool::dec(r)?,
        ))
    }
}

/// キャッシュにインスタンスの型引数を書くため
pub fn put_ty(out: &mut Vec<u8>, t: &TySyn) {
    t.enc(out)
}

/// d[*pos..] から TySyn を 1 つ読む
pub fn read_ty(d: &[u8], pos: &mut usize) -> Option<TySyn> {
    let mut r = Rd { d, pos: *pos };
    let t = TySyn::dec(&mut r);
    *pos = r.pos;
    t
}

impl Summary {
    pub fn encode(&self, out: &mut Vec<u8>) {
        self.enc(out)
    }

    pub fn decode(d: &[u8]) -> Option<Summary> {
        Summary::dec(&mut Rd::new(d))
    }
}

/// 判別値などの整数定数式 (リテラルと単項マイナスだけ)
fn const_int(e: &syn::Expr) -> R<i64> {
    match e {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(i),
            ..
        }) => i.base10_parse::<i64>().map_err(|e| e.to_string()),
        syn::Expr::Unary(syn::ExprUnary {
            op: syn::UnOp::Neg(_),
            expr,
            ..
        }) => Ok(-const_int(expr)?),
        syn::Expr::Paren(p) => const_int(&p.expr),
        _ => err(
            e.span(),
            "only integer literals are supported as discriminants",
        ),
    }
}

/// ライフタイム引数だけなら許す (消去する)。型引数・const 引数はまだ未対応。
fn only_lifetimes(g: &syn::Generics) -> R<()> {
    for p in &g.params {
        if !matches!(p, syn::GenericParam::Lifetime(_)) {
            return err(p.span(), "generics are not supported yet (lifetimes only)");
        }
    }
    Ok(())
}

/// trait 境界 (`T: A + B` と where 節) を集める
type Bounds = syn::punctuated::Punctuated<syn::TypeParamBound, syn::Token![+]>;

fn bound_names(bs: &Bounds) -> R<Vec<String>> {
            let mut v = vec![];
            for b in bs {
                match b {
                    // `?Sized` は制約を緩めるだけなので数えない
                    syn::TypeParamBound::Trait(t) if matches!(t.modifier, syn::TraitBoundModifier::Maybe(_)) => {}
                    syn::TypeParamBound::Trait(t)
                        if matches!(
                            t.path.segments.last().map(|s| &s.arguments),
                            Some(syn::PathArguments::Parenthesized(_))
                        ) =>
                    {
                        v.push(bound_string(t))
                    }
                    // `Iterator<Item = u32>`: 関連型の制約つき (文字列のまま持ち、呼び出し側で解く)
                    syn::TypeParamBound::Trait(t)
                        if t.path.segments.len() == 1
                            && matches!(t.path.segments[0].arguments, syn::PathArguments::AngleBracketed(_)) =>
                    {
                        v.push(bound_string(t))
                    }
                    // `fmt::Display` などのパスは最後のセグメント
                    syn::TypeParamBound::Trait(t) => match t.path.segments.last() {
                        Some(seg) if seg.arguments.is_none() => v.push(seg.ident.to_string()),
                        _ => return err(t.span(), "unsupported trait bound"),
                    },
                    syn::TypeParamBound::Lifetime(_) => {}
                    _ => return err(b.span(), "unsupported bound"),
                }
            }
            Ok(v)
        }

fn collect_generics(g: &syn::Generics, out: &mut Vec<(String, Vec<String>)>) -> R<()> {
    for p in &g.params {
        match p {
            syn::GenericParam::Lifetime(_) => {}
            syn::GenericParam::Type(tp) => {
                out.push((tp.ident.to_string(), bound_names(&tp.bounds)?))
            }
            // `const N: usize`: 配列の長さのパラメータ (型の引数の位置に長さを持つ Ty::Array(Unit, n) を入れる)
            syn::GenericParam::Const(c) => out.push((c.ident.to_string(), vec![CONST_GENERIC.to_string()])),
        }
    }
    if let Some(w) = &g.where_clause {
        for pr in &w.predicates {
            let syn::WherePredicate::Type(pt) = pr else {
                continue;
            };
            // (`where C::Item: Debug` / `where Self: Sized` など、型パラメータ以外への境界は検査に使わないので読み飛ばす)
            let syn::Type::Path(tp) = &pt.bounded_ty else {
                continue;
            };
            let Some(id) = tp.path.get_ident() else {
                continue;
            };
            let bs = bound_names(&pt.bounds)?;
            match out.iter_mut().find(|g| *id == g.0) {
                Some(g) => g.1.extend(bs),
                None if id == "Self" => {}
                None => return err(pt.span(), "where clause on an unknown type parameter"),
            }
        }
    }
    Ok(())
}

/// const ジェネリクスの引数 `4` / `{ 4 }` / `N`。値は長さを持つ配列型 `[(); n]` で表す
pub fn const_arg_syn(e: &syn::Expr) -> R<TySyn> {
    match e {
        syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(i), .. }) => {
            let n = i.base10_parse::<u64>().or_else(|er| err(i.span(), er))?;
            Ok(TySyn::Array(Box::new(TySyn::Prim(Ty::Unit)), ArrLen::Lit(n)))
        }
        syn::Expr::Block(b) if b.block.stmts.len() == 1 => match &b.block.stmts[0] {
            syn::Stmt::Expr(e, None) => const_arg_syn(e),
            _ => err(e.span(), "unsupported const argument"),
        },
        syn::Expr::Path(p) if p.path.get_ident().is_some() => {
            Ok(TySyn::Array(Box::new(TySyn::Prim(Ty::Unit)), ArrLen::Const(p.path.get_ident().unwrap().to_string())))
        }
        _ => err(e.span(), "const arguments must be a literal or a name"),
    }
}

/// 型の環境で name が const ジェネリクスの引数 (`[(); n]`) なら、その値
pub fn const_generic_value(env: &TyEnv, name: &str) -> Option<u32> {
    env.params.iter().find(|(n, _)| n == name).and_then(|(_, t)| match t {
        Ty::Array(Ty::Unit, n) if *n < CONST_LEN_BASE => Some(*n),
        _ => None,
    })
}



fn parse_sig(
    sig: &syn::Signature,
    is_extern: bool,
    is_main: bool,
    owner: Option<&Owner>,
) -> R<SigSyn> {
    let mut generics = owner.map(|o| o.generics.clone()).unwrap_or_default();
    collect_generics(&sig.generics, &mut generics)?;
    if is_extern && !generics.is_empty() {
        return err(sig.generics.span(), "extern fns cannot be generic");
    }
    if is_main && !generics.is_empty() {
        return err(sig.generics.span(), "`main` cannot be generic");
    }
    if sig.variadic.is_some() {
        return err(sig.span(), "variadic functions are not supported");
    }
    let self_ty = owner.map(|o| &o.self_ty);
    let mut params = vec![];
    let mut has_self = false;
    for a in &sig.inputs {
        match a {
            syn::FnArg::Typed(pt) => {
                // 引数位置の `impl Trait` は名前のない型パラメータ
                let (inner, rf) = match &*pt.ty {
                    syn::Type::Reference(r) => (&*r.elem, Some(r.mutability.is_some())),
                    t => (t, None),
                };
                if let syn::Type::ImplTrait(it) = inner {
                    let g = format!("__impl{}", generics.len());
                    generics.push((g.clone(), bound_names(&it.bounds)?));
                    let t = TySyn::Named(g);
                    params.push(match rf {
                        Some(m) => TySyn::Ref(m, Box::new(t)),
                        None => t,
                    });
                } else {
                    params.push(parse_tysyn(&pt.ty, self_ty)?)
                }
            }
            // syn 2 では `self` / `&self` / `&mut self` / `self: &Self` すべて ty に完全な型が入る
            syn::FnArg::Receiver(r) => {
                if owner.is_none() {
                    return err(r.span(), "`self` is only allowed in impls");
                }
                has_self = true;
                params.push(parse_tysyn(&r.ty, self_ty)?);
            }
        }
    }
    let ret = match &sig.output {
        syn::ReturnType::Default => TySyn::Prim(Ty::Unit),
        // 戻り値位置の `impl Trait` は本体を見て summarize が具体型に置き換える
        syn::ReturnType::Type(_, t) if matches!(&**t, syn::Type::ImplTrait(_)) => {
            TySyn::Named(IMPL_RET.into())
        }
        syn::ReturnType::Type(_, t) => parse_tysyn(t, self_ty)?,
    };
    let name = sig.ident.to_string();
    let symbol = match owner {
        _ if is_extern || is_main => name,
        // ローカルシンボル。短く保つことで .o / シンボル表を小さくする
        Some(o) => match o.impl_trait.as_deref() {
            Some(t @ ("Display" | "Debug")) => format!("_x{}.{t}.{name}", o.base),
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
        params,
        ret,
        symbol,
        is_extern,
        is_main,
        impl_of: owner.filter(|o| !o.is_trait).map(|o| o.base.clone()),
        has_self,
        generics,
        trait_of: owner.filter(|o| o.is_trait).map(|o| o.base.clone()),
        has_body: true,
        impl_trait: owner.and_then(|o| o.impl_trait.clone()),
        self_ty: owner.filter(|_| !is_trait).map(|o| o.self_ty.clone()),
        export: false,
        panic_handler: false,
    })
}


/// 本体の直下の `let n: T = ..` / `let mut n = 0;` から、ローカル変数 n の型 (注釈か、リテラル・String の初期化式)
fn local_ty_syn(b: &syn::Block, n: &str, int_hint: Option<Ty>) -> Option<TySyn> {
    let mut found = None;
    for st in &b.stmts {
        let syn::Stmt::Local(l) = st else { continue };
        let (pat, ann) = match &l.pat {
            syn::Pat::Type(pt) => (&*pt.pat, Some(&*pt.ty)),
            p => (p, None),
        };
        let syn::Pat::Ident(pi) = pat else { continue };
        if pi.ident != n {
            continue;
        }
        if let Some(t) = ann {
            found = parse_tysyn(t, None).ok();
            continue;
        }
        let init = l.init.as_ref().map(|i| &*i.expr);
        found = match init {
            Some(syn::Expr::Lit(el)) => match &el.lit {
                syn::Lit::Int(i) => Some(TySyn::Prim(Ty::from_suffix(i.suffix()).or(int_hint).unwrap_or(Ty::I32))),
                syn::Lit::Float(x) => Some(TySyn::Prim(if x.suffix() == "f32" { Ty::F32 } else { Ty::F64 })),
                syn::Lit::Bool(_) => Some(TySyn::Prim(Ty::Bool)),
                syn::Lit::Char(_) => Some(TySyn::Prim(Ty::Char)),
                syn::Lit::Str(_) => Some(TySyn::Ref(false, Box::new(TySyn::Str))),
                _ => None,
            },
            // `String::new()` / `String::from(..)` / `"..".to_string()`
            Some(syn::Expr::Call(c)) => match &*c.func {
                syn::Expr::Path(p) if p.path.segments.len() == 2 && p.path.segments[0].ident == "String" => {
                    Some(TySyn::Named("String".into()))
                }
                _ => None,
            },
            Some(syn::Expr::MethodCall(mc)) if mc.method == "to_string" => Some(TySyn::Named("String".into())),
            _ => None,
        };
    }
    found
}

/// syn のパスを (program.rs の検索に渡す) PPath にする
pub fn ppath(p: &syn::Path) -> PPath {
    PPath {
        global: p.leading_colon.is_some(),
        segs: p.segments.iter().map(|s| PSeg { name: s.ident.to_string(), args: !s.arguments.is_none() }).collect(),
    }
}
