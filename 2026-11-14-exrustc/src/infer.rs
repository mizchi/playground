//! 関数 1 個分の整数型推論 (union-find による単一化)。
//!
//! `let mut i = 0; while i < n { .. }` のように、注釈のない let の型を
//! 後続の使用箇所から決めるためのパス。型エラーの報告はここでは行わず
//! (矛盾は無視)、codegen 側の検査に任せる。結果は「注釈なし let → 型」の表。

use std::collections::HashMap;

use syn::{BinOp, Expr, Lit, Pat, Stmt};

use crate::front::{Program, Ty};

pub type LetTypes = HashMap<usize, Ty>;

pub fn let_key(l: &syn::Local) -> usize {
    l as *const syn::Local as usize
}

struct Inf<'p> {
    prog: &'p Program,
    parent: Vec<u32>,
    fixed: Vec<Option<Ty>>,
    scopes: Vec<Vec<(String, u32)>>,
    lets: Vec<(usize, u32)>,
    ret: u32,
}

pub fn infer(prog: &Program, f: &syn::ItemFn, ret: Ty) -> LetTypes {
    let mut s = Inf {
        prog,
        parent: vec![],
        fixed: vec![],
        scopes: vec![vec![]],
        lets: vec![],
        ret: 0,
    };
    s.ret = s.known(ret);
    for a in &f.sig.inputs {
        if let syn::FnArg::Typed(pt) = a {
            if let (Pat::Ident(pi), Ok(t)) = (&*pt.pat, Ty::parse(&pt.ty)) {
                let v = s.known(t);
                s.scopes[0].push((pi.ident.to_string(), v));
            }
        }
    }
    let tail = s.block(&f.block);
    let ret = s.ret;
    s.unify(tail, ret);
    let lets = std::mem::take(&mut s.lets);
    lets.into_iter()
        .map(|(k, v)| {
            let r = s.find(v);
            (k, s.fixed[r as usize].unwrap_or(Ty::I32))
        })
        .collect()
}

impl Inf<'_> {
    fn var(&mut self) -> u32 {
        self.parent.push(self.parent.len() as u32);
        self.fixed.push(None);
        self.parent.len() as u32 - 1
    }
    fn known(&mut self, t: Ty) -> u32 {
        let v = self.var();
        self.fixed[v as usize] = Some(t);
        v
    }
    fn find(&mut self, mut v: u32) -> u32 {
        while self.parent[v as usize] != v {
            let p = self.parent[v as usize];
            self.parent[v as usize] = self.parent[p as usize];
            v = p;
        }
        v
    }
    fn unify(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a == b {
            return;
        }
        let (fa, fb) = (self.fixed[a as usize], self.fixed[b as usize]);
        if fa == Some(Ty::Never) || fb == Some(Ty::Never) {
            return;
        }
        match (fa, fb) {
            (Some(x), Some(y)) if x != y => {} // 矛盾は codegen で報告
            (None, _) => self.parent[a as usize] = b,
            _ => self.parent[b as usize] = a,
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
                        Pat::Type(pt) => (&*pt.pat, Ty::parse(&pt.ty).ok()),
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
                    None => self.var(),
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
            Expr::Unary(u) => self.expr(&u.expr),
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
                    Shl(_) | Shr(_) | ShlAssign(_) | ShrAssign(_) => l,
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
                match Ty::parse(&c.ty) {
                    Ok(t) => self.known(t),
                    Err(_) => self.var(),
                }
            }
            _ => self.var(),
        }
    }
}
