//! 関数 1 個分の型推論 (union-find による単一化)。
//!
//! `let mut i = 0; while i < n { .. }` や `let mut i = 0; bump(&mut i)` のように、
//! 注釈のない let の型を後続の使用箇所から決めるためのパス。型エラーの報告はここでは
//! 行わず (矛盾は無視)、codegen 側の検査に任せる。結果は「注釈なし let → 型」の表。

use std::collections::HashMap;

use syn::{BinOp, Expr, Lit, Pat, Stmt, UnOp};

use crate::front::{Program, Ty, intern};

pub type LetTypes = HashMap<usize, Ty>;

pub fn let_key(l: &syn::Local) -> usize {
    l as *const syn::Local as usize
}

#[derive(Clone, Copy)]
enum Shape {
    Known(Ty),
    /// 参照。中身は別の型変数
    Ref(bool, u32),
}

struct Inf<'p> {
    prog: &'p Program,
    /// impl の中なら self 型
    self_sid: Option<u32>,
    parent: Vec<u32>,
    shape: Vec<Option<Shape>>,
    /// 整数リテラル由来 (決まらなければ i32 にしてよい) か
    int_lit: Vec<bool>,
    scopes: Vec<Vec<(String, u32)>>,
    lets: Vec<(usize, u32)>,
    ret: u32,
}

pub fn infer(prog: &Program, f: &syn::ItemFn, id: u32) -> LetTypes {
    let sig = &prog.sigs[id as usize];
    let ret = sig.ret;
    let mut s = Inf {
        prog,
        self_sid: sig.impl_of,
        parent: vec![],
        shape: vec![],
        int_lit: vec![],
        scopes: vec![vec![]],
        lets: vec![],
        ret: 0,
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
            t => self.with(Shape::Known(t)),
        }
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
            _ => {} // 矛盾は codegen で報告
        }
    }
    fn resolve(&mut self, v: u32) -> Option<Ty> {
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Known(t)) => Some(t),
            Some(Shape::Ref(m, i)) => Some(Ty::Ref(m, intern(self.resolve(i)?))),
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
        let mut t = self.resolve(base);
        while let Some(Ty::Ref(_, i)) = t {
            t = Some(*i);
        }
        match t {
            Some(Ty::Struct(sid)) => match self.prog.field(sid, name) {
                Some((ft, _)) => self.known(ft),
                None => self.var(),
            },
            _ => self.var(),
        }
    }
    /// 呼び出し: 実引数を仮引数型と単一化して戻り値型を返す。skip はレシーバの分
    fn apply(&mut self, callee: Option<u32>, args: Vec<u32>, skip: usize) -> u32 {
        match callee {
            Some(id) => {
                let sig = &self.prog.sigs[id as usize];
                let (params, ret) = (sig.params[skip.min(sig.params.len())..].to_vec(), sig.ret);
                for (a, p) in args.into_iter().zip(params) {
                    let k = self.known(p);
                    self.unify(a, k);
                }
                self.known(ret)
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
                if let Some((sid, _)) = self.prog.variant_by_path(&pp.path, self.self_sid) {
                    let k = self.known(Ty::Struct(sid));
                    self.unify(v, k);
                }
            }
            Pat::TupleStruct(ts) => {
                let (v, mode) = self.auto_deref(v, mode);
                let Some((sid, vi)) = self.prog.variant_by_path(&ts.path, self.self_sid) else { return };
                let k = self.known(Ty::Struct(sid));
                self.unify(v, k);
                let fts: Vec<Ty> = self.prog.structs[sid as usize].variants[vi].fields.iter().map(|f| f.1).collect();
                let elems: Vec<&Pat> = ts.elems.iter().collect();
                let rest = elems.iter().position(|p| matches!(p, Pat::Rest(_)));
                for (i, p) in elems.iter().enumerate() {
                    let fi = match rest {
                        Some(r) if i == r => continue,
                        Some(r) if i > r => fts.len().wrapping_sub(elems.len() - i),
                        _ => i,
                    };
                    if let Some(&ft) = fts.get(fi) {
                        let k = self.known(ft);
                        self.pat(p, k, mode);
                    }
                }
            }
            Pat::Struct(ps) => {
                let (v, mode) = self.auto_deref(v, mode);
                let (sid, fields): (u32, Vec<(String, Ty)>) =
                    if let Some((sid, vi)) = self.prog.variant_by_path(&ps.path, self.self_sid) {
                        let f = &self.prog.structs[sid as usize].variants[vi].fields;
                        (sid, f.iter().map(|f| (f.0.clone(), f.1)).collect())
                    } else if let Some(sid) = self.prog.struct_by_path(&ps.path, self.self_sid) {
                        (sid, self.prog.structs[sid as usize].fields.iter().map(|f| (f.0.clone(), f.1)).collect())
                    } else {
                        return;
                    };
                let k = self.known(Ty::Struct(sid));
                self.unify(v, k);
                for fp in &ps.fields {
                    if let syn::Member::Named(id) = &fp.member {
                        if let Some((_, ft)) = fields.iter().find(|f| f.0 == id.to_string()) {
                            let k = self.known(*ft);
                            self.pat(&fp.pat, k, mode);
                        }
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
                        Pat::Type(pt) => (&*pt.pat, self.prog.parse_ty(&pt.ty, self.self_sid).ok()),
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

    fn expr(&mut self, e: &Expr) -> u32 {
        use BinOp::*;
        match e {
            Expr::Lit(l) => self.lit_var(&l.lit),
            Expr::Paren(p) => self.expr(&p.expr),
            Expr::Group(g) => self.expr(&g.expr),
            Expr::Block(b) => self.block(&b.block),
            Expr::Unsafe(u) => self.block(&u.block),
            Expr::Path(p) if p.path.segments.len() == 2 => match self.prog.variant_by_path(&p.path, self.self_sid) {
                Some((sid, _)) => self.known(Ty::Struct(sid)),
                None => self.var(),
            },
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
            Expr::Struct(s) if self.prog.variant_by_path(&s.path, self.self_sid).is_some() => {
                let (sid, vi) = self.prog.variant_by_path(&s.path, self.self_sid).unwrap();
                for fv in &s.fields {
                    let v = self.expr(&fv.expr);
                    if let syn::Member::Named(id) = &fv.member {
                        let ft = self.prog.structs[sid as usize].variants[vi]
                            .fields
                            .iter()
                            .find(|f| f.0 == id.to_string())
                            .map(|f| f.1);
                        if let Some(ft) = ft {
                            let k = self.known(ft);
                            self.unify(v, k);
                        }
                    }
                }
                self.known(Ty::Struct(sid))
            }
            Expr::Struct(s) => {
                let sid = self.prog.struct_by_path(&s.path, self.self_sid);
                for fv in &s.fields {
                    let v = self.expr(&fv.expr);
                    if let (Some(sid), syn::Member::Named(id)) = (sid, &fv.member) {
                        if let Some((ft, _)) = self.prog.field(sid, &id.to_string()) {
                            let k = self.known(ft);
                            self.unify(v, k);
                        }
                    }
                }
                match sid {
                    Some(sid) => self.known(Ty::Struct(sid)),
                    None => self.var(),
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
                        let fts: Vec<Ty> =
                            self.prog.structs[sid as usize].variants[vi].fields.iter().map(|f| f.1).collect();
                        for (a, ft) in c.args.iter().zip(fts) {
                            let v = self.expr(a);
                            let k = self.known(ft);
                            self.unify(v, k);
                        }
                        return self.known(Ty::Struct(sid));
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
                let mut t = self.resolve(r);
                while let Some(Ty::Ref(_, i)) = t {
                    t = Some(*i);
                }
                let callee = match t {
                    Some(Ty::Struct(sid)) => self.prog.method(sid, &mc.method.to_string()),
                    _ => None,
                };
                let args: Vec<u32> = mc.args.iter().map(|a| self.expr(a)).collect();
                self.apply(callee, args, 1)
            }
            Expr::Cast(c) => {
                self.expr(&c.expr);
                match self.prog.parse_ty(&c.ty, self.self_sid) {
                    Ok(t) => self.known(t),
                    Err(_) => self.var(),
                }
            }
            _ => self.var(),
        }
    }
}
