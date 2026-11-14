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
    parent: Vec<u32>,
    shape: Vec<Option<Shape>>,
    /// 整数リテラル由来 (決まらなければ i32 にしてよい) か
    int_lit: Vec<bool>,
    scopes: Vec<Vec<(String, u32)>>,
    lets: Vec<(usize, u32)>,
    ret: u32,
}

pub fn infer(prog: &Program, f: &syn::ItemFn, ret: Ty) -> LetTypes {
    let mut s = Inf {
        prog,
        parent: vec![],
        shape: vec![],
        int_lit: vec![],
        scopes: vec![vec![]],
        lets: vec![],
        ret: 0,
    };
    s.ret = s.known(ret);
    for a in &f.sig.inputs {
        if let syn::FnArg::Typed(pt) = a {
            if let (Pat::Ident(pi), Ok(t)) = (&*pt.pat, prog.parse_ty(&pt.ty)) {
                let v = s.known(t);
                s.scopes[0].push((pi.ident.to_string(), v));
            }
        }
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
                        Pat::Type(pt) => (&*pt.pat, self.prog.parse_ty(&pt.ty).ok()),
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
                    if let Pat::Ident(pi) = pat {
                        self.scopes.last_mut().unwrap().push((pi.ident.to_string(), v));
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
            Expr::Lit(l) => match &l.lit {
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
            },
            Expr::Paren(p) => self.expr(&p.expr),
            Expr::Group(g) => self.expr(&g.expr),
            Expr::Block(b) => self.block(&b.block),
            Expr::Unsafe(u) => self.block(&u.block),
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
                let sid = s.path.get_ident().and_then(|id| self.prog.struct_index.get(&id.to_string()).copied());
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
                self.expr(&i.cond);
                let t = self.block(&i.then_branch);
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
                self.expr(&w.cond);
                self.block(&w.body);
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
                let callee = match &*c.func {
                    Expr::Path(p) => p
                        .path
                        .get_ident()
                        .filter(|id| self.lookup(&id.to_string()).is_none())
                        .and_then(|id| self.prog.index.get(&id.to_string()).copied()),
                    _ => None,
                };
                let args: Vec<u32> = c.args.iter().map(|a| self.expr(a)).collect();
                match callee {
                    Some(id) => {
                        let sig = &self.prog.sigs[id as usize];
                        let (params, ret) = (sig.params.clone(), sig.ret);
                        for (a, p) in args.into_iter().zip(params) {
                            let k = self.known(p);
                            self.unify(a, k);
                        }
                        self.known(ret)
                    }
                    None => self.var(),
                }
            }
            Expr::Cast(c) => {
                self.expr(&c.expr);
                match self.prog.parse_ty(&c.ty) {
                    Ok(t) => self.known(t),
                    Err(_) => self.var(),
                }
            }
            _ => self.var(),
        }
    }
}
