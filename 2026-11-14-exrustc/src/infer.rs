//! 関数 1 個分の型推論 (union-find による単一化)。
//!
//! `let mut i = 0; while i < n { .. }` や `let mut i = 0; bump(&mut i)` のように、
//! 注釈のない let の型を後続の使用箇所から決めるためのパス。型エラーの報告はここでは
//! 行わず (矛盾は無視)、codegen 側の検査に任せる。結果は「注釈なし let → 型」の表。

use std::collections::HashMap;

use syn::{BinOp, Expr, Lit, Pat, Stmt, UnOp};

use crate::front::{FnSig, Program, Ty, TyEnv, intern};

pub type LetTypes = HashMap<usize, Ty>;

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

#[derive(Clone, Copy)]
enum Shape {
    Known(Ty),
    /// 参照。中身は別の型変数
    Ref(bool, u32),
    /// ジェネリックな struct / enum (定義の id) に、型引数の型変数 (adt_args[start..start+len]) を入れたもの
    Adt(u32, u32, u32),
}

struct Inf<'p> {
    prog: &'p Program,
    /// impl の中なら self 型
    self_sid: Option<u32>,
    env: &'p TyEnv,
    parent: Vec<u32>,
    shape: Vec<Option<Shape>>,
    /// 整数リテラル由来 (決まらなければ i32 にしてよい) か
    int_lit: Vec<bool>,
    scopes: Vec<Vec<(String, u32)>>,
    lets: Vec<(usize, u32)>,
    ret: u32,
    /// Shape::Adt の型引数の型変数
    adt_args: Vec<u32>,
}

/// sig はこの関数 (インスタンスなら型引数を入れた後) のシグネチャ、env は `Self` と型パラメータ
pub fn infer(prog: &Program, f: &syn::ItemFn, sig: &FnSig, env: &TyEnv) -> LetTypes {
    let ret = sig.ret;
    let mut s = Inf {
        prog,
        self_sid: env.self_sid,
        env,
        parent: vec![],
        shape: vec![],
        int_lit: vec![],
        scopes: vec![vec![]],
        lets: vec![],
        ret: 0,
        adt_args: vec![],
    };
    s.ret = s.known(ret);
    for (a, &t) in f.sig.inputs.iter().zip(&sig.params) {
        let name = match a {
            syn::FnArg::Receiver(_) => "self".to_string(),
            syn::FnArg::Typed(pt) => match &*pt.pat {
                Pat::Ident(pi) => pi.ident.to_string(),
                _ => continue,
            },
        };
        let v = s.known(t);
        s.scopes[0].push((name, v));
    }
    let tail = s.block(&f.block);
    let ret = s.ret;
    s.unify(tail, ret);
    let lets = std::mem::take(&mut s.lets);
    lets.into_iter().filter_map(|(k, v)| Some((k, s.resolve(v)?))).collect()
}

impl Inf<'_> {
    fn var(&mut self) -> u32 {
        self.parent.push(self.parent.len() as u32);
        self.shape.push(None);
        self.int_lit.push(false);
        self.parent.len() as u32 - 1
    }
    fn with(&mut self, sh: Shape) -> u32 {
        let v = self.var();
        self.shape[v as usize] = Some(sh);
        v
    }
    fn known(&mut self, t: Ty) -> u32 {
        match t {
            Ty::Ref(m, inner) => {
                let i = self.known(*inner);
                self.with(Shape::Ref(m, i))
            }
            // 型パラメータを含む (まだ記号的な) インスタンスは Adt の形にする
            Ty::Struct(_) if self.prog.has_param(t) => {
                let mut pv = vec![];
                self.known_g(t, &mut pv)
            }
            t => self.with(Shape::Known(t)),
        }
    }
    fn adt(&mut self, base: u32, vars: Vec<u32>) -> u32 {
        let start = self.adt_args.len() as u32;
        let len = vars.len() as u32;
        self.adt_args.extend(vars);
        self.with(Shape::Adt(base, start, len))
    }
    fn adt_vars_of(&self, start: u32, len: u32) -> Vec<u32> {
        self.adt_args[start as usize..(start + len) as usize].to_vec()
    }
    /// v をジェネリックな定義 base のインスタンスとして見たときの型引数の型変数
    /// (分からなければ v にその形を与える)。非ジェネリックなら空
    fn adt_view(&mut self, v: u32, base: u32) -> Vec<u32> {
        let n = self.prog.structs[base as usize].generics.len();
        if n == 0 {
            return vec![];
        }
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Adt(b, st, len)) if b == base => self.adt_vars_of(st, len),
            Some(Shape::Known(Ty::Struct(i))) if self.prog.base_of(i) == base => {
                let args = self.prog.structs[i as usize].args.clone();
                args.into_iter().map(|a| self.known(a)).collect()
            }
            None => {
                let vars: Vec<u32> = (0..n).map(|_| self.var()).collect();
                let a = self.adt(base, vars.clone());
                self.unify(r, a);
                vars
            }
            _ => (0..n).map(|_| self.var()).collect(),
        }
    }
    /// ジェネリックな定義 base のフィールド型 (Param を含む) を、型引数の型変数 vars で具体化した型変数
    fn field_var(&mut self, t: Ty, vars: &[u32]) -> u32 {
        let mut pv: Vec<Option<u32>> = vars.iter().map(|&v| Some(v)).collect();
        self.known_g(t, &mut pv)
    }
    fn find(&mut self, mut v: u32) -> u32 {
        while self.parent[v as usize] != v {
            let p = self.parent[v as usize];
            self.parent[v as usize] = self.parent[p as usize];
            v = p;
        }
        v
    }
    fn link(&mut self, from: u32, to: u32) {
        self.parent[from as usize] = to;
        self.int_lit[to as usize] |= self.int_lit[from as usize];
    }
    fn unify(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a == b {
            return;
        }
        match (self.shape[a as usize], self.shape[b as usize]) {
            (Some(Shape::Known(Ty::Never)), _) | (_, Some(Shape::Known(Ty::Never))) => {}
            (None, _) => self.link(a, b),
            (_, None) => self.link(b, a),
            (Some(Shape::Ref(_, x)), Some(Shape::Ref(_, y))) => {
                self.link(a, b);
                self.unify(x, y);
            }
            (Some(Shape::Known(x)), Some(Shape::Known(y))) if x == y => self.link(a, b),
            (Some(Shape::Adt(x, s1, l1)), Some(Shape::Adt(y, s2, l2))) if x == y => {
                self.link(a, b);
                for (p, q) in self.adt_vars_of(s1, l1).into_iter().zip(self.adt_vars_of(s2, l2)) {
                    self.unify(p, q);
                }
            }
            (Some(Shape::Adt(x, st, l)), Some(Shape::Known(Ty::Struct(i))))
            | (Some(Shape::Known(Ty::Struct(i))), Some(Shape::Adt(x, st, l)))
                if self.prog.base_of(i) == x =>
            {
                // 具体型の方を代表にする
                let (adt_root, known_root) = if matches!(self.shape[a as usize], Some(Shape::Adt(..))) { (a, b) } else { (b, a) };
                self.link(adt_root, known_root);
                let args = self.prog.structs[i as usize].args.clone();
                for (p, t) in self.adt_vars_of(st, l).into_iter().zip(args) {
                    let k = self.known(t);
                    self.unify(p, k);
                }
            }
            _ => {} // 矛盾は codegen で報告
        }
    }
    fn resolve(&mut self, v: u32) -> Option<Ty> {
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Known(t)) => Some(t),
            Some(Shape::Ref(m, i)) => Some(Ty::Ref(m, intern(self.resolve(i)?))),
            Some(Shape::Adt(b, st, l)) => {
                let mut args = vec![];
                for v in self.adt_vars_of(st, l) {
                    args.push(self.resolve(v)?);
                }
                self.prog.instantiate(b, args).ok().map(Ty::Struct)
            }
            None if self.int_lit[r as usize] => Some(Ty::I32),
            None => None,
        }
    }
    fn deref(&mut self, v: u32) -> u32 {
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Ref(_, i)) => i,
            _ => self.var(),
        }
    }
    /// 自動 deref しながらフィールドの型を引く
    fn field(&mut self, base: u32, name: &str) -> u32 {
        let (v, _) = self.auto_deref(base, None);
        let r = self.find(v);
        let sid = match self.shape[r as usize] {
            Some(Shape::Known(Ty::Struct(sid))) => sid,
            Some(Shape::Adt(b, _, _)) => b,
            _ => return self.var(),
        };
        let b = self.prog.base_of(sid);
        let vars = self.adt_view(v, b);
        match self.prog.field(b, name) {
            Some((ft, _)) => self.field_var(ft, &vars),
            None => self.var(),
        }
    }

    /// struct (vi = None) / バリアント (vi = Some) の値を作る式: 実引数をフィールド型と単一化する
    fn adt_ctor(&mut self, sid: u32, vi: Option<usize>, args: Vec<(String, u32)>) -> u32 {
        let base = self.prog.base_of(sid);
        let n = self.prog.structs[base as usize].generics.len();
        let vars: Vec<u32> = (0..n).map(|_| self.var()).collect();
        let fields: Vec<(String, Ty)> = {
            let sd = &self.prog.structs[base as usize];
            let f = match vi {
                Some(vi) => &sd.variants[vi].fields,
                None => &sd.fields,
            };
            f.iter().map(|x| (x.0.clone(), x.1)).collect()
        };
        for (name, a) in args {
            if let Some((_, ft)) = fields.iter().find(|f| f.0 == name) {
                let k = self.field_var(*ft, &vars);
                self.unify(a, k);
            }
        }
        if n == 0 { self.known(Ty::Struct(base)) } else { self.adt(base, vars) }
    }

    /// パターンのフィールド: v (auto-deref 済み) を base のインスタンスとして見て、フィールド型の型変数を返す
    fn pat_fields(&mut self, v: u32, sid: u32, vi: Option<usize>) -> Vec<(String, u32)> {
        let base = self.prog.base_of(sid);
        let vars = self.adt_view(v, base);
        if vars.is_empty() {
            let k = self.known(Ty::Struct(base));
            self.unify(v, k);
        }
        let fields: Vec<(String, Ty)> = {
            let sd = &self.prog.structs[base as usize];
            let f = match vi {
                Some(vi) => &sd.variants[vi].fields,
                None => &sd.fields,
            };
            f.iter().map(|x| (x.0.clone(), x.1)).collect()
        };
        fields.into_iter().map(|(n, t)| (n, self.field_var(t, &vars))).collect()
    }
    /// 呼び出し: 実引数を仮引数型と単一化して戻り値型を返す。skip はレシーバの分
    /// 型パラメータを含む型を、呼び出しごとの新しい型変数 (pv) で置き換えて作る
    fn known_g(&mut self, t: Ty, pv: &mut Vec<Option<u32>>) -> u32 {
        match t {
            Ty::Param(i) => match pv.get(i as usize).copied().flatten() {
                Some(v) => v,
                None => {
                    let v = self.var();
                    if pv.len() <= i as usize {
                        pv.resize(i as usize + 1, None);
                    }
                    pv[i as usize] = Some(v);
                    v
                }
            },
            Ty::Ref(m, inner) => {
                let i = self.known_g(*inner, pv);
                self.with(Shape::Ref(m, i))
            }
            Ty::Struct(sid) if self.prog.has_param(t) => {
                let sd = &self.prog.structs[sid as usize];
                let (base, args) = (sd.base.unwrap(), sd.args.clone());
                let vars: Vec<u32> = args.into_iter().map(|a| self.known_g(a, pv)).collect();
                self.adt(base, vars)
            }
            t => self.with(Shape::Known(t)),
        }
    }

    /// 呼び出し: 実引数を仮引数型と単一化して戻り値型を返す。skip はレシーバの分、
    /// self_ty はレシーバの型 (trait のデフォルトメソッドの Self を決める)
    fn apply(&mut self, callee: Option<u32>, args: Vec<u32>, skip: usize) -> u32 {
        self.apply_self(callee, args, skip, None)
    }

    /// メソッド呼び出し: recv はレシーバ (auto-deref 済み) の型変数。`Self` (impl の self 型 / trait の Self) と単一化する
    fn apply_recv(&mut self, callee: Option<u32>, args: Vec<u32>, recv: u32) -> u32 {
        let Some(id) = callee else { return self.var() };
        let sig = &self.prog.sigs[id as usize];
        let (params, ret, self_ty, is_trait) = (sig.params[1.min(sig.params.len())..].to_vec(), sig.ret, sig.self_ty, sig.trait_of.is_some());
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
        self.known_g(ret, &mut pv)
    }

    fn apply_self(&mut self, callee: Option<u32>, args: Vec<u32>, skip: usize, self_ty: Option<Ty>) -> u32 {
        match callee {
            Some(id) => {
                let sig = &self.prog.sigs[id as usize];
                let (params, ret) = (sig.params[skip.min(sig.params.len())..].to_vec(), sig.ret);
                let mut pv: Vec<Option<u32>> = vec![None; sig.generics.len()];
                if let (Some(t), true) = (self_ty, sig.trait_of.is_some()) {
                    pv[0] = Some(self.known(t));
                }
                for (a, p) in args.into_iter().zip(params) {
                    let k = self.known_g(p, &mut pv);
                    self.unify(a, k);
                }
                self.known_g(ret, &mut pv)
            }
            None => self.var(),
        }
    }

    fn lit_var(&mut self, l: &Lit) -> u32 {
        match l {
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
            _ => self.var(),
        }
    }

    /// 参照型の変数を辿って中身へ。参照を通ったら束縛モードを Ref にする
    fn auto_deref(&mut self, mut v: u32, mut mode: Option<bool>) -> (u32, Option<bool>) {
        loop {
            let r = self.find(v);
            match self.shape[r as usize] {
                Some(Shape::Ref(m, i)) => {
                    v = i;
                    mode = Some(if mode == Some(false) { false } else { m });
                }
                _ => return (v, mode),
            }
        }
    }

    fn bind_name(&mut self, name: String, v: u32, mode: Option<bool>) {
        let v = match mode {
            Some(m) => self.with(Shape::Ref(m, v)),
            None => v,
        };
        self.scopes.last_mut().unwrap().push((name, v));
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
            Pat::Ident(pi) => {
                if let Some((_, sub)) = &pi.subpat {
                    self.pat(sub, v, mode);
                }
                let mode = if pi.by_ref.is_some() { Some(pi.mutability.is_some()) } else { mode };
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
                let fts: Vec<u32> = if let Some((sid, vi)) = self.prog.variant_by_path(&ts.path, self.self_sid) {
                    self.pat_fields(v, sid, Some(vi)).into_iter().map(|f| f.1).collect()
                } else if let Some(sid) = self.prog.struct_by_path(&ts.path, self.self_sid) {
                    self.pat_fields(v, sid, None).into_iter().map(|f| f.1).collect()
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
            Pat::Struct(ps) => {
                let (v, mode) = self.auto_deref(v, mode);
                let fields = if let Some((sid, vi)) = self.prog.variant_by_path(&ps.path, self.self_sid) {
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

    fn lookup(&self, n: &str) -> Option<u32> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|s| s.iter().rev())
            .find(|(x, _)| x == n)
            .map(|&(_, v)| v)
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
                    self.lets.push((let_key(l), v));
                    match pat {
                        Pat::Ident(pi) if pi.subpat.is_none() => {
                            self.scopes.last_mut().unwrap().push((pi.ident.to_string(), v))
                        }
                        p => self.pat(p, v, None),
                    }
                }
                Stmt::Expr(e, semi) => {
                    let v = self.expr(e);
                    if i + 1 == n && semi.is_none() {
                        res = Some(v);
                    }
                }
                _ => {}
            }
        }
        self.scopes.pop();
        res.unwrap_or_else(|| self.known(Ty::Unit))
    }

    /// 式の型変数。値を作る式 (リテラル・構築・呼び出し) は結果の型も記録し、
    /// codegen が期待型のないところで使う (`Pair { a: 1 }.swap()` の 1 を後ろの使用から決める)
    fn expr(&mut self, e: &Expr) -> u32 {
        let v = self.expr_inner(e);
        if matches!(e, Expr::Lit(_) | Expr::Struct(_) | Expr::Call(_) | Expr::MethodCall(_) | Expr::Path(_) | Expr::Macro(_)) {
            self.lets.push((e as *const Expr as usize, v));
        }
        v
    }

    fn expr_inner(&mut self, e: &Expr) -> u32 {
        use BinOp::*;
        match e {
            Expr::Lit(l) => self.lit_var(&l.lit),
            Expr::Paren(p) => self.expr(&p.expr),
            Expr::Group(g) => self.expr(&g.expr),
            Expr::Block(b) => self.block(&b.block),
            Expr::Unsafe(u) => self.block(&u.block),
            Expr::Path(p)
                if p.path.segments.len() >= 2
                    || (p.path.get_ident().is_some_and(|id| self.lookup(&id.to_string()).is_none())
                        && self.prog.variant_by_path(&p.path, self.self_sid).is_some()) =>
            {
                match self.prog.variant_by_path(&p.path, self.self_sid) {
                    Some((sid, vi)) => self.adt_ctor(sid, Some(vi), vec![]),
                    None => self.var(),
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
            Expr::Path(p) => match p.path.get_ident().and_then(|id| self.lookup(&id.to_string())) {
                Some(v) => v,
                None => self.var(),
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
                    syn::Member::Unnamed(_) => self.var(),
                }
            }
            Expr::Struct(s) => {
                let args: Vec<(String, u32)> = s.fields.iter().map(|fv| (member_name(&fv.member), self.expr(&fv.expr))).collect();
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
                match b.op {
                    And(_) | Or(_) => {
                        let k = self.known(Ty::Bool);
                        self.unify(l, k);
                        self.unify(r, k);
                        k
                    }
                    Eq(_) | Ne(_) | Lt(_) | Le(_) | Gt(_) | Ge(_) => {
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
            Expr::Break(_) | Expr::Continue(_) => self.known(Ty::Never),
            Expr::Call(c) => {
                if let Expr::Path(p) = &*c.func {
                    if let Some((sid, vi)) = self.prog.variant_by_path(&p.path, self.self_sid) {
                        let args = c.args.iter().enumerate().map(|(i, a)| (i.to_string(), self.expr(a))).collect();
                        return self.adt_ctor(sid, Some(vi), args);
                    }
                    let local = p.path.get_ident().is_some_and(|id| self.lookup(&id.to_string()).is_some());
                    if !local && self.prog.fn_by_path(&p.path, self.self_sid).is_none() {
                        if let Some(sid) = self.prog.struct_by_path(&p.path, self.self_sid) {
                            let args = c.args.iter().enumerate().map(|(i, a)| (i.to_string(), self.expr(a))).collect();
                            return self.adt_ctor(sid, None, args);
                        }
                    }
                }
                let callee = match &*c.func {
                    Expr::Path(p) if p.path.get_ident().is_some_and(|id| self.lookup(&id.to_string()).is_some()) => None,
                    Expr::Path(p) => self.prog.fn_by_path(&p.path, self.self_sid),
                    _ => None,
                };
                let args: Vec<u32> = c.args.iter().map(|a| self.expr(a)).collect();
                self.apply(callee, args, 0)
            }
            Expr::MethodCall(mc) => {
                let r = self.expr(&mc.receiver);
                let (rv, _) = self.auto_deref(r, None);
                let root = self.find(rv);
                let sid = match self.shape[root as usize] {
                    Some(Shape::Known(Ty::Struct(sid))) => Some(sid),
                    Some(Shape::Adt(b, _, _)) => Some(b),
                    _ => None,
                };
                let n = mc.method.to_string();
                let callee = sid.and_then(|sid| self.prog.method(sid, &n).or_else(|| self.prog.default_method(sid, &n)));
                let args: Vec<u32> = mc.args.iter().map(|a| self.expr(a)).collect();
                self.apply_recv(callee, args, rv)
            }
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
