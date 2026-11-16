//! 関数 1 個分の型推論 (union-find による単一化)。
//!
//! `let mut i = 0; while i < n { .. }` や `let mut i = 0; bump(&mut i)` のように、
//! 注釈のない let の型を後続の使用箇所から決めるためのパス。型エラーの報告はここでは
//! 行わず (矛盾は無視)、codegen 側の検査に任せる。結果は「注釈なし let → 型」の表。

use syn::{BinOp, Expr, Lit, Pat, Stmt, UnOp};

use crate::front::{FnSig, Program, Ty, TyEnv, intern};
pub use crate::solver::*;

/// 注釈なし let と、値を作る式の型 (ノードのアドレス → 型)
pub type LetTypes = crate::fxhash::FxMap<usize, Ty>;

/// 整数 / &str / 生ポインタの組み込みメソッド名 (同名の struct のメソッドがある場合でも、
/// 受け手が struct でなければこちら)
fn is_builtin_scalar_method(n: &str) -> bool {
    matches!(
        n,
        "wrapping_add"
            | "wrapping_sub"
            | "wrapping_mul"
            | "wrapping_neg"
            | "checked_add"
            | "checked_sub"
            | "checked_mul"
            | "checked_div"
            | "checked_rem"
            | "checked_pow"
            | "wrapping_pow"
            | "saturating_pow"
            | "overflowing_pow"
            | "overflowing_add"
            | "overflowing_sub"
            | "overflowing_mul"
            | "rem_euclid"
            | "div_euclid"
            | "next_multiple_of"
            | "next_power_of_two"
            | "isqrt"
            | "try_into"
            | "checked_abs"
            | "checked_neg"
            | "leading_ones"
            | "trailing_ones"
            | "saturating_add"
            | "saturating_sub"
            | "abs"
            | "pow"
            | "count_ones"
            | "count_zeros"
            | "leading_zeros"
            | "trailing_zeros"
            | "is_power_of_two"
            | "rotate_left"
            | "rotate_right"
            | "swap_bytes"
            | "signum"
            | "is_positive"
            | "is_negative"
            | "abs_diff"
            | "is_ascii_digit"
            | "is_ascii_alphabetic"
            | "is_ascii_uppercase"
            | "is_ascii_lowercase"
            | "to_ascii_uppercase"
            | "to_ascii_lowercase"
            | "is_ascii_whitespace"
            | "is_ascii_alphanumeric"
            | "is_ascii_hexdigit"
            | "is_ascii_punctuation"
            | "is_ascii_graphic"
            | "is_ascii_control"
            | "is_ascii"
            | "to_le_bytes"
            | "to_be_bytes"
            | "to_ne_bytes"
            | "as_bytes"
            | "as_ptr"
            | "is_null"
            | "read_volatile"
            | "write_volatile"
            | "offset"
            | "wrapping_offset"
            | "read_unaligned"
            | "write_unaligned"
    )
}

/// `x.f` / `x.0` のフィールド名
fn member_name(m: &syn::Member) -> String {
    match m {
        syn::Member::Named(id) => id.to_string(),
        syn::Member::Unnamed(i) => i.index.to_string(),
    }
}

pub fn let_key(l: &syn::Local) -> usize {
    l as *const syn::Local as usize
}


struct Inf<'p> {
    s: Solver<'p>,
    pending: Vec<(*const syn::ExprClosure, Vec<u32>, u32)>,
    /// parse し直したマクロの引数。後回しにしたクロージャ (pending) が指すので推論の終わりまで持っておく
    keep: Vec<Vec<Expr>>,
    /// 次の apply_recv に渡すメソッド呼び出しの turbofish
    turbofish: Option<*const syn::AngleBracketedGenericArguments>,
    /// (根の関数, 最初の番号, 本体): クロージャに出会ったら clo_ctx を作る
    clo_root: Option<(u32, u32, *const syn::Block)>,
}

impl<'p> std::ops::Deref for Inf<'p> {
    type Target = Solver<'p>;
    fn deref(&self) -> &Solver<'p> {
        &self.s
    }
}

impl<'p> std::ops::DerefMut for Inf<'p> {
    fn deref_mut(&mut self) -> &mut Solver<'p> {
        &mut self.s
    }
}

/// sig はこの関数 (インスタンスなら型引数を入れた後) のシグネチャ、env は `Self` と型パラメータ
pub fn infer(prog: &Program, f: &syn::ItemFn, sig: &FnSig, env: &TyEnv) -> LetTypes {
    infer_full(prog, f, sig, env, &[], Some(sig.ret)).0
}

/// 推論結果: (let と式の型, 戻り値型, 引数の型)
pub type Full = (LetTypes, Option<Ty>, Vec<Option<Ty>>);

/// extra は最初からスコープにある変数 (クロージャのキャプチャ)。ret が None なら戻り値型も推論して返す。
/// 型パラメータを含む引数の型は未知として扱い、推論した型を返す (注釈のないクロージャ引数)
pub fn infer_full(
    prog: &Program,
    f: &syn::ItemFn,
    sig: &FnSig,
    env: &TyEnv,
    extra: &[(String, Ty)],
    ret: Option<Ty>,
) -> Full {
    infer_ctx(prog, f, sig, env, extra, ret, None)
}

/// root = (根の関数, f の本体の最初のクロージャの前順の番号): 本体のクロージャの型 (struct) まで解く
pub fn infer_ctx(
    prog: &Program,
    f: &syn::ItemFn,
    sig: &FnSig,
    env: &TyEnv,
    extra: &[(String, Ty)],
    ret: Option<Ty>,
    root: Option<(u32, u32)>,
) -> Full {
    let clo_root = root.map(|(r, base)| (r, base, &*f.block as *const syn::Block));
    let mut s = Inf {
        s: Solver::new(prog, env),
        pending: vec![],
        keep: vec![],
        turbofish: None,
        clo_root,
    };
    s.ret = match ret {
        // 一部だけ分かっている型 (`Result<?, u8>`): 型パラメータの所を型変数にする
        Some(t) if prog.has_param(t) => s.known_g(t, &mut vec![]),
        Some(t) => s.known(t),
        None => s.var(),
    };
    for (n, t) in extra {
        let v = s.known(*t);
        s.scopes[0].push((n.clone(), v));
    }
    let mut pvars = vec![];
    for (a, &t) in f.sig.inputs.iter().zip(&sig.params) {
        let name = match a {
            syn::FnArg::Receiver(_) => "self".to_string(),
            syn::FnArg::Typed(pt) => match &*pt.pat {
                Pat::Ident(pi) => pi.ident.to_string(),
                _ => {
                    pvars.push(s.var());
                    continue;
                }
            },
        };
        let v = if prog.has_param(t) { s.var() } else { s.known(t) };
        pvars.push(v);
        s.scopes[0].push((name, v));
    }
    let tail = s.block(&f.block);
    let ret = s.ret;
    s.unify(tail, ret);
    s.finish_late_bounds();
    s.finish_collects();
    s.finish_assocs();
    let lets = std::mem::take(&mut s.lets);
    let r = s.resolve(ret);
    let ps = pvars.into_iter().map(|v| s.resolve(v)).collect();
    (
        lets.into_iter()
            .filter_map(|(k, v)| Some((k, s.resolve(v)?)))
            .collect(),
        r,
        ps,
    )
}

impl Inf<'_> {
    /// `None::<u32>` / `Option::<u32>::None`: 型引数を書いたバリアントの型
    fn variant_turbofish(&mut self, p: &syn::ExprPath, sid: u32) -> Option<Ty> {
        let segs = &p.path.segments;
        let args = segs.iter().rev().find_map(|s| match &s.arguments {
            syn::PathArguments::AngleBracketed(ab) => Some(ab),
            _ => None,
        })?;
        let mut ts = vec![];
        for a in &args.args {
            if let syn::GenericArgument::Type(t) = a {
                ts.push(self.prog.parse_ty(t, self.env).ok()?);
            }
        }
        let base = self.prog.base_of(sid);
        self.prog.instantiate(base, ts).ok().map(Ty::Struct)
    }

    /// prelude の helper `f(recv, args..)` の戻り値 (turbofish は型引数の前から)
    fn apply_helper(&mut self, id: u32, mc: &syn::ExprMethodCall, args: Vec<u32>) -> u32 {
        let sig = &self.prog.sigs[id as usize];
        let (params, ret, ng) = (sig.params[1.min(sig.params.len())..].to_vec(), sig.ret, sig.generics.len());
        let mut pv: Vec<Option<u32>> = vec![None; ng];
        if let Some(tf) = &mc.turbofish {
            for (i, a) in tf.args.iter().enumerate().take(ng) {
                if let syn::GenericArgument::Type(t) = a {
                    if let Ok(t) = self.prog.parse_ty(t, self.env) {
                        pv[i] = Some(self.known(t));
                    }
                }
            }
        }
        for (a, p) in args.into_iter().zip(params) {
            // (char を渡す &str の引数は推論では見ない)
            if p == Ty::Ref(false, intern(Ty::Str)) {
                continue;
            }
            let k = self.known_g(p, &mut pv);
            self.unify(a, k);
        }
        self.known_g(ret, &mut pv)
    }

    /// 式が (参照越しの) 配列 / スライスなら Some。受け手の型を見るためだけに先に推論する
    fn expr_peek_elem(&mut self, e: &Expr) -> Option<u32> {
        // 受け手は後でもう一度 expr() されるが、同じ式なので型変数は単一化されて一致する
        let v = self.expr(e);
        // (Vec は prelude のメソッドを持つので、配列・スライスの組み込みメソッドには回さない)
        let d = self.deref_expr(v);
        let r = self.find(d);
        if !matches!(self.shape[r as usize], Some(Shape::Arr(..)) | Some(Shape::Slc(_))) {
            return None;
        }
        self.elem_of(v)
    }

    fn apply_recv(&mut self, callee: Option<u32>, args: Vec<u32>, recv: u32) -> u32 {
        let Some(id) = callee else { return self.var() };
        let sig = &self.prog.sigs[id as usize];
        let (params, ret, self_ty, is_trait) = (
            sig.params[1.min(sig.params.len())..].to_vec(),
            sig.ret,
            sig.self_ty,
            sig.trait_of.is_some(),
        );
        let mut pv: Vec<Option<u32>> = vec![None; sig.generics.len()];
        if is_trait {
            pv[0] = Some(recv);
        } else if let Some(st) = self_ty {
            let k = self.known_g(st, &mut pv);
            self.unify(k, recv);
        }
        for (a, p) in args.into_iter().zip(params) {
            let k = self.known_g(p, &mut pv);
            self.unify(a, k);
        }
        // turbofish (`collect::<Vec<_>>()`): 後ろの型パラメータ (メソッド自身のもの) から
        if let Some(tf) = self.turbofish.take() {
            // SAFETY: tf は infer() の間生きている関数の AST の中を指す
            let tf = unsafe { &*tf };
            let tys: Vec<&syn::Type> = tf
                .args
                .iter()
                .filter_map(|a| if let syn::GenericArgument::Type(t) = a { Some(t) } else { None })
                .collect();
            let ng = pv.len();
            if tys.len() <= ng {
                for (k, t) in tys.into_iter().enumerate() {
                    let v = self.syn_ty_var(t);
                    let i = ng - tf.args.len() + k;
                    match pv[i] {
                        Some(old) => self.unify(old, v),
                        None => pv[i] = Some(v),
                    }
                }
            }
        }
        let prog = self.prog;
        self.unify_fn_bounds(&prog.sigs[id as usize].generics, &mut pv);
        self.unify_assoc_bounds(&prog.sigs[id as usize].generics, &mut pv);
        let r = self.known_g(ret, &mut pv);
        self.late_bounds(id, pv);
        r
    }

    /// 書式マクロの引数 (`println!("{}", (1..4).map(..))`) も推論する。parse し直したノードは捨てるので、
    /// 型はソース上の位置で記録する (codegen も同じ位置のノードを parse して引く)
    fn fmt_args(&mut self, m: &syn::Macro) {
        use syn::parse::Parser;
        let Ok(args) = syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated.parse2(m.tokens.clone()) else {
            return;
        };
        let xs: Vec<Expr> = args
            .into_iter()
            .filter(|a| !matches!(a, Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(_), .. })))
            .map(|a| match a {
                // 名前つき引数 `w = expr`
                Expr::Assign(asg) => *asg.right,
                a => a,
            })
            .collect();
        if xs.is_empty() {
            return;
        }
        self.no_record += 1;
        self.pos_record += 1;
        let ki = self.keep.len();
        self.keep.push(xs);
        for i in 0..self.keep[ki].len() {
            let x: *const Expr = &self.keep[ki][i];
            self.expr(unsafe { &*x });
        }
        self.pos_record -= 1;
        self.no_record -= 1;
    }

    /// 書式文字列の幅・精度に名前で使う変数 (`{:w$}` / `{:.p$}`) は usize
    fn fmt_width_vars(&mut self, m: &syn::Macro) {
        self.fmt_args(m);
        let Some(lit) = m.tokens.clone().into_iter().find_map(|t| match t {
            proc_macro2::TokenTree::Literal(l) => syn::parse_str::<syn::LitStr>(&l.to_string()).ok(),
            _ => None,
        }) else {
            return;
        };
        let s = lit.value();
        if !s.contains('$') {
            return;
        }
        let b = s.as_bytes();
        for (i, _) in s.match_indices('$') {
            let mut k = i;
            while k > 0 && (b[k - 1].is_ascii_alphanumeric() || b[k - 1] == b'_') {
                k -= 1;
            }
            let name = &s[k..i];
            if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
                continue;
            }
            if let Some(v) = self.lookup(name) {
                let u = self.known(Ty::Usize);
                self.unify(v, u);
            }
        }
    }

    /// 書かれた型の型変数 (`Vec<_>` の `_` は新しい型変数)
    fn syn_ty_var(&mut self, t: &syn::Type) -> u32 {
        if let Ok(k) = self.prog.parse_ty(t, self.env) {
            return self.known(k);
        }
        match t {
            syn::Type::Infer(_) => self.var(),
            syn::Type::Reference(r) => {
                let i = self.syn_ty_var(&r.elem);
                self.with(Shape::Ref(r.mutability.is_some(), i))
            }
            syn::Type::Path(p) if p.qself.is_none() => {
                let last = p.path.segments.last().unwrap();
                let base = self.prog.struct_index.get(&last.ident.to_string()).copied();
                match (base, &last.arguments) {
                    (Some(b), syn::PathArguments::AngleBracketed(ab)) => {
                        let args: Vec<u32> = ab
                            .args
                            .iter()
                            .filter_map(|a| if let syn::GenericArgument::Type(t) = a { Some(t) } else { None })
                            .map(|t| self.syn_ty_var(t))
                            .collect();
                        self.adt(b, args)
                    }
                    _ => self.var(),
                }
            }
            syn::Type::Tuple(tt) => {
                let args: Vec<u32> = tt.elems.iter().map(|t| self.syn_ty_var(t)).collect();
                let tb = self.prog.tuple_base(args.len());
                self.adt(tb, args)
            }
            _ => self.var(),
        }
    }

    fn lit_var(&mut self, l: &Lit) -> u32 {
        match l {
            Lit::Int(i) if matches!(i.suffix(), "f32" | "f64") => {
                self.known(if i.suffix() == "f32" { Ty::F32 } else { Ty::F64 })
            }
            Lit::Float(f) => match f.suffix() {
                "f32" => self.known(Ty::F32),
                "f64" => self.known(Ty::F64),
                _ => {
                    let v = self.var();
                    self.float_lit[v as usize] = true;
                    v
                }
            },
            Lit::Int(i) => match Ty::from_suffix(i.suffix()) {
                Some(t) => self.known(t),
                None => {
                    let v = self.var();
                    self.int_lit[v as usize] = true;
                    v
                }
            },
            Lit::Bool(_) => self.known(Ty::Bool),
            Lit::Byte(_) => self.known(Ty::U8),
            Lit::Char(_) => self.known(Ty::Char),
            Lit::Str(_) => self.known(Ty::Ref(false, intern(Ty::Str))),
            Lit::ByteStr(b) => self.known(Ty::Ref(
                false,
                intern(Ty::Array(intern(Ty::U8), b.value().len() as u32)),
            )),
            _ => self.var(),
        }
    }

    fn closure_body(&mut self, c: &syn::ExprClosure, vs: &[u32], rv: u32) {
        self.scopes.push(vec![]);
        for (p, &v) in c.inputs.iter().zip(vs) {
            self.pat(p, v, None);
        }
        let saved = self.ret;
        self.ret = rv;
        let b = self.expr(&c.body);
        self.unify(b, rv);
        self.ret = saved;
        self.scopes.pop();
    }

    fn finish_closures(&mut self, mark: usize) {
        while self.pending.len() > mark {
            let (c, vs, rv) = self.pending.remove(mark);
            let saved = std::mem::take(&mut self.defer);
            // SAFETY: c は infer() の間生きている関数の AST の中を指す
            let c = unsafe { &*c };
            self.closure_body(c, &vs, rv);
            self.defer = saved;
        }
    }

    /// パターンを型変数 v に当て、束縛を今のスコープに入れる
    fn pat(&mut self, pat: &Pat, v: u32, mode: Option<bool>) {
        match pat {
            Pat::Ident(pi) if crate::codegen::unit_variant_ident(self.prog, pi).is_some() => {
                let path = crate::codegen::unit_variant_ident(self.prog, pi).unwrap();
                let (v, _) = self.auto_deref(v, mode);
                if let Some((sid, vi)) = self.prog.variant_by_path(&path, self.self_sid) {
                    self.pat_fields(v, sid, Some(vi));
                }
            }
            Pat::Ident(pi) if crate::codegen::const_lit_pat(self.prog, pi).is_some() => {
                let lp = crate::codegen::const_lit_pat(self.prog, pi).unwrap();
                self.pat(&lp, v, mode);
            }
            Pat::Ident(pi) => {
                if let Some((_, sub)) = &pi.subpat {
                    self.pat(sub, v, mode);
                }
                let mode = if pi.by_ref.is_some() {
                    Some(pi.mutability.is_some())
                } else {
                    mode
                };
                self.bind_name(pi.ident.to_string(), v, mode);
            }
            Pat::Paren(p) => self.pat(&p.pat, v, mode),
            Pat::Type(p) => self.pat(&p.pat, v, mode),
            Pat::Reference(r) => {
                let i = self.deref(v);
                self.pat(&r.pat, i, None);
            }
            Pat::Or(o) => {
                for c in &o.cases {
                    self.pat(c, v, mode);
                }
            }
            Pat::Lit(l) => {
                let (v, _) = self.auto_deref(v, mode);
                let e = self.lit_var(&l.lit);
                self.unify(v, e);
            }
            Pat::Range(r) => {
                let (v, _) = self.auto_deref(v, mode);
                for e in r.start.iter().chain(r.end.iter()) {
                    let e = self.expr(e);
                    self.unify(v, e);
                }
            }
            Pat::Path(pp) => {
                let (v, _) = self.auto_deref(v, mode);
                if let Some((sid, vi)) = self.prog.variant_by_path(&pp.path, self.self_sid) {
                    self.pat_fields(v, sid, Some(vi));
                }
            }
            Pat::TupleStruct(ts) => {
                let (v, mode) = self.auto_deref(v, mode);
                let fts: Vec<u32> =
                    if let Some((sid, vi)) = self.prog.variant_by_path(&ts.path, self.self_sid) {
                        self.pat_fields(v, sid, Some(vi))
                            .into_iter()
                            .map(|f| f.1)
                            .collect()
                    } else if let Some(sid) = self.prog.struct_by_path(&ts.path, self.self_sid) {
                        self.pat_fields(v, sid, None)
                            .into_iter()
                            .map(|f| f.1)
                            .collect()
                    } else {
                        return;
                    };
                let elems: Vec<&Pat> = ts.elems.iter().collect();
                let rest = elems.iter().position(|p| matches!(p, Pat::Rest(_)));
                for (i, p) in elems.iter().enumerate() {
                    let fi = match rest {
                        Some(r) if i == r => continue,
                        Some(r) if i > r => fts.len().wrapping_sub(elems.len() - i),
                        _ => i,
                    };
                    if let Some(&ft) = fts.get(fi) {
                        self.pat(p, ft, mode);
                    }
                }
            }
            Pat::Tuple(pt) => {
                let (v, mode) = self.auto_deref(v, mode);
                let base = self.prog.tuple_base(pt.elems.len());
                let fts: Vec<u32> = self
                    .pat_fields(v, base, None)
                    .into_iter()
                    .map(|f| f.1)
                    .collect();
                let elems: Vec<&Pat> = pt.elems.iter().collect();
                let rest = elems.iter().position(|p| matches!(p, Pat::Rest(_)));
                for (i, p) in elems.iter().enumerate() {
                    let fi = match rest {
                        Some(r) if i == r => continue,
                        Some(r) if i > r => fts.len().wrapping_sub(elems.len() - i),
                        _ => i,
                    };
                    if let Some(&ft) = fts.get(fi) {
                        self.pat(p, ft, mode);
                    }
                }
            }
            Pat::Struct(ps) => {
                let (v, mode) = self.auto_deref(v, mode);
                let fields =
                    if let Some((sid, vi)) = self.prog.variant_by_path(&ps.path, self.self_sid) {
                        self.pat_fields(v, sid, Some(vi))
                    } else if let Some(sid) = self.prog.struct_by_path(&ps.path, self.self_sid) {
                        self.pat_fields(v, sid, None)
                    } else {
                        return;
                    };
                for fp in &ps.fields {
                    let name = member_name(&fp.member);
                    if let Some(&(_, ft)) = fields.iter().find(|f| f.0 == name) {
                        self.pat(&fp.pat, ft, mode);
                    }
                }
            }
            _ => {}
        }
    }

    fn block(&mut self, b: &syn::Block) -> u32 {
        self.scopes.push(vec![]);
        let mut res = None;
        let n = b.stmts.len();
        for (i, st) in b.stmts.iter().enumerate() {
            match st {
                Stmt::Local(l) => {
                    let (pat, ann) = match &l.pat {
                        Pat::Type(pt) => (&*pt.pat, self.prog.parse_ty(&pt.ty, self.env).ok()),
                        p => (p, None),
                    };
                    let v = match &l.init {
                        Some(init) => self.expr(&init.expr),
                        None => self.var(),
                    };
                    if let Some(t) = ann {
                        let k = self.known(t);
                        self.unify(v, k);
                    }
                    if self.no_record == 0 {
                        self.lets.push((let_key(l), v));
                    }
                    match pat {
                        Pat::Ident(pi) if pi.subpat.is_none() => self
                            .scopes
                            .last_mut()
                            .unwrap()
                            .push((pi.ident.to_string(), v)),
                        p => self.pat(p, v, None),
                    }
                }
                Stmt::Expr(e, semi) => {
                    let v = self.expr(e);
                    if i + 1 == n && semi.is_none() {
                        res = Some(v);
                    }
                    // `{ return None; }` は発散する (型は周りから決まる)
                    if i + 1 == n && semi.is_some() && diverges(e) {
                        res = Some(self.var());
                    }
                }
                Stmt::Macro(m) if i + 1 == n && diverging_macro(&m.mac) => res = Some(self.var()),
                Stmt::Macro(m) if is_fmt_macro(&m.mac) => self.fmt_width_vars(&m.mac),
                _ => {}
            }
        }
        self.scopes.pop();
        res.unwrap_or_else(|| self.known(Ty::Unit))
    }

    /// 式の型変数。値を作る式 (リテラル・構築・呼び出し) は結果の型も記録し、
    /// codegen が期待型のないところで使う (`Pair { a: 1 }.swap()` の 1 を後ろの使用から決める)
    fn expr(&mut self, e: &Expr) -> u32 {
        // 呼び出しの引数のクロージャは、呼び先の境界 (`F: FnMut(Self::Item) -> B`) と単一化してから本体を推論する
        let call = matches!(e, Expr::Call(_) | Expr::MethodCall(_));
        let mark = self.pending.len();
        if call {
            self.defer += 1;
        }
        // ループとラベル付きブロック: break の値の型を集める
        let lname = |l: &Option<syn::Label>| l.as_ref().map(|l| l.name.ident.to_string());
        let frame = match e {
            Expr::Loop(l) => Some((lname(&l.label), false)),
            Expr::While(w) => Some((lname(&w.label), false)),
            Expr::ForLoop(f) => Some((lname(&f.label), false)),
            Expr::Block(b) if b.label.is_some() => Some((lname(&b.label), true)),
            _ => None,
        };
        let v = match frame {
            None => self.expr_inner(e),
            Some((label, is_block)) => {
                let rv = self.var();
                self.brk.push((label, rv, is_block, false));
                let v = self.expr_inner(e);
                let (_, rv, _, broke) = self.brk.pop().unwrap();
                match e {
                    // break のない loop は !
                    Expr::Loop(_) if !broke => self.known(Ty::Never),
                    Expr::Loop(_) => rv,
                    Expr::Block(_) if broke => {
                        if self.resolve(v) != Some(Ty::Never) {
                            self.unify(rv, v);
                        }
                        rv
                    }
                    _ => v,
                }
            }
        };
        if call {
            self.defer -= 1;
            self.finish_closures(mark);
        }
        if matches!(
            e,
            Expr::Lit(_)
                | Expr::Struct(_)
                | Expr::Call(_)
                | Expr::MethodCall(_)
                | Expr::Path(_)
                | Expr::Macro(_)
                | Expr::Try(_)
                | Expr::Index(_)
                | Expr::Field(_)
        ) && self.no_record == 0
        {
            self.lets.push((e as *const Expr as usize, v));
            // (メソッド呼び出しはノード自体でも引けるようにする: codegen の helper が戻り値の型を見る)
            if let Expr::MethodCall(mc) = e {
                self.lets.push((mc as *const syn::ExprMethodCall as usize, v));
            }
            // (関数呼び出しも: `Tr::dec(r)?` の Self を戻り値の型から決める)
            if let Expr::Call(c) = e {
                self.lets.push((c as *const syn::ExprCall as usize, v));
            }
        } else if self.pos_record > 0 && matches!(e, Expr::Lit(_) | Expr::Call(_) | Expr::MethodCall(_) | Expr::Path(_) | Expr::Struct(_) | Expr::Index(_) | Expr::Field(_)) {
            let k = pos_key(e);
            if k != 0 {
                self.lets.push((k, v));
            }
        }
        v
    }

    fn expr_inner(&mut self, e: &Expr) -> u32 {
        use BinOp::*;
        match e {
            // 値としての `a..b` / `a..=b`: prelude の Range<T> / RangeInclusive<T>
            Expr::Range(rg) => {
                let name = if matches!(rg.limits, syn::RangeLimits::Closed(_)) {
                    "RangeInclusive"
                } else if rg.end.is_none() {
                    "RangeFrom"
                } else {
                    "Range"
                };
                let el = self.var();
                for x in rg.start.iter().chain(rg.end.iter()) {
                    let xv = self.expr(x);
                    self.unify(el, xv);
                }
                match self.prog.struct_index.get(name) {
                    Some(&b) => self.adt(b, vec![el]),
                    None => self.var(),
                }
            }
            // `format!(..)`: String
            Expr::Macro(m) if m.mac.path.is_ident("format") && self.prog.struct_index.contains_key("String") => {
                self.fmt_width_vars(&m.mac);
                let ss = self.prog.struct_index["String"];
                self.known(Ty::Struct(ss))
            }
            // `dbg!(e)`: e の型
            Expr::Macro(m) if m.mac.path.is_ident("dbg") => {
                use syn::parse::Parser;
                let Ok(args) = syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated.parse2(m.mac.tokens.clone()) else {
                    return self.var();
                };
                let xs: Vec<Expr> = args.into_iter().collect();
                if xs.is_empty() {
                    return self.var();
                }
                self.no_record += 1;
                self.pos_record += 1;
                let ki = self.keep.len();
                self.keep.push(xs);
                let x: *const Expr = &self.keep[ki][0];
                let v = self.expr(unsafe { &*x });
                self.pos_record -= 1;
                self.no_record -= 1;
                v
            }
            // `matches!(e, pat)`: bool (e は codegen が parse し直すので位置で記録する)
            Expr::Macro(m) if m.mac.path.is_ident("matches") => {
                use syn::parse::Parser;
                let parser = |input: syn::parse::ParseStream| -> syn::Result<Expr> {
                    let e: Expr = input.parse()?;
                    let _: proc_macro2::TokenStream = input.parse()?;
                    Ok(e)
                };
                if let Ok(e) = parser.parse2(m.mac.tokens.clone()) {
                    let ki = self.keep.len();
                    self.keep.push(vec![e]);
                    let x: *const Expr = &self.keep[ki][0];
                    self.no_record += 1;
                    self.pos_record += 1;
                    self.expr(unsafe { &*x });
                    self.pos_record -= 1;
                    self.no_record -= 1;
                }
                self.known(Ty::Bool)
            }
            Expr::Macro(m) if is_fmt_macro(&m.mac) => {
                self.fmt_width_vars(&m.mac);
                // (`print!` 系は ())
                let unit = m.mac.path.segments.last().is_some_and(|s| matches!(s.ident.to_string().as_str(), "print" | "println" | "eprint" | "eprintln"));
                if unit { self.known(Ty::Unit) } else { self.var() }
            }
            // `vec![a, b]` / `vec![x; n]`: Vec<要素の型>
            Expr::Macro(m)
                if m.mac.path.is_ident("vec") && self.prog.struct_index.contains_key("Vec") =>
            {
                use syn::parse::Parser;
                let el = self.var();
                let rep = (|input: syn::parse::ParseStream| {
                    let x: Expr = input.parse()?;
                    let _: syn::Token![;] = input.parse()?;
                    let n: Expr = input.parse()?;
                    Ok((x, n))
                })
                .parse2(m.mac.tokens.clone());
                let xs: Vec<Expr> = match rep {
                    Ok((x, _)) => vec![x],
                    Err(_) => syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated
                        .parse2(m.mac.tokens.clone())
                        .map(|v| v.into_iter().collect())
                        .unwrap_or_default(),
                };
                // (parse し直したノードは推論の後で捨てるので、アドレスで型を記録しない。
                //  解放されたアドレスが codegen の作るノードと重なると別の式の型を引いてしまう)
                self.no_record += 1;
                let ki = self.keep.len();
                self.keep.push(xs);
                for i in 0..self.keep[ki].len() {
                    // (Vec を keep に移しても要素のアドレスは変わらない)
                    let x: *const Expr = &self.keep[ki][i];
                    let v = self.expr(unsafe { &*x });
                    self.unify(el, v);
                }
                self.no_record -= 1;
                let vb = self.prog.struct_index["Vec"];
                self.adt(vb, vec![el])
            }
            Expr::Lit(l) => self.lit_var(&l.lit),
            Expr::Paren(p) => self.expr(&p.expr),
            Expr::Group(g) => self.expr(&g.expr),
            // `e?`: Option<T> / Result<T, E> の T
            Expr::Try(t) => {
                let v = self.expr(&t.expr);
                let (v, _) = self.auto_deref(v, None);
                let r = self.find(v);
                let base = match self.shape[r as usize] {
                    Some(Shape::Known(Ty::Struct(sid))) => Some(self.prog.base_of(sid)),
                    Some(Shape::Adt(b, _, _)) => Some(b),
                    _ => None,
                };
                match base {
                    Some(b)
                        if matches!(
                            self.prog.structs[b as usize].name.as_str(),
                            "Option" | "Result"
                        ) =>
                    {
                        self.adt_view(v, b)[0]
                    }
                    _ => self.var(),
                }
            }
            Expr::Tuple(t) if t.elems.is_empty() => self.known(Ty::Unit),
            Expr::Tuple(t) => {
                let base = self.prog.tuple_base(t.elems.len());
                let args = t
                    .elems
                    .iter()
                    .enumerate()
                    .map(|(i, e)| (i.to_string(), self.expr(e)))
                    .collect();
                self.adt_ctor(base, None, args)
            }
            Expr::Array(a) => {
                let e = self.var();
                for x in &a.elems {
                    let v = self.expr(x);
                    self.unify(e, v);
                }
                self.with(Shape::Arr(e, a.elems.len() as u32))
            }
            Expr::Repeat(r) => {
                let e = self.expr(&r.expr);
                let n = match &*r.len {
                    Expr::Lit(syn::ExprLit {
                        lit: Lit::Int(i), ..
                    }) => i.base10_parse::<u32>().ok(),
                    _ => None,
                };
                match n {
                    Some(n) => self.with(Shape::Arr(e, n)),
                    None => self.var(),
                }
            }
            Expr::Index(ix) => {
                let b = self.expr(&ix.expr);
                if let Expr::Range(rg) = &*ix.index {
                    for x in rg.start.iter().chain(rg.end.iter()) {
                        let v = self.expr(x);
                        let k = self.known(Ty::Usize);
                        self.unify(v, k);
                    }
                    // (`s[a..b]` の s が str / String なら str)
                    let d = self.deref_expr(b);
                    let r = self.find(d);
                    let is_str = match self.shape[r as usize] {
                        Some(Shape::Known(Ty::Str)) => true,
                        Some(Shape::Known(Ty::Struct(sid))) => {
                            self.prog.structs[sid as usize].base.is_none() && self.prog.structs[sid as usize].name == "String"
                        }
                        _ => false,
                    };
                    if is_str {
                        return self.known(Ty::Str);
                    }
                    return match self.elem_of(b) {
                        Some(e) => self.with(Shape::Slc(e)),
                        None => self.var(),
                    };
                }
                // ユーザの `impl Index for S` (Vec などでない struct): index の戻り値の指す先
                let d = self.deref_expr(b);
                let r = self.find(d);
                if let Some(Shape::Known(Ty::Struct(sid))) = self.shape[r as usize] {
                    if let Some(id) = self.prog.method(sid, "index") {
                        if let Ty::Ref(_, out) = self.prog.sigs[id as usize].ret {
                            if !self.prog.has_param(*out) {
                                let i = self.expr(&ix.index);
                                if let Some(&pt) = self.prog.sigs[id as usize].params.get(1) {
                                    if !self.prog.has_param(pt) {
                                        let k = self.known(pt);
                                        self.unify(i, k);
                                    }
                                }
                                return self.known(*out);
                            }
                        }
                    }
                }
                let i = self.expr(&ix.index);
                let k = self.known(Ty::Usize);
                self.unify(i, k);
                self.elem_of(b).unwrap_or_else(|| self.var())
            }
            Expr::ForLoop(f) => {
                // アダプタを剥がして要素の型を決める
                let mut e: &Expr = &f.expr;
                let mut adapters = vec![];
                loop {
                    match e {
                        Expr::MethodCall(mc) if mc.args.is_empty() => {
                            adapters.push(mc.method.to_string());
                            e = &mc.receiver;
                        }
                        Expr::Paren(p) => e = &p.expr,
                        _ => break,
                    }
                }
                adapters.reverse();
                let mut iter_item = None;
                let item = if let Expr::Range(rg) = e {
                    let v = self.var();
                    for x in rg.start.iter().chain(rg.end.iter()) {
                        let xv = self.expr(x);
                        self.unify(v, xv);
                    }
                    v
                } else {
                    let (by_ref, base_e) = match e {
                        Expr::Reference(r) => (Some(r.mutability.is_some()), &*r.expr),
                        _ => (None, e),
                    };
                    let bv = self.expr(base_e);
                    let mut mode = by_ref;
                    if mode.is_none() {
                        let r = self.find(bv);
                        if let Some(Shape::Ref(m, _)) = self.shape[r as usize] {
                            mode = Some(m);
                        }
                    }
                    for a in &adapters {
                        match a.as_str() {
                            "iter" => mode = Some(false),
                            "iter_mut" => mode = Some(true),
                            "copied" | "cloned" => mode = None,
                            _ => {}
                        }
                    }
                    match self.elem_of(bv) {
                        Some(el) => match mode {
                            Some(m) => self.with(Shape::Ref(m, el)),
                            None => el,
                        },
                        None => {
                            // Iterator: 式全体の型の `Item`
                            // (型引数が未定のアダプタの列 `(0..n).step_by(2)` でも Item を型変数のまま辿る)
                            let mut fv = if adapters.is_empty() { bv } else { self.expr(&f.expr) };
                            // (ユーザの `impl IntoIterator for &Coll` / `for Coll`: into_iter が返すイテレータの Item)
                            let user_into = match self.resolve(fv) {
                                Some(Ty::Ref(_, Ty::Struct(sid))) => self.prog.method(*sid, "__ref_into_iter"),
                                // (`for x in &c`: c の型で `__ref_into_iter` を探す)
                                // (`for (k, v) in &map`: 無ければ iter / iter_mut (HashMap / BTreeMap / VecDeque など))
                                Some(Ty::Struct(sid)) if by_ref.is_some() => self.prog.method(sid, "__ref_into_iter").or_else(|| {
                                    self.prog.method(sid, if by_ref == Some(true) { "iter_mut" } else { "iter" })
                                }),
                                Some(Ty::Struct(sid)) if self.prog.method(sid, "next").is_none() => self.prog.method(sid, "into_iter"),
                                _ => None,
                            };
                            if let Some(id) = user_into {
                                fv = self.apply_recv(Some(id), vec![], fv);
                            }
                            let it = match self.assoc_var(fv, "Item") {
                                Some(v) => v,
                                None => match self.resolve(fv).and_then(|t| self.prog.assoc_of(t, "Item")) {
                                    Some(t) => self.known(t),
                                    None => self.var(),
                                },
                            };
                            iter_item = Some(it);
                            self.var()
                        }
                    }
                };
                let item = if let Some(it) = iter_item {
                    it
                } else if adapters.iter().any(|a| a == "enumerate") {
                    let base = self.prog.tuple_base(2);
                    let u = self.known(Ty::Usize);
                    self.adt(base, vec![u, item])
                } else {
                    item
                };
                self.scopes.push(vec![]);
                self.pat(&f.pat, item, None);
                self.block(&f.body);
                self.scopes.pop();
                self.known(Ty::Unit)
            }
            Expr::Block(b) => self.block(&b.block),
            Expr::Unsafe(u) => self.block(&u.block),
            Expr::Path(p)
                if p.path.segments.len() >= 2
                    || (p.path.segments.len() == 1
                        && p.qself.is_none()
                        && self.lookup(&p.path.segments[0].ident.to_string()).is_none()
                        && self.prog.variant_by_path(&p.path, self.self_sid).is_some()) =>
            {
                match self.prog.variant_by_path(&p.path, self.self_sid) {
                    Some((sid, vi)) => {
                        let v = self.adt_ctor(sid, Some(vi), vec![]);
                        // `None::<u32>`: 型引数
                        if let Some(t) = self.variant_turbofish(p, sid) {
                            let k = self.known(t);
                            self.unify(v, k);
                        }
                        v
                    }
                    None => {
                        let segs: Vec<String> = p
                            .path
                            .segments
                            .iter()
                            .map(|s| s.ident.to_string())
                            .collect();
                        match Ty::from_suffix(&segs[0]).or(if segs[0] == "char" {
                            Some(Ty::Char)
                        } else {
                            None
                        }) {
                            Some(_) if segs.len() == 2 && segs[1] == "BITS" => self.known(Ty::U32),
                            Some(t) if segs.len() == 2 => self.known(t),
                            _ => self.var(),
                        }
                    }
                }
            }
            Expr::Match(m) => {
                let sv = self.expr(&m.expr);
                let r = self.var();
                for arm in &m.arms {
                    self.scopes.push(vec![]);
                    self.pat(&arm.pat, sv, None);
                    if let Some((_, g)) = &arm.guard {
                        self.expr(g);
                    }
                    let t = self.expr(&arm.body);
                    self.unify(r, t);
                    self.scopes.pop();
                }
                r
            }
            Expr::Let(l) => {
                // if let / while let の条件。束縛は呼び出し側のスコープに入れる
                let sv = self.expr(&l.expr);
                self.pat(&l.pat, sv, None);
                self.known(Ty::Bool)
            }
            // const / static
            Expr::Path(p)
                if p.path.get_ident().is_some_and(|id| {
                    self.lookup(&id.to_string()).is_none()
                        && self.prog.const_index.contains_key(&id.to_string())
                }) =>
            {
                let ci = self.prog.const_index[&p.path.get_ident().unwrap().to_string()];
                { let __a0 = self.prog.consts[ci as usize].ty; self.known(__a0) }
            }
            Expr::Path(p) => match p
                .path
                .get_ident()
                .and_then(|id| self.lookup(&id.to_string()))
            {
                Some(v) => v,
                // const ジェネリクスの N は usize
                None if p.path.get_ident().is_some_and(|id| {
                    let n = id.to_string();
                    crate::front::const_generic_value(self.env, &n).is_some()
                }) => self.known(Ty::Usize),
                // 値として使われた (ジェネリックでない) 関数は関数ポインタ
                None => match self.prog.fn_by_path(&p.path, self.self_sid) {
                    Some(id)
                        if !self.prog.sigs[id as usize].is_generic()
                            && !self
                                .prog
                                .const_index
                                .contains_key(&p.path.segments.last().unwrap().ident.to_string()) =>
                    {
                        let s = &self.prog.sigs[id as usize];
                        let f = crate::front::intern_fn(crate::front::FnTy {
                            params: s.params.clone(),
                            ret: s.ret,
                        });
                        self.known(Ty::Fn(f))
                    }
                    _ => self.var(),
                },
            },
            Expr::Unary(u) => {
                let v = self.expr(&u.expr);
                match u.op {
                    UnOp::Deref(_) => self.deref(v),
                    _ => v,
                }
            }
            Expr::Reference(r) => {
                let v = self.expr(&r.expr);
                self.with(Shape::Ref(r.mutability.is_some(), v))
            }
            Expr::Field(f) => {
                let b = self.expr(&f.base);
                match &f.member {
                    syn::Member::Named(id) => self.field(b, &id.to_string()),
                    // タプル構造体・タプルのフィールドは "0", "1", .. という名前
                    syn::Member::Unnamed(i) => self.field(b, &i.index.to_string()),
                }
            }
            Expr::Struct(s) => {
                let args: Vec<(String, u32)> = s
                    .fields
                    .iter()
                    .map(|fv| (member_name(&fv.member), self.expr(&fv.expr)))
                    .collect();
                if let Some((sid, vi)) = self.prog.variant_by_path(&s.path, self.self_sid) {
                    self.adt_ctor(sid, Some(vi), args)
                } else if let Some(sid) = self.prog.struct_by_path(&s.path, self.self_sid) {
                    self.adt_ctor(sid, None, args)
                } else {
                    self.var()
                }
            }
            Expr::Binary(b) => {
                let l = self.expr(&b.left);
                let r = self.expr(&b.right);
                // `&i64 * 10` / `acc + x` (x: &i64): 参照は値として計算する (codegen と同じ)
                // (片方が struct かその参照なら演算子のオーバーロード: 参照を数値とみなして外さない)
                let is_adt = |this: &mut Self, v: u32| {
                    let mut t = this.resolve(v);
                    while let Some(Ty::Ref(_, i)) = t {
                        t = Some(*i);
                    }
                    matches!(t, Some(Ty::Struct(_)))
                };
                let adt = is_adt(self, l) || is_adt(self, r);
                let (l, r) = if adt || matches!(b.op, And(_) | Or(_) | Shl(_) | Shr(_) | ShlAssign(_) | ShrAssign(_)) {
                    (l, r)
                } else {
                    (self.num_operand(l), self.num_operand(r))
                };
                match b.op {
                    And(_) | Or(_) => {
                        let k = self.known(Ty::Bool);
                        self.unify(l, k);
                        self.unify(r, k);
                        k
                    }
                    Eq(_) | Ne(_) => {
                        // (`v == &a` / `&s[..] == v`: 片方だけ参照なら中身どうし。Vec と配列・スライスは要素どうし)
                        let (fl, fr) = (self.find(l), self.find(r));
                        match (self.shape[fl as usize], self.shape[fr as usize]) {
                            (Some(Shape::Ref(_, x)), Some(s)) if !matches!(s, Shape::Ref(..)) => self.seq_unify(x, r),
                            (Some(s), Some(Shape::Ref(_, y))) if !matches!(s, Shape::Ref(..)) => self.seq_unify(l, y),
                            // (`w == b"pub"` で w: &[u8]: 参照の中身どうし (スライスと配列は単一化しない))
                            (Some(Shape::Ref(_, x)), Some(Shape::Ref(_, y))) => {
                                let (fx, fy) = (self.find(x), self.find(y));
                                let seqish = |s: &Option<Shape>| matches!(s, Some(Shape::Slc(_) | Shape::Arr(..) | Shape::Known(Ty::Slice(_) | Ty::Array(..))));
                                if seqish(&self.shape[fx as usize]) && seqish(&self.shape[fy as usize]) {
                                    self.seq_unify(x, y)
                                } else {
                                    self.unify(l, r)
                                }
                            }
                            _ => self.seq_unify(l, r),
                        }
                        self.known(Ty::Bool)
                    }
                    Lt(_) | Le(_) | Gt(_) | Ge(_) => {
                        self.unify(l, r);
                        self.known(Ty::Bool)
                    }
                    Shl(_) | Shr(_) => l,
                    ShlAssign(_) | ShrAssign(_) => self.known(Ty::Unit),
                    AddAssign(_) | SubAssign(_) | MulAssign(_) | DivAssign(_) | RemAssign(_)
                    | BitXorAssign(_) | BitAndAssign(_) | BitOrAssign(_) => {
                        self.unify(l, r);
                        self.known(Ty::Unit)
                    }
                    _ => {
                        // 演算子のオーバーロード (`impl Add for V` / `impl Mul<i32> for V` / `impl Add for &M`):
                        // 戻り値は add などのメソッドの型
                        let m = match b.op {
                            Add(_) => "add",
                            Sub(_) => "sub",
                            Mul(_) => "mul",
                            Div(_) => "div",
                            Rem(_) => "rem",
                            BitAnd(_) => "bitand",
                            BitOr(_) => "bitor",
                            BitXor(_) => "bitxor",
                            _ => "",
                        };
                        let lt = self.resolve(l);
                        let found = match lt {
                            Some(Ty::Struct(sid)) if !m.is_empty() => self.prog.method(sid, m).map(|id| (id, l)),
                            Some(Ty::Ref(_, Ty::Struct(sid))) if !m.is_empty() => {
                                self.prog.method(*sid, &format!("__ref_{m}")).map(|id| (id, l))
                            }
                            // (左がまだ分からない `&b * m`: 右が `&M` で `impl Mul for &M` があればそれ)
                            None if !m.is_empty() => match self.resolve(r) {
                                Some(Ty::Ref(_, Ty::Struct(sid))) => self.prog.method(*sid, &format!("__ref_{m}")).map(|id| (id, l)),
                                _ => None,
                            },
                            _ => None,
                        };
                        if let Some((id, recv)) = found {
                            return self.apply_recv(Some(id), vec![r], recv);
                        }
                        self.unify(l, r);
                        l
                    }
                }
            }
            Expr::Assign(a) => {
                let l = self.expr(&a.left);
                let r = self.expr(&a.right);
                self.unify(l, r);
                self.known(Ty::Unit)
            }
            Expr::If(i) => {
                // if let の束縛は then 節だけで見える
                self.scopes.push(vec![]);
                self.expr(&i.cond);
                let t = self.block(&i.then_branch);
                self.scopes.pop();
                match &i.else_branch {
                    Some((_, e)) => {
                        let e = self.expr(e);
                        self.unify(t, e);
                        t
                    }
                    None => self.known(Ty::Unit),
                }
            }
            Expr::While(w) => {
                self.scopes.push(vec![]);
                self.expr(&w.cond);
                self.block(&w.body);
                self.scopes.pop();
                self.known(Ty::Unit)
            }
            Expr::Loop(l) => {
                self.block(&l.body);
                self.known(Ty::Unit)
            }
            Expr::Return(r) => {
                if let Some(e) = &r.expr {
                    let v = self.expr(e);
                    let ret = self.ret;
                    self.unify(v, ret);
                }
                self.known(Ty::Never)
            }
            Expr::Break(b) => {
                let label = b.label.as_ref().map(|l| l.ident.to_string());
                let fi = self.brk.iter().rposition(|f| match &label {
                    Some(n) => f.0.as_deref() == Some(n.as_str()),
                    None => !f.2,
                });
                let v = match &b.expr {
                    Some(e) => Some(self.expr(e)),
                    None => None,
                };
                if let Some(fi) = fi {
                    self.brk[fi].3 = true;
                    let rv = self.brk[fi].1;
                    let v = v.unwrap_or_else(|| self.known(Ty::Unit));
                    self.unify(rv, v);
                }
                self.known(Ty::Never)
            }
            Expr::Continue(_) => self.known(Ty::Never),
            Expr::Call(c) if self.prog.prim_method_call(c, self.self_sid).is_some() => {
                // (作り直した式は推論の終わりまで keep に持ち (後回しのクロージャが指す)、アドレスでは記録しない:
                //  捨てた後のアドレスが codegen の作るノードと重なると別の式の型を引いてしまう。位置で記録する)
                let e = self.prog.prim_method_call(c, self.self_sid).unwrap();
                let ki = self.keep.len();
                self.keep.push(vec![e]);
                let x: *const Expr = &self.keep[ki][0];
                self.no_record += 1;
                self.pos_record += 1;
                let v = self.expr(unsafe { &*x });
                self.pos_record -= 1;
                self.no_record -= 1;
                v
            }
            Expr::Call(c) => {
                if let Expr::Path(p) = &*c.func {
                    // `S::default()` / `<S>::default()` (derive の Default は関数として見えない): S
                    if c.args.is_empty() && p.path.segments.last().is_some_and(|s| s.ident == "default") {
                        let sp = self.prog.strip_mods(&p.path);
                        let pp = sp.as_ref().unwrap_or(&p.path);
                        let ty: Option<syn::Type> = match &p.qself {
                            Some(q) => Some((*q.ty).clone()),
                            None if pp.segments.len() == 2 && pp.segments[0].ident != "Default" => {
                                Some(syn::Type::Path(syn::TypePath { qself: None, path: syn::Path::from(pp.segments[0].clone()) }))
                            }
                            None => None,
                        };
                        if let Some(t) = ty.and_then(|t| self.prog.parse_ty(&t, self.env).ok()).filter(|&t| !self.prog.has_param(t)) {
                            return self.known(t);
                        }
                    }
                    // `u32::from_le_bytes(x)`: 引数は [u8; 4]
                    let segs = &p.path.segments;
                    if segs.len() == 2
                        && c.args.len() == 1
                        && matches!(segs[1].ident.to_string().as_str(), "from_le_bytes" | "from_be_bytes" | "from_ne_bytes")
                    {
                        if let Some(t) = Ty::prim(&segs[0].ident.to_string()).filter(|t| t.is_int()) {
                            let a = self.expr(&c.args[0]);
                            let k = self.known(Ty::Array(intern(Ty::U8), t.bits() / 8));
                            self.unify(a, k);
                            return self.known(t);
                        }
                    }
                    if let Some((sid, vi)) = self.prog.variant_by_path(&p.path, self.self_sid) {
                        let args = c
                            .args
                            .iter()
                            .enumerate()
                            .map(|(i, a)| (i.to_string(), self.expr(a)))
                            .collect();
                        return self.adt_ctor(sid, Some(vi), args);
                    }
                    let local = p
                        .path
                        .get_ident()
                        .is_some_and(|id| self.lookup(&id.to_string()).is_some());
                    if !local && self.prog.fn_by_path(&p.path, self.self_sid).is_none() {
                        if let Some(sid) = self.prog.struct_by_path(&p.path, self.self_sid) {
                            let args = c
                                .args
                                .iter()
                                .enumerate()
                                .map(|(i, a)| (i.to_string(), self.expr(a)))
                                .collect();
                            return self.adt_ctor(sid, None, args);
                        }
                    }
                }
                let callee = match &*c.func {
                    Expr::Path(p)
                        if p.path
                            .get_ident()
                            .is_some_and(|id| self.lookup(&id.to_string()).is_some()) =>
                    {
                        None
                    }
                    Expr::Path(p) => self.prog.fn_by_path(&p.path, self.self_sid),
                    _ => None,
                };
                let args: Vec<u32> = c.args.iter().map(|a| self.expr(a)).collect();
                // `Box::new(x)` は `&mut T` (所有する参照として扱う)
                if let Expr::Path(p) = &*c.func {
                    let n = p.path.segments.len();
                    if callee.is_none()
                        && n >= 2
                        && p.path.segments[n - 1].ident == "new"
                        && p.path.segments[n - 2].ident == "Box"
                        && args.len() == 1
                    {
                        return self.with(Shape::Bx(args[0]));
                    }
                }
                if callee.is_none() {
                    // クロージャ (の参照) / `&dyn Fn` の呼び出し
                    let f = self.expr(&c.func);
                    return match self.callable(f, args.len()) {
                        Some((ps, r)) => {
                            for (a, p) in args.into_iter().zip(ps) {
                                self.unify(a, p);
                            }
                            r
                        }
                        None => self.var(),
                    };
                }
                self.apply(callee, args, 0)
            }
            Expr::Closure(c) => {
                // 引数・戻り値を型変数にして本体を推論する (キャプチャは外のスコープを見る)
                let st = self.adt_args.len() as u32;
                let n = c.inputs.len() as u32;
                let mut vs = vec![];
                for p in &c.inputs {
                    let v = match crate::closure::param(p).1 {
                        Some(t) => match self.prog.parse_ty(t, self.env) {
                            Ok(t) => self.known(t),
                            Err(_) => self.var(),
                        },
                        None => self.var(),
                    };
                    vs.push(v);
                }
                let rv = match &c.output {
                    syn::ReturnType::Type(_, t) => match self.prog.parse_ty(t, self.env) {
                        Ok(t) => self.known(t),
                        Err(_) => self.var(),
                    },
                    syn::ReturnType::Default => self.var(),
                };
                self.adt_args.extend(vs.iter().copied().chain([rv]));
                // (クロージャの struct を後で解けるように、キャプチャの型変数を覚えておく)
                if self.clo_ctx.is_none() {
                    if let Some((r, base, b)) = self.clo_root.take() {
                        let mut m = crate::fxhash::FxMap::default();
                        // SAFETY: b は infer の間生きている関数の本体を指す
                        for (i, cl) in crate::closure::closures_of(unsafe { &*b }).iter().enumerate() {
                            m.insert(crate::closure::closure_key(cl), base + i as u32);
                        }
                        self.clo_ctx = Some((r, m));
                    }
                }
                if let Some((root, keys)) = &self.clo_ctx {
                    if let Some(fid) = keys.get(&crate::closure::closure_key(c)).and_then(|k| self.prog.closure_by.get(&(*root, *k))) {
                        if let Some(cd) = self.prog.closures.get(fid) {
                            let caps: Option<Vec<u32>> = cd.captures.iter().map(|n| self.lookup(n)).collect();
                            let free: Vec<usize> = c
                                .inputs
                                .iter()
                                .enumerate()
                                .filter(|(_, p)| crate::closure::param(p).1.is_none())
                                .map(|(i, _)| i)
                                .collect();
                            if let Some(caps) = caps {
                                self.clo_info.insert(st, (*fid, caps, free));
                            }
                        }
                    }
                }
                // codegen がクロージャの型引数を決めるのに使う: 引数 (パターンのアドレス) と戻り値 (式のアドレス)
                if self.no_record == 0 {
                    for (p, &v) in c.inputs.iter().zip(&vs) {
                        self.lets.push((p as *const Pat as usize, v));
                    }
                    self.lets.push((c as *const syn::ExprClosure as usize, rv));
                }
                if self.defer > 0 {
                    self.pending.push((c as *const syn::ExprClosure, vs, rv));
                } else {
                    self.closure_body(c, &vs, rv);
                }
                self.with(Shape::Clo(st, n))
            }
            // 値としての `[a, b].into_iter()`: Vec の IntoIter (codegen が vec![..] にする)
            Expr::MethodCall(mc)
                if mc.method == "into_iter"
                    && mc.args.is_empty()
                    && matches!(&*mc.receiver, Expr::Array(_))
                    && self.prog.struct_index.contains_key("IntoIter") =>
            {
                let Expr::Array(a) = &*mc.receiver else { unreachable!() };
                let el = self.var();
                for x in &a.elems {
                    let v = self.expr(x);
                    self.unify(el, v);
                }
                let ib = self.prog.struct_index["IntoIter"];
                self.adt(ib, vec![el])
            }
            // `v.get(a..b)` (Vec / 配列 / スライス): Option<&[T]>
            Expr::MethodCall(mc)
                if (mc.method == "get" || mc.method == "get_mut")
                    && matches!(mc.args.first(), Some(Expr::Range(_)))
                    && {
                        let r = self.expr(&mc.receiver);
                        self.elem_of(r).is_some()
                    } =>
            {
                let r = self.expr(&mc.receiver);
                let el = self.elem_of(r).unwrap();
                if let Some(Expr::Range(rg)) = mc.args.first() {
                    for x in rg.start.iter().chain(rg.end.iter()) {
                        let v = self.expr(x);
                        let k = self.known(Ty::Usize);
                        self.unify(v, k);
                    }
                }
                let m = mc.method == "get_mut";
                let sl = self.with(Shape::Slc(el));
                let rr = self.with(Shape::Ref(m, sl));
                self.option_of(rr)
            }
            Expr::MethodCall(mc)
                if {
                    let r = self.expr_peek_elem(&mc.receiver);
                    r.is_some()
                } =>
            {
                let r = self.expr(&mc.receiver);
                let el = self.elem_of(r).unwrap();
                let args: Vec<u32> = mc.args.iter().map(|a| self.expr(a)).collect();
                match mc.method.to_string().as_str() {
                    "len" => self.known(Ty::Usize),
                    "is_empty" | "contains" => self.known(Ty::Bool),
                    // `s.try_into()`: Result<[T; N], TryFromSliceError> (N は使われ方から)
                    "try_into" if self.prog.struct_index.contains_key("TryFromSliceError") => {
                        let rb = self.prog.struct_index["Result"];
                        let eb = self.prog.struct_index["TryFromSliceError"];
                        let t = self.var();
                        let e = self.known(Ty::Struct(eb));
                        self.adt(rb, vec![t, e])
                    }
                    // `s.get(a..b)`: Option<&[T]>
                    "get" | "get_mut" if matches!(mc.args.first(), Some(Expr::Range(_))) => {
                        let m = mc.method == "get_mut";
                        let sl = self.with(Shape::Slc(el));
                        let rr = self.with(Shape::Ref(m, sl));
                        self.option_of(rr)
                    }
                    "get" | "get_mut" | "first" | "last" => {
                        for a in args {
                            let k = self.known(Ty::Usize);
                            self.unify(a, k);
                        }
                        let m = mc.method == "get_mut";
                        let rr = self.with(Shape::Ref(m, el));
                        self.option_of(rr)
                    }
                    // (添字の引数は usize)
                    "swap" | "truncate" | "rotate_left" | "rotate_right" => {
                        for a in args {
                            let k = self.known(Ty::Usize);
                            self.unify(a, k);
                        }
                        self.known(Ty::Unit)
                    }
                    "split_at" | "split_at_mut" => {
                        for a in args {
                            let k = self.known(Ty::Usize);
                            self.unify(a, k);
                        }
                        self.var()
                    }
                    // 値としての `xs.iter()` / `iter_mut()`: prelude の Iter<T> / IterMut<T>
                    "iter" | "iter_mut" => {
                        let name = if mc.method == "iter" { "Iter" } else { "IterMut" };
                        match self.prog.struct_index.get(name) {
                            Some(&b) => self.adt(b, vec![el]),
                            None => self.var(),
                        }
                    }
                    "as_ptr" => {
                        let k = self.resolve(el).map(|t| Ty::Ptr(false, intern(t)));
                        match k {
                            Some(t) => self.known(t),
                            None => self.var(),
                        }
                    }
                    "as_mut_ptr" => {
                        let k = self.resolve(el).map(|t| Ty::Ptr(true, intern(t)));
                        match k {
                            Some(t) => self.known(t),
                            None => self.var(),
                        }
                    }
                    // `a.as_slice()`: &[T]
                    "as_slice" | "as_mut_slice" => {
                        let sl = self.with(Shape::Slc(el));
                        self.with(Shape::Ref(mc.method == "as_mut_slice", sl))
                    }
                    _ => self.var(),
                }
            }
            Expr::MethodCall(mc) if is_builtin_scalar_method(&mc.method.to_string()) => {
                let r = self.expr(&mc.receiver);
                let rv = self.deref_expr(r);
                let args: Vec<u32> = mc.args.iter().map(|a| self.expr(a)).collect();
                // (String::as_bytes など、struct が同じ名前のメソッドを持つならそれ)
                let root = self.find(rv);
                let sid = match self.shape[root as usize] {
                    Some(Shape::Known(Ty::Struct(sid))) => Some(sid),
                    Some(Shape::Adt(b, _, _)) => Some(b),
                    _ => None,
                };
                if let Some(id) = sid.and_then(|s| self.prog.method(s, &mc.method.to_string())) {
                    return self.apply_recv(Some(id), args, rv);
                }
                self.scalar_method(mc, r, rv, args)
            }
            Expr::MethodCall(mc) => {
                let r = self.expr(&mc.receiver);
                let rv = self.deref_expr(r);
                let root = self.find(rv);
                let sid = match self.shape[root as usize] {
                    Some(Shape::Known(Ty::Struct(sid))) => Some(sid),
                    Some(Shape::Adt(b, _, _)) => Some(b),
                    _ => None,
                };
                let n = mc.method.to_string();
                let mut callee = sid.and_then(|sid| {
                    self.prog
                        .method(sid, &n)
                        .or_else(|| self.prog.default_method(sid, &n))
                });
                // メソッドがなく `deref` があれば、deref した先 (`rc.borrow()` の RefCell) で探す
                let mut rv = rv;
                let mut sid = sid;
                let mut hops = 0;
                while callee.is_none() && hops < 4 {
                    let Some(s0) = sid else { break };
                    let Some(did) = self.prog.method(s0, "deref") else { break };
                    if !matches!(n.as_str(), "clone" | "to_string" | "cmp" | "partial_cmp" | "eq") {
                        let r = self.apply_recv(Some(did), vec![], rv);
                        rv = self.deref_expr(r);
                        let root = self.find(rv);
                        sid = match self.shape[root as usize] {
                            Some(Shape::Known(Ty::Struct(sid))) => Some(sid),
                            Some(Shape::Adt(b, _, _)) => Some(b),
                            _ => None,
                        };
                        callee = sid.and_then(|sid| {
                            self.prog.method(sid, &n).or_else(|| self.prog.default_method(sid, &n))
                        });
                    } else {
                        break;
                    }
                    hops += 1;
                }
                // derive(Clone) の clone (メソッドの本体は codegen が作る): 受け手 (参照を外した) と同じ型
                if callee.is_none() && n == "clone" && mc.args.is_empty() && sid.is_some() {
                    return rv;
                }
                let mut args: Vec<u32> = mc.args.iter().map(|a| self.expr(a)).collect();
                // `it.zip(&v)` / `it.chain([x])`: Vec / スライス / 配列の引数は codegen が `.iter()` / `.into_iter()` にする。
                // 型もそのイテレータのものにする (しないと要素の型が決まらず、`x == "s"` で &String を &str と取り違える)
                if matches!(n.as_str(), "zip" | "chain") && args.len() == 1 {
                    let mut base = self.resolve(args[0]);
                    let is_ref = matches!(base, Some(Ty::Ref(..)));
                    while let Some(Ty::Ref(_, i)) = base {
                        if matches!(i, Ty::Slice(_)) {
                            break;
                        }
                        base = Some(*i);
                    }
                    let vec_id = self.prog.struct_index.get("Vec").copied();
                    let seq = match base {
                        Some(Ty::Ref(_, Ty::Slice(_))) | Some(Ty::Array(..)) => true,
                        Some(Ty::Struct(sid)) => Some(self.prog.base_of(sid)) == vec_id,
                        _ => false,
                    };
                    let syntactic_array = matches!(&mc.args[0], Expr::Array(_) | Expr::Repeat(_));
                    if seq || syntactic_array {
                        let a = &mc.args[0];
                        let e: Expr = match (is_ref, a) {
                            (true, Expr::Reference(r)) => {
                                let inner = &r.expr;
                                syn::parse_quote!((#inner).iter())
                            }
                            (true, _) => syn::parse_quote!((#a).iter()),
                            _ => syn::parse_quote!((#a).into_iter()),
                        };
                        self.no_record += 1;
                        let ki = self.keep.len();
                        self.keep.push(vec![e]);
                        let x: *const Expr = &self.keep[ki][0];
                        let v = self.expr(unsafe { &*x });
                        self.no_record -= 1;
                        args[0] = v;
                    }
                }
                // `&dyn Trait` のメソッド: trait の宣言のシグネチャ
                let mut dt = self.resolve(rv);
                while let Some(Ty::Ref(_, i)) = dt {
                    dt = Some(*i);
                }
                if let Some(Ty::Dyn(tr)) = dt {
                    let decl = self.prog.traits.get(tr).and_then(|ds| {
                        ds.iter()
                            .copied()
                            .find(|&d| self.prog.names[d as usize].rsplit("::").next() == Some(&n))
                    });
                    if let Some(d) = decl {
                        let sig = &self.prog.sigs[d as usize];
                        let (ps, ret) = (sig.params[1..].to_vec(), sig.ret);
                        for (a, p) in args.into_iter().zip(ps) {
                            if !self.prog.has_param(p) {
                                let k = self.known(p);
                                self.unify(a, k);
                            }
                        }
                        if !self.prog.has_param(ret) {
                            return self.known(ret);
                        }
                    }
                    return self.var();
                }
                // `x.to_string()` (Display): String
                if callee.is_none() && n == "to_string" && args.is_empty() {
                    if let Some(&ss) = self.prog.struct_index.get("String") {
                        return self.known(Ty::Struct(ss));
                    }
                }
                // `parts.join(", ")` / `.concat()`: String (要素が Vec なら、その Vec)
                if callee.is_none() && matches!(n.as_str(), "join" | "concat") && args.len() <= 1 {
                    let mut t = self.resolve(rv);
                    while let Some(Ty::Ref(_, i)) = t {
                        t = Some(*i);
                    }
                    let elem = match t {
                        Some(Ty::Array(e, _)) | Some(Ty::Slice(e)) => Some(*e),
                        Some(Ty::Struct(s)) if self.prog.structs[s as usize].name.starts_with("Vec") => {
                            self.prog.structs[s as usize].args.first().copied()
                        }
                        _ => None,
                    };
                    if let Some(e @ Ty::Struct(es)) = elem {
                        if n == "concat" && self.prog.base_of(es) == self.prog.struct_index.get("Vec").copied().unwrap_or(u32::MAX) {
                            return self.known(e);
                        }
                    }
                    // (列でない受け手 (`path.join("x")`) は trait のメソッドとして下で見る)
                    if elem.is_some() || t.is_none() {
                        if let Some(&ss) = self.prog.struct_index.get("String") {
                            return self.known(Ty::Struct(ss));
                        }
                    }
                }
                // 組み込みの比較 (`3.cmp(&5)` / derive した型の cmp): Ordering / Option<Ordering>
                if callee.is_none() && matches!(n.as_str(), "cmp" | "partial_cmp") && args.len() == 1 {
                    if let Some(&os) = self.prog.struct_index.get("Ordering") {
                        let o = self.known(Ty::Struct(os));
                        return if n == "cmp" { o } else { self.option_of(o) };
                    }
                }
                // スライス・配列の受け手 (`v.swap(i, j)` の v: &mut [T]): Vec の同名メソッドのシグネチャで推論する
                if callee.is_none() && sid.is_none() {
                    let mut t = self.resolve(rv);
                    while let Some(Ty::Ref(_, i)) = t {
                        t = Some(*i);
                    }
                    let root = self.find(rv);
                    let elem = match (t, self.shape[root as usize]) {
                        (Some(Ty::Slice(e)) | Some(Ty::Array(e, _)), _) => Some(self.known(*e)),
                        (_, Some(Shape::Slc(e)) | Some(Shape::Arr(e, _))) => Some(e),
                        _ => None,
                    };
                    if let (Some(e), Some(&vb)) = (elem, self.prog.struct_index.get("Vec")) {
                        if let Some(id) = self.prog.method(vb, &n) {
                            if !matches!(n.as_str(), "len" | "is_empty" | "iter" | "iter_mut" | "into_iter") {
                                let vv = self.adt(vb, vec![e]);
                                return self.apply_recv(Some(id), args.clone(), vv);
                            }
                        }
                    }
                    // 受け手の型が分からない (ジェネリックな `&mut [T]` など) ときも、添字の引数 (`swap(i, j)`) は usize
                    if t.is_none() && matches!(n.as_str(), "swap" | "split_at" | "split_at_mut" | "truncate" | "rotate_left" | "rotate_right" | "split_off") {
                        for &a in &args {
                            let u = self.known(Ty::Usize);
                            self.unify(a, u);
                        }
                    }
                }
                if callee.is_none() && sid.is_none() {
                    // プリミティブ型への trait 実装のメソッド
                    if let Some(t) = self.resolve(rv) {
                        if let Some(id) = self.prog.prim_method(t, &n) {
                            return self.apply_recv(Some(id), args, rv);
                        }
                    }
                    // struct でない受け手: 整数などの組み込みメソッド (min / max など)。
                    // 受け手の型が分からない未知のメソッドは分からないまま
                    if self.resolve(rv).is_none()
                        && !is_builtin_scalar_method(&n)
                        && !matches!(n.as_str(), "min" | "max" | "len" | "is_empty")
                    {
                        return self.var();
                    }
                    return self.scalar_method(mc, r, rv, args);
                }
                // `v.extend(arg)` (Vec / BinaryHeap): arg の要素 (参照なら中身) が v の要素
                if n == "extend" && args.len() == 1 {
                    if let Some(b) = sid {
                        let bn = self.prog.structs[self.prog.base_of(b) as usize].name.as_str();
                        if matches!(bn, "Vec" | "BinaryHeap") {
                            let root = self.find(rv);
                            let elem = match self.shape[root as usize] {
                                Some(Shape::Adt(_, st, l)) => self.adt_vars_of(st, l).first().copied(),
                                Some(Shape::Known(Ty::Struct(s))) => {
                                    self.prog.structs[s as usize].args.first().copied().map(|t| self.known(t))
                                }
                                _ => None,
                            };
                            let known_elem = elem.is_some_and(|ev| self.resolve(ev).is_some());
                            if let (Some(ev), Some(at), false) = (elem, self.resolve(args[0]), known_elem) {
                                if let Some(it) = self.extend_item(at) {
                                    // (`&T` の要素は T を足す。`&str` / `&[T]` はそれ自体が要素)
                                    let it = match it {
                                        Ty::Ref(_, i) if !matches!(i, Ty::Str | Ty::Slice(_) | Ty::Dyn(_)) => *i,
                                        t => t,
                                    };
                                    let k = self.known(it);
                                    self.unify(ev, k);
                                }
                            }
                            return self.known(Ty::Unit);
                        }
                    }
                }
                // String にないメソッド (`s.to_uppercase()`): &str の helper
                if callee.is_none() {
                    if let Some(b) = sid {
                        if self.prog.structs[self.prog.base_of(b) as usize].name == "String" {
                            let h = if n == "split" && args.first().is_some_and(|&a| self.resolve(a) != Some(Ty::Char)) {
                                "split_str".to_string()
                            } else if n == "split_whitespace" {
                                "split_ws".to_string()
                            } else if matches!(mc.args.first(), Some(Expr::Closure(_)))
                                && self.prog.index.contains_key(&format!("__exr_str_{n}_fn"))
                            {
                                format!("{n}_fn")
                            } else {
                                n.clone()
                            };
                            if let Some(&id) = self.prog.index.get(&format!("__exr_str_{h}")) {
                                return self.apply_helper(id, mc, args);
                            }
                        }
                    }
                }
                self.turbofish = mc.turbofish.as_ref().map(|t| t as *const _);
                let r = self.apply_recv(callee, args, rv);
                self.turbofish = None;
                if n == "collect" {
                    self.collects.push((rv, r));
                }
                r
            }
            _ => self.expr_rest(e),
        }
    }

    /// 整数 / &str / 生ポインタの組み込みメソッドの型
    fn scalar_method(&mut self, mc: &syn::ExprMethodCall, r: u32, rv: u32, args: Vec<u32>) -> u32 {
        let root = self.find(rv);
        let recv_ty = match self.shape[root as usize] {
            Some(Shape::Known(t)) => Some(t),
            _ => None,
        };
        let name = mc.method.to_string();
        // prelude の `__exr_str_<m>` / `__exr_char_<m>` (シグネチャで推論する)
        let pre = match self.shape[root as usize] {
            Some(Shape::Known(Ty::Str)) => Some("__exr_str_"),
            Some(Shape::Known(Ty::Char)) => Some("__exr_char_"),
            _ => None,
        };
        if let Some(pre) = pre {
            let h = if name == "split" && pre == "__exr_str_" && args.first().is_some_and(|&a| self.resolve(a) != Some(Ty::Char)) {
                "split_str".to_string()
            } else if name == "split_whitespace" {
                "split_ws".to_string()
            } else if matches!(mc.args.first(), Some(Expr::Closure(_)))
                && self.prog.index.contains_key(&format!("{pre}{name}_fn"))
            {
                format!("{name}_fn")
            } else {
                name.clone()
            };
            if let Some(&id) = self.prog.index.get(&format!("{pre}{h}")) {
                let builtin = matches!(name.as_str(), "len" | "is_empty" | "as_bytes" | "as_ptr");
                if !builtin {
                    return self.apply_helper(id, mc, args);
                }
            }
        }
        // 整数の `to_le_bytes` など: [u8; N]
        if let Some(t) = recv_ty.filter(|t| t.is_int()) {
            if matches!(name.as_str(), "to_le_bytes" | "to_be_bytes" | "to_ne_bytes") {
                return self.known(Ty::Array(intern(Ty::U8), t.bits() / 8));
            }
        }
        // 浮動小数点のメソッド (bool を返すもの / ビット列 / Ordering 以外は同じ型)
        if let Some(t) = recv_ty.filter(|t| t.is_float()) {
            return match name.as_str() {
                "is_nan" | "is_finite" | "is_infinite" | "is_sign_negative" | "is_sign_positive" | "is_normal" => {
                    self.known(Ty::Bool)
                }
                "to_bits" => self.known(if t == Ty::F32 { Ty::U32 } else { Ty::U64 }),
                "total_cmp" => match self.prog.struct_index.get("Ordering") {
                    Some(&o) => self.known(Ty::Struct(o)),
                    None => self.var(),
                },
                _ => {
                    for a in args {
                        let _ = a;
                    }
                    self.known(t)
                }
            };
        }
        // &str
        if let Some(Shape::Known(Ty::Str)) = self.shape[root as usize] {
            return match name.as_str() {
                "len" => self.known(Ty::Usize),
                "is_empty" => self.known(Ty::Bool),
                "as_bytes" => self.known(Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8))))),
                "as_ptr" => self.known(Ty::Ptr(false, intern(Ty::U8))),
                // prelude の &str の関数 (split などのイテレータは分からないまま)
                "trim" | "trim_start" | "trim_end" => self.known(Ty::Ref(false, intern(Ty::Str))),
                "starts_with" | "ends_with" | "contains" => self.known(Ty::Bool),
                "to_string" | "to_owned" => match self.prog.struct_index.get("String") {
                    Some(&s) => self.known(Ty::Struct(s)),
                    None => self.var(),
                },
                "find" => {
                    let u = self.known(Ty::Usize);
                    self.option_of(u)
                }
                _ => self.var(),
            };
        }
        // 生ポインタ
        if let Some(Ty::Ptr(_, e)) = self.resolve(r) {
            return match name.as_str() {
                "add" | "sub" | "offset" | "wrapping_add" | "wrapping_sub" | "wrapping_offset" => r,
                "is_null" => self.known(Ty::Bool),
                "read" | "read_volatile" | "read_unaligned" => self.known(*e),
                _ => self.known(Ty::Unit),
            };
        }
        let _ = recv_ty;
        match name.as_str() {
            "wrapping_add" | "wrapping_sub" | "wrapping_mul" | "min" | "max" | "saturating_add"
            | "saturating_sub" => {
                for a in args {
                    self.unify(a, rv);
                }
                rv
            }
            "rem_euclid" | "div_euclid" | "next_multiple_of" => {
                for a in args {
                    self.unify(a, rv);
                }
                rv
            }
            "overflowing_add" | "overflowing_sub" | "overflowing_mul" | "overflowing_pow" => {
                for a in args {
                    let k = if name == "overflowing_pow" { self.known(Ty::U32) } else { rv };
                    self.unify(a, k);
                }
                let b = self.known(Ty::Bool);
                let tb = self.prog.tuple_base(2);
                self.adt(tb, vec![rv, b])
            }
            "checked_pow" => {
                for a in args {
                    let k = self.known(Ty::U32);
                    self.unify(a, k);
                }
                self.option_of(rv)
            }
            "leading_ones" | "trailing_ones" => self.known(Ty::U32),
            "checked_abs" | "checked_neg" => self.option_of(rv),
            // `x.try_into()`: Result<T, TryFromIntError> (T は使われ方から)
            "try_into" => match (self.prog.struct_index.get("Result"), self.prog.struct_index.get("TryFromIntError")) {
                (Some(&rb), Some(&eb)) => {
                    let t = self.var();
                    let e = self.known(Ty::Struct(eb));
                    self.adt(rb, vec![t, e])
                }
                _ => self.var(),
            },
            "wrapping_neg" | "abs" | "signum" | "swap_bytes" | "to_ascii_uppercase"
            | "to_ascii_lowercase" | "next_power_of_two" | "isqrt" => rv,
            "pow" | "rotate_left" | "rotate_right" | "wrapping_pow" | "saturating_pow" => {
                for a in args {
                    let k = self.known(Ty::U32);
                    self.unify(a, k);
                }
                rv
            }
            "checked_add" | "checked_sub" | "checked_mul" | "checked_div" | "checked_rem" => {
                for a in args {
                    self.unify(a, rv);
                }
                self.option_of(rv)
            }
            "count_ones" | "count_zeros" | "leading_zeros" | "trailing_zeros" => {
                self.known(Ty::U32)
            }
            "abs_diff" => self.var(),
            _ => self.known(Ty::Bool),
        }
    }

    fn expr_rest(&mut self, e: &Expr) -> u32 {
        match e {
            Expr::Cast(c) => {
                self.expr(&c.expr);
                match self.prog.parse_ty(&c.ty, self.env) {
                    Ok(t) => self.known(t),
                    Err(_) => self.var(),
                }
            }
            _ => self.var(),
        }
    }
}

/// 必ず発散する式 (`return` / `break` / `continue` / `panic!(..)`)
fn diverges(e: &Expr) -> bool {
    match e {
        Expr::Return(_) | Expr::Break(_) | Expr::Continue(_) => true,
        Expr::Macro(m) => diverging_macro(&m.mac),
        _ => false,
    }
}

fn diverging_macro(m: &syn::Macro) -> bool {
    m.path.segments.last().is_some_and(|s| matches!(s.ident.to_string().as_str(), "panic" | "unreachable" | "todo" | "unimplemented"))
}

fn is_fmt_macro(m: &syn::Macro) -> bool {
    m.path.segments.last().is_some_and(|s| {
        matches!(s.ident.to_string().as_str(), "print" | "println" | "eprint" | "eprintln" | "write" | "writeln" | "format" | "panic")
    })
}

/// マクロの引数の式の記録に使う、ソース上の位置のキー (アドレスのキーと重ならないよう最上位ビットを立てる)
pub fn pos_key(e: &Expr) -> usize {
    use syn::spanned::Spanned;
    let sp = e.span();
    let (a, b) = (sp.start(), sp.end());
    // (作り直したノード (call_site) は位置で区別できない: 記録しない)
    let cs = proc_macro2::Span::call_site();
    if a == cs.start() && b == cs.end() {
        return 0;
    }
    (1usize << 63)
        | ((a.line & 0xFFFFF) << 43)
        | ((a.column & 0x7FF) << 32)
        | ((b.line & 0xFFFFF) << 12)
        | (b.column & 0xFFF)
}
