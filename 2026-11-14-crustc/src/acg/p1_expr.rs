use super::*;
use crate::parse::{Block, Expr, GArg, Pat, Path, Stmt};
use crate::program::{FnSig, PPath, TyEnv};
use crate::types::{Ty, intern};
use clif::{IntCC, TrapCode, Value, types};

/// ラベルの名前 (syn の `l.ident` と同じく先頭の `'` を除く)
pub(crate) fn label_name(l: &str) -> String {
    l.trim_start_matches('\'').to_string()
}

/// クロージャ本体を普通の fn にする: `fn call(__env: (), a: _, __p1: _) { let (x, y) = __p1; 本体 }`。
/// (closure::synth と同じ。本体の型推論 (infer_ctx) に渡して、決まらない引数・戻り値の型を求めるのに使う)
pub(crate) fn closure_synth(c: &Expr) -> ItemFn {
    let Expr::Closure(_, inputs, _, body) = c else { unreachable!() };
    let mut params: Vec<(Pat, crate::parse::Ty)> = vec![(Pat::Ident(false, false, "__env".into(), None), crate::parse::Ty::Tuple(vec![]))];
    let mut stmts: Vec<Stmt> = vec![];
    for (i, (pat, ty)) in inputs.iter().enumerate() {
        let simple = matches!(pat, Pat::Ident(false, _, _, None));
        let pat_in = if simple {
            pat.clone()
        } else {
            let id = format!("__p{i}");
            stmts.push(Stmt::Let(pat.clone(), None, Some(path_expr(&id)), None));
            Pat::Ident(false, false, id, None)
        };
        params.push((pat_in, ty.clone().unwrap_or(crate::parse::Ty::Infer)));
    }
    match &**body {
        Expr::Block(None, false, b) => stmts.extend(b.stmts.iter().cloned()),
        e => stmts.push(Stmt::Expr(e.clone(), false)),
    }
    ItemFn {
        attrs: vec![],
        sig: crate::parse::FnSig {
            name: "call".into(),
            generics: crate::parse::Generics { params: vec![], preds: vec![] },
            recv: None,
            recv_ty: None,
            params,
            ret: None,
            quals: vec![],
        },
        block: Block { stmts },
    }
}

impl Cx<'_> {
    // --- place (左辺値) ---------------------------------------------------

    pub(crate) fn place(&mut self, e: &Expr) -> R<Place> {
        match e {
            Expr::Paren(p) => self.place(p),
            Expr::Path(..) if self.const_generic(e).is_some() => {
                let n = self.const_generic(e).unwrap();
                let v = self.b.ins().iconst(types::I64, n as i64);
                let var = self.b.declare_var(types::I64);
                self.b.def_var(var, v);
                Ok(Place::Var(var, Ty::Usize))
            }
            // const (`LIMIT` / `S::N`) を場所として使う (`println!("{}", LIMIT)` / `&LIMIT`): 一時領域に置く
            Expr::Path(_, p) if self.const_of(e).is_some() && ident_of(p).is_none_or(|id| self.lookup(id).is_none()) => {
                let ci = self.const_of(e).unwrap();
                let (v, t) = self.const_value(e, ci)?;
                match v {
                    Some(a) if t.is_agg() => Ok(Place::Mem(a, 0, t)),
                    Some(v) => {
                        let var = self.b.declare_var(clif_ty(t).unwrap_or(PTR));
                        self.b.def_var(var, v);
                        Ok(Place::Var(var, t))
                    }
                    None => Ok(Place::Zst(t)),
                }
            }
            Expr::Path(_, p) => {
                if let Some(q) = self.static_path(e) {
                    return self.place(&q);
                }
                let Some(id) = ident_of(p) else {
                    return err(SP, "unsupported path");
                };
                match self.lookup(id) {
                    Some((Loc::Var(v), t)) => Ok(Place::Var(v, t)),
                    Some((Loc::Zst, t)) => Ok(Place::Zst(t)),
                    Some((l, t)) => {
                        let a = self.loc_addr(l).unwrap();
                        Ok(Place::Mem(a, 0, t))
                    }
                    None => match self.prog.const_index.get(id) {
                        // static はデータのアドレス
                        Some(&ci) if self.prog.consts[ci as usize].stat.is_some() => {
                            let t = self.prog.consts[ci as usize].ty;
                            let a = self.static_sym(ci);
                            Ok(Place::Mem(a, 0, t))
                        }
                        // unit struct の値 (`struct U;` の `U`)
                        _ => match self.prog.struct_by_p(&PPath::of(p), self.self_sid) {
                            Some(sid)
                                if self.prog.structs[sid as usize].fields.is_empty()
                                    && !self.prog.structs[sid as usize].is_enum
                                    && self.prog.structs[sid as usize].generics.is_empty() =>
                            {
                                let t = Ty::Struct(sid);
                                Ok(Place::Mem(self.temp(t), 0, t))
                            }
                            // `use std::f64::consts::PI;` してからの `PI`
                            _ => match self.float_const_name(e) {
                                Some(x) => {
                                    let v = self.b.ins().f64const(x);
                                    let var = self.b.declare_var(types::F64);
                                    self.b.def_var(var, v);
                                    Ok(Place::Var(var, Ty::F64))
                                }
                                None => err(SP, format!("cannot find value `{id}`")),
                            },
                        },
                    },
                }
            }
            Expr::Field(base, fname) => {
                // (フィールドがなく Deref を実装した型 (`self.prog` で self: &Inf): `(*self.deref()).prog`)
                if let Some(Ty::Struct(sid)) = self.peek(base).map(|t| self.strip_refs(t)) {
                    if self.prog.field(sid, fname).is_none() {
                        if let Some(d) = self.user_deref_sid(base, sid) {
                            let f2 = self.keep_expr(Expr::Field(Box::new(Expr::Paren(Box::new(d))), fname.clone()));
                            return self.place(f2);
                        }
                    }
                }
                let (a, o, sid) = self.struct_base(base)?;
                match self.prog.field(sid, fname) {
                    Some((ft, foff)) => Ok(Place::Mem(a, o + foff as i32, ft)),
                    None => err(SP, format!("no field `{fname}` on type `{}`", self.prog.structs[sid as usize].name)),
                }
            }
            Expr::Unary(op, x) if op == "*" => {
                // `*g` (g: MutexGuard など Deref を実装した型): `*g.deref_mut()` / `*g.deref()`
                if let Some(d) = self.user_deref(x) {
                    let d = self.keep_expr(d);
                    return self.place(d);
                }
                // `*b` は b をムーブしない
                let (v, t) = self.read_no_move(x)?;
                match (v, t) {
                    (Some(_), t) if t.is_fat() => err(SP, "dereferencing a slice is not supported (index it instead)"),
                    (Some(p), Ty::Ref(_, inner) | Ty::Ptr(_, inner) | Ty::Box(inner)) => Ok(Place::Mem(p, 0, *inner)),
                    _ => err(SP, format!("type `{}` cannot be dereferenced", self.prog.tn(t))),
                }
            }
            // `map[&k]`: get(&k).unwrap() の指す先
            Expr::Index(b, k)
                if matches!(self.peek(b).map(|t| self.strip_refs(t)),
                    Some(Ty::Struct(sid)) if self.prog.structs[self.prog.base_of(sid) as usize].name == "HashMap") =>
            {
                let e = self.keep_expr(mcall(mcall(Expr::Paren(b.clone()), "get", vec![(**k).clone()]), "unwrap", vec![]));
                let (v, t) = self.expr(e, None)?;
                match (v, t) {
                    (Some(p), Ty::Ref(_, inner)) => Ok(Place::Mem(p, 0, *inner)),
                    _ => err(SP, "internal: bad map index"),
                }
            }
            // `d[i]` / `m[&k]` (impl Index / IndexMut、VecDeque / BTreeMap): `*x.index_mut(i)` / `*x.index(i)`
            Expr::Index(b, k)
                if matches!(self.peek_or_static(b).map(|t| self.strip_refs(t)),
                    Some(Ty::Struct(sid)) if self.vec_fields(sid).is_none()
                        && (self.prog.method(sid, "index").is_some() || self.prog.method(sid, "index_mut").is_some())) =>
            {
                let Some(Ty::Struct(sid)) = self.peek_or_static(b).map(|t| self.strip_refs(t)) else { unreachable!() };
                let m = if self.prog.method(sid, "index_mut").is_some() { "index_mut" } else { "index" };
                let e = self.keep_expr(mcall(Expr::Paren(b.clone()), m, vec![(**k).clone()]));
                let (v, t) = self.expr(e, None)?;
                match (v, t) {
                    (Some(p), Ty::Ref(_, inner)) => Ok(Place::Mem(p, 0, *inner)),
                    _ => err(SP, "`index` must return a reference"),
                }
            }
            Expr::Index(b, index) if !matches!(&**index, Expr::Range(..)) => {
                let (base, len, elem) = self.indexable(b)?;
                let (i, it) = self.expr(index, Some(Ty::Usize))?;
                let Some(i) = i else {
                    return err(SP, "index diverges");
                };
                self.same(SP, it, Ty::Usize)?;
                // (`for k in 0..3` の k で長さ 3 以上の配列を引くなら、範囲内と分かっている)
                let in_range = match (&**index, self.iconst_of(len)) {
                    (Expr::Path(_, p), Some(n)) => ident_of(p)
                        .and_then(|id| self.lookup(id))
                        .is_some_and(|(l, _)| self.loop_bounds.iter().any(|&(bl, h)| bl == l && h <= n as u64)),
                    _ => false,
                };
                if in_range {
                    let addr = self.elem_addr(base, i, elem);
                    return Ok(Place::Mem(addr, 0, elem));
                }
                // 範囲外なら panic (rustc と同じく境界検査する)
                let ok = self.b.ins().icmp(IntCC::UnsignedLessThan, i, len);
                // (rustc の場所: Vec など Index の impl を通るものは `[`、配列・スライスは式の先頭)
                // (場所 (Span) は今は使わないので、どちらも SP)
                let _via_impl = matches!(self.peek(b).map(|t| self.strip_refs(t)), Some(Ty::Struct(_)));
                let sp = SP;
                self.index_check(ok, len, i, sp);
                let addr = self.elem_addr(base, i, elem);
                Ok(Place::Mem(addr, 0, elem))
            }
            _ => err(SP, "expression is not assignable"),
        }
    }

    /// フィールドアクセスの土台: 自動 deref しながら struct のアドレスを求める
    pub(crate) fn struct_base(&mut self, base: &Expr) -> R<(Value, i32, u32)> {
        let mut p = if is_place_expr(base) {
            self.place(base)?
        } else {
            let r = self.expr(base, None)?;
            if let (Some(v), t) = r {
                self.own_temp(v, t);
            }
            match r {
                (Some(a), t) if t.is_agg() => Place::Mem(a, 0, t),
                (Some(p), Ty::Ref(_, i) | Ty::Box(i)) => Place::Mem(p, 0, *i),
                (_, t) => {
                    return err(SP, format!("type `{}` has no fields", self.prog.tn(t)));
                }
            }
        };
        loop {
            p = match p {
                Place::Mem(a, o, Ty::Struct(sid)) => return Ok((a, o, sid)),
                Place::Mem(a, o, Ty::Ref(_, i) | Ty::Box(i)) => {
                    let ptr = self.b.ins().load(PTR, mf(), a, o);
                    Place::Mem(ptr, 0, *i)
                }
                Place::Var(v, Ty::Ref(_, i) | Ty::Box(i)) => {
                    let ptr = self.b.use_var(v);
                    Place::Mem(ptr, 0, *i)
                }
                p => {
                    return err(SP, format!("type `{}` has no fields", self.prog.tn(p.ty())));
                }
            }
        }
    }

    // --- 式 -------------------------------------------------------------

    /// 式を評価し、期待型への暗黙の変換 (`&[T; N]` → `&[T]`) をする
    pub(crate) fn expr(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let (v, t) = self.expr_inner(e, expected).map_err(|m| {
            // (場所の代わりに、エラーになった一番内側の式を付ける)
            if m.contains(" (at `") || std::env::var_os("CRUSTC_NO_AT").is_some() {
                m
            } else {
                let d = format!("{e:?}");
                format!("{m} (at `{}`)", d.chars().take(160).collect::<String>())
            }
        })?;
        // `&Box<T>` → `&T` (deref の強制変換: `e = x;` で x: &Box<Expr>)
        if let (Some(want @ Ty::Ref(m2, we)), Some(val), Ty::Ref(m1, Ty::Box(bi))) = (expected, v, t) {
            if *we == **bi && !bi.is_fat() && (m1 || !m2) {
                let p = self.b.ins().load(PTR, mf(), val, 0);
                return Ok((Some(p), want));
            }
        }
        // `&T` / `&mut T` → `*const T` / `*mut T`
        if let (Some(want @ Ty::Ptr(m2, pe)), Ty::Ref(m1, re)) = (expected, t) {
            if pe == re && (m1 || !m2) {
                return Ok((v, want));
            }
        }
        if let (Some(want @ Ty::Ref(m2, Ty::Slice(we))), Some(v), Ty::Ref(m1, Ty::Array(ge, n))) = (expected, v, t) {
            if we == ge && (m1 || !m2) {
                let fat = self.make_fat(v, *n as i64);
                return Ok((Some(fat), want));
            }
        }
        // `&Vec<T>` → `&[T]`、`&String` → `&str`
        if let (Some(want @ Ty::Ref(m2, Ty::Slice(_) | Ty::Str)), Some(v), Ty::Ref(m1, Ty::Struct(sid))) = (expected, v, t) {
            if m1 || !m2 {
                if let Some((fat, ft)) = self.deref_to_slice(v, *sid, m2) {
                    let ok = match (want, ft) {
                        (Ty::Ref(_, Ty::Str), Ty::Ref(_, Ty::Str)) => true,
                        (Ty::Ref(_, Ty::Slice(we)), Ty::Ref(_, Ty::Slice(fe))) => fe == we,
                        _ => false,
                    };
                    if ok {
                        return Ok((Some(fat), want));
                    }
                }
            }
        }
        // `&mut T` → `&T` (`Box<T>` も `&mut T` なのでここで `&T` になる)
        if let (Some(want @ Ty::Ref(false, wi)), Ty::Ref(true, gi)) = (expected, t) {
            if wi == gi {
                return Ok((v, want));
            }
        }
        // `Box<T>` → `Box<dyn Trait>`
        if let (Some(want @ Ty::Box(Ty::Dyn(tr))), Some(v), Ty::Box(inner)) = (expected, v, t) {
            if !matches!(inner, Ty::Dyn(_)) {
                let fat = self.to_dyn(v, *inner, tr, SP)?;
                return Ok((Some(fat), want));
            }
        }
        // `&T` → `&dyn Trait` (データのポインタ + vtable)
        if let (Some(want @ Ty::Ref(m2, Ty::Dyn(tr))), Some(v), Ty::Ref(m1, inner)) = (expected, v, t) {
            if !matches!(inner, Ty::Dyn(_)) && (m1 || !m2) {
                let fat = self.to_dyn(v, *inner, tr, SP)?;
                return Ok((Some(fat), want));
            }
        }
        Ok((v, t))
    }

    pub(crate) fn expr_inner(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        // 期待型がなければ、型推論が後ろの使用から決めた型を使う
        // (syn 版の in_fmt_args のときの pos_key の引き直しは、自前の構文木にはないので省く)
        let expected = match expected {
            Some(t) => Some(t),
            None if matches!(e, Expr::Lit(_) | Expr::Struct(..) | Expr::Call(..)) => self.peek(e),
            None if matches!(e, Expr::Path(..)) => self.lets.get(&(e as *const Expr as usize)).copied(),
            None => None,
        };
        match e {
            Expr::Lit(l) => self.lit(l, expected),
            Expr::Paren(p) => self.expr(p, expected),
            Expr::Block(label, _, b) => {
                if let Some(l) = label {
                    return self.labeled_block(label_name(l), b, expected);
                }
                self.block(b, expected)
            }
            Expr::Path(_, p) if self.is_variant_path(p) || p.segs.len() >= 2 => {
                let Some((sid, vi)) = self.prog.variant_by_p(&PPath::of(p), self.self_sid) else {
                    return self.path_value(e, expected);
                };
                if self.prog.structs[sid as usize].variants[vi].kind != crate::types::VariantKind::Unit {
                    return err(SP, "this variant has fields: use `E::V(..)` or `E::V { .. }`");
                }
                // `None::<u32>`: 型引数を書いたバリアント
                let expected = expected.or_else(|| {
                    let args = p.segs.iter().rev().find_map(|s| if s.args.is_empty() { None } else { Some(&s.args) })?;
                    let mut ts = vec![];
                    for a in args {
                        if let GArg::Ty(t) = a {
                            ts.push(self.parse_ty(t).ok()?);
                        }
                    }
                    self.prog.instantiate(self.prog.base_of(sid), ts).ok().map(Ty::Struct)
                });
                self.variant_value(sid, vi, &[], expected)
            }
            Expr::Match(..) => self.match_(e, expected),
            // const ジェネリクスの N: インスタンスの型引数の長さ
            Expr::Path(..) if self.const_generic(e).is_some() => {
                let n = self.const_generic(e).unwrap();
                Ok((Some(self.b.ins().iconst(types::I64, n as i64)), Ty::Usize))
            }
            // const は式そのものを埋め込む
            Expr::Path(..) if self.const_of(e).is_some() => {
                let ci = self.const_of(e).unwrap();
                self.const_value(e, ci)
            }
            Expr::Path(..) if self.fn_item(e).is_some() => {
                let id = self.fn_item(e).unwrap();
                self.fn_value(e, id, expected)
            }
            Expr::Path(..) | Expr::Field(..) => {
                let p = self.place(e)?;
                let r = self.read(p);
                // drop が要る値を値として読んだらムーブ
                if self.needs_drop(r.1) {
                    let mut root = e;
                    while let Expr::Field(b, _) = strip_paren(root) {
                        root = b;
                    }
                    if let Expr::Path(_, rp) = strip_paren(root) {
                        if let Some(id) = ident_of(rp) {
                            if let Some(c) = self.caps.iter().find(|c| c.0 == id) {
                                // 値で持ったキャプチャ (FnOnce) から出す: 退避して環境の側を「空」(0) にする
                                // (環境はそのまま drop されるが、0 の Vec / String / Box は何も解放しない)
                                if let (false, Expr::Path(..), Place::Mem(a, o, t), Some(v)) = (c.2, strip_paren(e), p, r.0) {
                                    if self.zeroable(t) {
                                        let v = if t.is_agg() {
                                            let tmp = self.temp(t);
                                            self.copy(tmp, v, t);
                                            tmp
                                        } else {
                                            v
                                        };
                                        let dst = self.addr_plus(a, o);
                                        self.zero(dst, t);
                                        return Ok((Some(v), t));
                                    }
                                }
                                return err(SP, "moving a captured value out of a closure is not supported");
                            }
                        }
                    }
                    // 一時値のフィールドのムーブ (`f().0`): 一時値ごとの drop をやめ、残りのフィールドだけをここで drop する
                    // (Drop を実装した型からはフィールドをムーブできないので、フィールドごとに drop してよい)
                    if let (Expr::Field(..), Place::Mem(a, o, t), Some(v)) = (strip_paren(e), p, r.0) {
                        if let Some(tm) = self.temps.iter().rev().find(|tm| tm.loc == Loc::Ptr(a)).map(|tm| (tm.flag, tm.ty)) {
                            if let Ty::Struct(sid) = tm.1 {
                                let fields = self.prog.structs[sid as usize].fields.clone();
                                if !self.prog.structs[sid as usize].is_enum && fields.iter().any(|f| f.2 as i32 == o) {
                                    let v = if t.is_agg() {
                                        let tmp = self.temp(t);
                                        self.copy(tmp, v, t);
                                        tmp
                                    } else {
                                        v
                                    };
                                    self.clear_flag(tm.0);
                                    for (_, ft, off) in fields {
                                        if off as i32 != o && self.needs_drop(ft) {
                                            let fa = self.addr_plus(a, off as i32);
                                            self.drop_in_place(fa, ft);
                                        }
                                    }
                                    return Ok((Some(v), t));
                                }
                            }
                        }
                    }
                    if let (Expr::Field(..), Place::Mem(a, o, t), Some(v)) = (strip_paren(e), p, r.0) {
                        if self.zeroable(t) {
                            // フィールドだけのムーブ: 値を退避して元を「空」(0) にする。
                            // 持ち主はそのまま drop される (0 の Box は解放しない、0 の Option は None)
                            let v = if t.is_agg() {
                                let tmp = self.temp(t);
                                self.copy(tmp, v, t);
                                tmp
                            } else {
                                v
                            };
                            let dst = self.addr_plus(a, o);
                            self.zero(dst, t);
                            return Ok((Some(v), t));
                        }
                    }
                    self.mark_moved(e);
                }
                Ok(r)
            }
            Expr::Unary(op, x) if op == "*" => {
                let p = self.place(e)?;
                // `*b` (b: Box<T>、T は drop が要る): 中身のムーブ。b は drop しない
                let boxed = matches!(self.peek(x), Some(Ty::Box(_)));
                if boxed && self.needs_drop(p.ty()) {
                    self.mark_moved(e);
                }
                Ok(self.read(p))
            }
            Expr::Unary(..) => self.unary(e, expected),
            Expr::Ref(..) => self.reference(e, expected),
            Expr::Struct(..) => self.struct_lit(e, expected),
            Expr::Binary(..) | Expr::AssignOp(..) => self.binary(e, expected),
            Expr::Assign(left, right) => {
                let p = self.place(left)?;
                let t = p.ty();
                let (v, rt) = self.expr(right, Some(t))?;
                if rt == Ty::Never {
                    return Ok((None, Ty::Never));
                }
                self.same(SP, rt, t)?;
                let refill = if self.needs_drop(t) { self.drop_before_assign(left, p) } else { None };
                self.write(p, v);
                if let Some(f) = refill {
                    let one = self.b.ins().iconst(types::I8, 1);
                    self.b.ins().stack_store(one, f, 0);
                }
                Ok((None, Ty::Unit))
            }
            Expr::If(..) => self.if_(e, expected),
            Expr::While(_, cond, _) if matches!(&**cond, Expr::Let(..)) => self.while_let(e),
            Expr::While(label, cond, wbody) => {
                self.pending_label = label.as_deref().map(label_name);
                let header = self.b.create_block();
                let body = self.b.create_block();
                let exit = self.b.create_block();
                // -O: 条件と本体で変わらない添字の受け手は、ループの前で読んでおく
                let nh = self.hoisted.len();
                if self.prog.opt {
                    self.hoist_index_bases(wbody, None, Some(cond))?;
                }
                self.b.ins().jump(header, &[]);
                self.b.switch_to_block(header);
                let t0 = self.temps.len();
                let (c, ct) = self.expr(cond, Some(Ty::Bool))?;
                self.same(SP, ct, Ty::Bool)?;
                match c {
                    Some(c) => {
                        self.b.ins().brif(c, body, &[], exit, &[]);
                    }
                    None => {
                        self.b.ins().trap(TrapCode::unwrap_user(1));
                    }
                }
                self.b.switch_to_block(body);
                self.loops.push(Loop {
                    cont: header,
                    brk: exit,
                    breaks: 0,
                    label: self.pending_label.take(),
                    val: None,
                    is_block: false,
                    depth: self.scopes.len(),
                    temps: t0,
                });
                let (_, bt) = self.block(wbody, Some(Ty::Unit))?;
                self.loops.pop();
                if bt != Ty::Never {
                    self.same(SP, bt, Ty::Unit)?;
                    self.drop_for_exit(usize::MAX, t0);
                    self.b.ins().jump(header, &[]);
                } else {
                    self.b.ins().trap(TrapCode::unwrap_user(1));
                }
                self.b.switch_to_block(exit);
                self.drop_temps(t0);
                self.hoisted.truncate(nh);
                Ok((None, Ty::Unit))
            }
            Expr::Loop(label, lbody) => {
                self.pending_label = label.as_deref().map(label_name);
                let body = self.b.create_block();
                let exit = self.b.create_block();
                self.b.ins().jump(body, &[]);
                self.b.switch_to_block(body);
                self.loops.push(Loop {
                    cont: body,
                    brk: exit,
                    breaks: 0,
                    label: self.pending_label.take(),
                    val: None,
                    is_block: false,
                    depth: self.scopes.len(),
                    temps: self.temps.len(),
                });
                let (_, bt) = self.block(lbody, Some(Ty::Unit))?;
                let lp = self.loops.pop().unwrap();
                if bt != Ty::Never {
                    self.same(SP, bt, Ty::Unit)?;
                    self.drop_for_exit(usize::MAX, lp.temps);
                    self.b.ins().jump(body, &[]);
                } else {
                    self.b.ins().trap(TrapCode::unwrap_user(1));
                }
                self.b.switch_to_block(exit);
                if lp.breaks == 0 {
                    // break のない loop は値を返さない (型は !)
                    return Ok((None, Ty::Never));
                }
                // `break v` の値は exit の引数
                if let Some(t) = lp.val {
                    let v = self.b.block_params(exit).first().copied();
                    return Ok((v, t));
                }
                Ok((None, Ty::Unit))
            }
            Expr::Break(label, value) => {
                let label = label.as_deref().map(label_name);
                let Some(li) = self.loop_index(label.as_deref(), true) else {
                    return err(SP, "`break` outside of a loop (or unknown label)");
                };
                // `break v`: 値を評価してから抜ける (型は最初の break で決まる)
                let mut args = vec![];
                if let Some(e) = value {
                    let want = self.loops[li].val;
                    let (v, t) = self.expr(e, want)?;
                    if t == Ty::Never {
                        return Ok((None, Ty::Never));
                    }
                    match want {
                        Some(w) => self.same(SP, t, w)?,
                        None => {
                            let brk = self.loops[li].brk;
                            let ct = if t.is_agg() || t.is_fat() { PTR } else { clif_ty(t).unwrap_or(types::I8) };
                            if t != Ty::Unit {
                                self.b.append_block_param(brk, ct);
                            }
                            self.loops[li].val = Some(t);
                        }
                    }
                    if let Some(v) = v {
                        args.push(v);
                    }
                }
                let lp = &mut self.loops[li];
                lp.breaks += 1;
                let (t, d, tt) = (lp.brk, lp.depth, lp.temps);
                self.drop_for_exit(d, tt);
                let args: Vec<Value> = args.into_iter().map(|v| v.into()).collect();
                self.b.ins().jump(t, &args);
                Ok(self.dead())
            }
            Expr::Continue(label) => {
                let label = label.as_deref().map(label_name);
                let Some(li) = self.loop_index(label.as_deref(), false) else {
                    return err(SP, "`continue` outside of a loop (or unknown label)");
                };
                let lp = &self.loops[li];
                let (t, d, tt) = (lp.cont, lp.depth, lp.temps);
                self.drop_for_exit(d, tt);
                self.b.ins().jump(t, &[]);
                Ok(self.dead())
            }
            Expr::Return(value) => {
                let ret = self.ret;
                let v = match value {
                    Some(e) => {
                        let (v, t) = self.expr(e, Some(ret))?;
                        if t == Ty::Never {
                            return Ok((None, Ty::Never));
                        }
                        self.same(SP, t, ret)?;
                        v
                    }
                    None => {
                        self.same(SP, Ty::Unit, ret)?;
                        None
                    }
                };
                self.drop_for_exit(0, 0);
                self.emit_return(v);
                Ok(self.dead())
            }
            Expr::Call(..) => self.call(e, expected),
            Expr::Closure(..) => self.closure_value(e, expected),
            Expr::Range(..) => self.range_value(e, expected),
            Expr::Tuple(t) if t.is_empty() => Ok((None, Ty::Unit)),
            Expr::Tuple(_) => self.tuple_lit(e, expected),
            Expr::Array(_) => self.array_lit(e, expected),
            Expr::Repeat(..) => self.repeat_lit(e, expected),
            Expr::Index(_, ix) if matches!(&**ix, Expr::Range(..)) => err(SP, "range indexing needs a reference: `&a[lo..hi]`"),
            Expr::Index(..) => {
                let p = self.place(e)?;
                Ok(self.read(p))
            }
            Expr::For(..) => self.for_(e),
            Expr::MethodCall(..) => self.method_call(e, expected),
            Expr::Cast(x, ty) => {
                let to = self.parse_ty(ty)?;
                // `&x as &dyn Trait` は期待型つきの評価 (強制変換) と同じ
                if let Ty::Ref(_, Ty::Dyn(_)) = to {
                    let (v, t) = self.expr(x, Some(to))?;
                    self.same(SP, t, to)?;
                    return Ok((v, t));
                }
                let hint = self.peek(x).or(match &**x {
                    Expr::Lit(l) if matches!(lit::lit(l), Some(Lit::Float(_))) => Some(Ty::F64),
                    _ => Some(Ty::I32),
                });
                let (v, from) = self.expr(x, hint)?;
                if from == Ty::Never {
                    return Ok((None, Ty::Never));
                }
                // 参照 → 生ポインタ、生ポインタ ⇔ 生ポインタ / usize
                match (from, to) {
                    (Ty::Ref(_, a), Ty::Ptr(_, b)) if a == b || matches!(*a, Ty::Array(e, _) if e == b) => {
                        return Ok((v, to));
                    }
                    (Ty::Ptr(..), Ty::Ptr(..)) => return Ok((v, to)),
                    (Ty::Ptr(..), t) if matches!(t, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
                        return Ok((v, to));
                    }
                    // 関数ポインタのアドレス (`f as usize`)
                    (Ty::Fn(_), t) if matches!(t, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
                        return Ok((v, to));
                    }
                    (t, Ty::Ptr(..)) if matches!(t, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
                        return Ok((v, to));
                    }
                    _ => {}
                }
                // フィールドのない enum → 整数: タグ (= 判別値) を読む
                if let Ty::Struct(sid) = from {
                    let sd = &self.prog.structs[sid as usize];
                    if to.is_int() && sd.is_enum && sd.variants.iter().all(|v| v.fields.is_empty()) {
                        let tag = sd.tag;
                        let t = self.b.ins().load(clif_ty(tag).unwrap(), mf(), v.unwrap(), 0);
                        return Ok((Some(self.convert(t, tag, to)), to));
                    }
                }
                // 浮動小数点が関わるキャスト (Rust の `as` と同じく、浮動小数点 → 整数は飽和し NaN は 0)
                if from.is_float() || to.is_float() {
                    let v = v.unwrap();
                    let span = SP;
                    // 128 bit 整数 ⇔ 浮動小数点は prelude の関数 (Cranelift にない)
                    if from.bits() == 128 && to.is_float() {
                        let sg = self.b.ins().iconst(types::I8, from.signed() as i64);
                        let single = self.b.ins().iconst(types::I8, (to == Ty::F32) as i64);
                        let (r, _) =
                            self.call_named("__exr_u128_to_f", vec![Ty::Unit], &[(v, Ty::U128), (sg, Ty::Bool), (single, Ty::Bool)], span)?;
                        let r = r.unwrap();
                        let r = if to == Ty::F32 { self.b.ins().fdemote(types::F32, r) } else { r };
                        return Ok((Some(r), to));
                    }
                    if from.is_float() && to.bits() == 128 {
                        let w = if from == Ty::F32 { self.b.ins().fpromote(types::F64, v) } else { v };
                        let sg = self.b.ins().iconst(types::I8, to.signed() as i64);
                        let (r, _) = self.call_named("__exr_f_to_u128", vec![Ty::Unit], &[(w, Ty::F64), (sg, Ty::Bool)], span)?;
                        return Ok((r, to));
                    }
                    let r = match (from.is_float(), to.is_float()) {
                        (true, true) if from == to => v,
                        (true, true) if to == Ty::F64 => self.b.ins().fpromote(types::F64, v),
                        (true, true) => self.b.ins().fdemote(types::F32, v),
                        (false, true) if from.is_int() => {
                            let w = if from.bits() < 32 { self.convert(v, from, if from.signed() { Ty::I32 } else { Ty::U32 }) } else { v };
                            let ft = clif_ty(to).unwrap();
                            if from.signed() { self.b.ins().fcvt_from_sint(ft, w) } else { self.b.ins().fcvt_from_uint(ft, w) }
                        }
                        (true, false) if to.is_int() => {
                            // 64 bit に飽和させてから、狭い型は範囲に切り詰める
                            let wide =
                                if to.signed() { self.b.ins().fcvt_to_sint_sat(types::I64, v) } else { self.b.ins().fcvt_to_uint_sat(types::I64, v) };
                            if to.bits() == 64 {
                                wide
                            } else {
                                let (lo, hi): (i64, i64) = match to {
                                    Ty::I8 => (i8::MIN as i64, i8::MAX as i64),
                                    Ty::I16 => (i16::MIN as i64, i16::MAX as i64),
                                    Ty::I32 => (i32::MIN as i64, i32::MAX as i64),
                                    Ty::U8 => (0, u8::MAX as i64),
                                    Ty::U16 => (0, u16::MAX as i64),
                                    _ => (0, u32::MAX as i64),
                                };
                                let c = if to.signed() {
                                    let a = self.b.ins().iconst(types::I64, lo);
                                    let m = self.b.ins().smax(wide, a);
                                    let b2 = self.b.ins().iconst(types::I64, hi);
                                    self.b.ins().smin(m, b2)
                                } else {
                                    let b2 = self.b.ins().iconst(types::I64, hi);
                                    self.b.ins().umin(wide, b2)
                                };
                                self.b.ins().ireduce(clif_ty(to).unwrap(), c)
                            }
                        }
                        _ => {
                            return err(SP, format!("unsupported cast `{}` as `{}`", self.prog.tn(from), self.prog.tn(to)));
                        }
                    };
                    return Ok((Some(r), to));
                }
                // 整数 ⇔ char / bool → 整数 (char には u8 からだけ)
                let ok = (to.is_int() && (from.is_int() || matches!(from, Ty::Bool | Ty::Char))) || (to == Ty::Char && matches!(from, Ty::U8 | Ty::Char));
                if !ok {
                    return err(SP, format!("unsupported cast `{}` as `{}`", self.prog.tn(from), self.prog.tn(to)));
                }
                Ok((Some(self.convert(v.unwrap(), from, to)), to))
            }
            Expr::Macro(p, g) => self.macro_(p, g, expected),
            Expr::Try(_) => self.try_(e),
            _ => err(SP, "unsupported expression"),
        }
    }

    pub(crate) fn reference(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Ref(m, rexpr) = e else { return err(SP, "internal: expected a reference") };
        let m = *m;
        // `&a[lo..hi]` → スライス (fat pointer)
        if let Expr::Index(b, ix) = &**rexpr {
            if let Expr::Range(..) = &**ix {
                return self.slice_range(b, ix, m);
            }
        }
        let fn_item = matches!(&**rexpr, Expr::Path(..) if self.fn_item(rexpr).is_some());
        // (`&None` などのバリアント・定数の名前は場所でなく一時値)
        let not_var = |e: &Expr| {
            matches!(e, Expr::Path(_, p) if ident_of(p).is_some_and(|n| {
                self.lookup(n).is_none() && !self.prog.const_index.contains_key(n)
            }))
        };
        let inner = match &**rexpr {
            Expr::Paren(p) => &**p,
            e => e,
        };
        if is_place_expr(rexpr) && !fn_item && !not_var(inner) {
            return match self.place(rexpr)? {
                // `&mut v[i]` (v: Vec<Box<T>>) を `&mut T` の位置に: Box の中身を指す (deref の強制変換)
                Place::Mem(a, o, Ty::Box(x)) if matches!(expected, Some(Ty::Ref(_, e)) if *e == *x) && !x.is_fat() => {
                    let p = self.b.ins().load(PTR, mf(), a, o);
                    Ok((Some(p), Ty::Ref(m, x)))
                }
                Place::Mem(a, o, t) => Ok((Some(self.addr_plus(a, o)), Ty::Ref(m, intern(t)))),
                Place::Zst(t) => {
                    let a = self.temp(t);
                    Ok((Some(a), Ty::Ref(m, intern(t))))
                }
                Place::Var(..) => err(SP, "internal: borrowed local is not in memory"),
            };
        }
        // 一時値への参照: 一時領域に置いてそのアドレスを取る
        let inner_exp = match expected {
            Some(Ty::Ref(_, i)) => Some(*i),
            _ => None,
        };
        // 定数式への共有参照は静的データに昇格する (`fn f() -> &'static S { &S { x: 1 } }`)
        if !m && is_promotable(self, rexpr) {
            let t = inner_exp.filter(|t| !matches!(t, Ty::Dyn(_) | Ty::Slice(_) | Ty::Str)).or_else(|| self.peek(rexpr));
            if let Some(t) = t.filter(|&t| !self.needs_drop(t) && !self.prog.has_param(t)) {
                if let Ok(dv) = crate::ast_consteval::eval(self.prog, rexpr, t) {
                    if dv.relocs.is_empty() && !dv.bytes.is_empty() {
                        let a = self.data_addr(dv.bytes);
                        return Ok((Some(a), Ty::Ref(false, intern(t))));
                    }
                }
            }
        }
        let (v, t) = self.expr(rexpr, inner_exp)?;
        if t == Ty::Never {
            return Ok((None, Ty::Never));
        }
        let a = match (v, t) {
            (Some(a), t) if t.is_agg() => a,
            (v, t) => {
                let a = self.temp(t);
                if let Some(v) = v {
                    self.b.ins().store(mf(), v, a, 0);
                }
                a
            }
        };
        // `&一時値`: 一時値は文 (let ならブロック) の終わりまで所有する
        self.own_temp_at(a, t);
        Ok((Some(a), Ty::Ref(m, intern(t))))
    }

    pub(crate) fn struct_lit(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Struct(path, sfields, rest) = e else { return err(SP, "internal: expected a struct literal") };
        if let Some((sid, vi)) = self.prog.variant_by_p(&PPath::of(path), self.self_sid) {
            let v = &self.prog.structs[sid as usize].variants[vi];
            if v.kind != crate::types::VariantKind::Struct {
                return err(SP, "this variant is not a struct-like variant");
            }
            let mut args = vec![];
            for (fid, fe) in sfields {
                // (タプルの番号 `0: x` は名前でない)
                if fid.parse::<u32>().is_ok() {
                    return err(SP, "expected a field name");
                }
                let Some(fi) = v.fields.iter().position(|f| f.0 == *fid) else {
                    return err(SP, format!("variant `{}` has no field named `{fid}`", v.name));
                };
                if args.iter().any(|&(i, _)| i == fi) {
                    return err(SP, format!("field `{fid}` specified more than once"));
                }
                args.push((fi, fe));
            }
            if args.len() != v.fields.len() || rest.is_some() {
                return err(SP, format!("missing fields in initializer of `{}`", v.name));
            }
            return self.variant_value(sid, vi, &args, expected);
        }
        let Some(sid) = self.prog.struct_by_p(&PPath::of(path), self.self_sid) else {
            return err(SP, "cannot find struct");
        };
        // `S { a: 1, ..base }`: base を一時変数に置き、書かれていないフィールドをそこからムーブする
        // (書いたフィールドの元の値は、一時変数のスコープの終わりで drop される)
        if let Some(base) = rest {
            let st = match expected {
                Some(Ty::Struct(x)) if self.prog.base_of(x) == self.prog.base_of(sid) => Some(Ty::Struct(x)),
                _ if self.prog.structs[sid as usize].generics.is_empty() => Some(Ty::Struct(sid)),
                _ => None,
            };
            self.scopes.push(vec![]);
            let (bv, bt) = self.expr(base, st)?;
            if bt == Ty::Never {
                self.scopes.pop();
                return Ok((None, Ty::Never));
            }
            self.bind("__exr_base".into(), bt, bv);
            let Ty::Struct(bsid) = bt else {
                return err(SP, "the base of `..` must be a struct");
            };
            let all: Vec<String> = self.prog.structs[bsid as usize].fields.iter().map(|f| f.0.clone()).collect();
            let mut nfields = sfields.clone();
            for f in all {
                let given = sfields.iter().any(|fv| fv.0 == f);
                if !given {
                    nfields.push((f.clone(), Expr::Field(Box::new(path_expr("__exr_base")), f)));
                }
            }
            let ns = self.keep_expr(Expr::Struct(path.clone(), nfields, None));
            let r = self.struct_lit(ns, Some(bt))?;
            self.pop_scope();
            return Ok(r);
        }
        let fields = self.prog.structs[sid as usize].fields.clone();
        let mut args = vec![];
        for (name, fe) in sfields {
            let Some(fi) = fields.iter().position(|f| f.0 == *name) else {
                return err(SP, format!("struct `{}` has no field named `{name}`", self.prog.structs[sid as usize].name));
            };
            if args.iter().any(|&(i, _)| i == fi) {
                return err(SP, format!("field `{name}` specified more than once"));
            }
            args.push((fi, fe));
        }
        if args.len() != fields.len() {
            return err(SP, format!("missing fields in initializer of `{}`", self.prog.structs[sid as usize].name));
        }
        self.adt_value(sid, None, &args, expected, SP)
    }

    /// `|..| ..`: キャプチャを struct に詰めた値。型引数 (キャプチャ・引数・戻り値の型) をここで決める
    pub(crate) fn closure_value(&mut self, c: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Closure(_, inputs, _, _) = c else { return err(SP, "internal: expected a closure") };
        let prog = self.prog;
        let hint = self.closure_hint.take();
        let Some(&k) = self.closure_ix.get(&format!("{c:?}")) else {
            return err(SP, "internal: unknown closure");
        };
        let Some(&cid) = prog.closure_by.get(&(self.root, k)) else {
            return err(SP, "internal: unregistered closure");
        };
        let cd = &prog.closures[&cid];
        let sig0 = &prog.sigs[cid as usize];
        let ng = sig0.generics.len();
        let mut sol: Vec<Option<Ty>> = vec![None; ng];
        for (i, s) in sol.iter_mut().enumerate().take(cd.n_parent) {
            *s = self.env.params.get(i).map(|p| p.1);
        }
        // 期待型がこのクロージャの型そのもの (戻り値位置の `impl Fn`) なら型引数ごと使う
        let exp_args = match expected {
            Some(Ty::Struct(sid)) if prog.base_of(sid) == cd.sid => Some(prog.structs[sid as usize].args.clone()),
            _ => None,
        };
        // 期待型が `dyn Fn(A) -> B` なら引数と戻り値の手がかりにする
        let hint = hint.or_else(|| {
            let s = match expected {
                Some(Ty::Dyn(s)) => s,
                Some(Ty::Ref(_, Ty::Dyn(s))) => *s,
                _ => return None,
            };
            self.dyn_fn_sig(s).ok().map(|(ps, r)| (ps, Some(r)))
        });
        let mut caps = vec![];
        for (i, n) in cd.captures.iter().enumerate() {
            let Some((loc, t)) = self.lookup(n) else {
                return err(SP, format!("cannot capture `{n}` here"));
            };
            let ct = if cd.moved.get(i).copied().unwrap_or(false) { t } else { Ty::Ptr(true, intern(t)) };
            sol[cd.n_parent + i] = Some(ct);
            caps.push((loc, t));
        }
        let mut g = cd.n_parent + cd.captures.len();
        let mut pslots = vec![];
        for (i, p) in inputs.iter().enumerate() {
            if p.1.is_some() {
                continue;
            }
            pslots.push((i, g));
            // 呼び先の境界 → 親の型推論 (`let f = |x| x; f(1u8)` の使われ方) の順
            sol[g] = hint
                .as_ref()
                .and_then(|h| h.0.get(i).copied())
                .filter(|&t| !prog.has_param(t))
                .or_else(|| self.lets.get(&(&p.0 as *const Pat as usize)).copied());
            g += 1;
        }
        // 戻り値: 注釈 → 手がかり → 本体の型推論
        let ri = ng - 1;
        sol[ri] = cd
            .ret_ann
            .map(|t| prog.partial_subst(t, &sol))
            .or(hint.as_ref().and_then(|h| h.1).filter(|&t| !prog.has_param(t)))
            .or_else(|| self.lets.get(&(c as *const Expr as usize)).copied().filter(|&t| !prog.has_param(t)))
            .or(hint.as_ref().and_then(|h| h.1));
        if let Some(a) = exp_args {
            for (s, a) in sol.iter_mut().zip(a) {
                *s = Some(a);
            }
        }
        // 一部だけ分かっている戻り値型 (`Result<?, u8>`) は型推論の手がかりにする
        let partial_ret = sol[ri].filter(|&t| prog.has_param(t));
        if partial_ret.is_some() {
            sol[ri] = None;
        }
        // 決まらない引数・戻り値は本体の型推論で (`let f = |x| x + k;`)
        if sol.iter().any(|s| s.is_none()) {
            let f = closure_synth(c);
            let sig = FnSig { params: sig0.params.iter().map(|&t| prog.partial_subst(t, &sol)).collect(), ret: Ty::Unit, ..sig0.clone() };
            let env = TyEnv {
                self_sid: self.env.self_sid,
                params: sig0.generics.iter().zip(&sol).filter_map(|(g, s)| Some((g.0.clone(), (*s)?))).collect(),
                self_ty: self.env.self_ty,
            };
            let extra: Vec<(String, Ty)> = cd.captures.iter().zip(&caps).map(|(n, c)| (n.clone(), c.1)).collect();
            let (_, r, ps) = crate::ast_infer::infer_ctx(
                prog,
                false,
                &f.sig.params,
                &f.block,
                &sig,
                &env,
                &extra,
                sol[ri].or(partial_ret),
                Some((self.root, k + 1)),
            );
            if sol[ri].is_none() {
                sol[ri] = r;
            }
            for &(i, g) in &pslots {
                if sol[g].is_none() {
                    sol[g] = ps.get(i + 1).copied().flatten();
                }
            }
            if let Some(&(_i, _)) = pslots.iter().find(|(_, g)| sol[*g].is_none()) {
                return err(SP, "cannot infer the type of this closure parameter; add a type annotation");
            }
        }
        let Some(args) = sol.into_iter().collect::<Option<Vec<Ty>>>() else {
            return err(SP, "cannot infer the return type of this closure; annotate it (`|..| -> T { .. }`)");
        };
        let sid = prog.instantiate(cd.sid, args.clone()).or_else(|e| err(SP, e))?;
        // 呼ばれなくても本体を検査・コンパイルする (未使用なら --gc-sections で消える)
        {
            let mut g = self.insts.lock().unwrap();
            let iid = g.request(prog, cid, args).or_else(|e| err(SP, e))?;
            drop(g);
            self.requested.push(iid);
        }
        let st = Ty::Struct(sid);
        let a = self.temp(st);
        let fields = prog.structs[sid as usize].fields.clone();
        for (i, ((loc, t), (_, ft, off))) in caps.into_iter().zip(fields).enumerate() {
            if cd.moved.get(i).copied().unwrap_or(false) {
                let p = match loc {
                    Loc::Var(v) => Place::Var(v, t),
                    Loc::Zst => Place::Zst(t),
                    l => Place::Mem(self.loc_addr(l).unwrap(), 0, t),
                };
                let (v, _) = self.read(p);
                self.write(Place::Mem(a, off as i32, ft), v);
                if self.needs_drop(t) {
                    self.mark_moved_loc(loc);
                }
            } else {
                let addr = match loc {
                    Loc::Zst => self.b.ins().iconst(PTR, 0),
                    Loc::Var(_) => {
                        return err(SP, "internal: captured variable is not in memory");
                    }
                    l => self.loc_addr(l).unwrap(),
                };
                self.b.ins().store(mf(), addr, a, off as i32);
            }
        }
        Ok((Some(a), st))
    }

    /// `&T` (アドレス v) を `&dyn tr` の fat pointer にする
    pub(crate) fn to_dyn(&mut self, v: Value, t: Ty, tr: &str, span: Sp) -> R<Value> {
        let mut fns = vec![];
        if let (Some((_, bps, _)), Ty::Fn(f)) = (crate::ast_summary::parse_fn_bound(tr), t) {
            // 関数ポインタ: データ (関数ポインタを置いた場所) から呼ぶ prelude の中継関数
            if bps.len() != f.params.len() {
                return err(span, format!("expected a function with {} arguments", bps.len()));
            }
            let Some(&shim) = self.prog.index.get(&format!("__exr_fnptr{}", f.params.len())) else {
                return err(span, "fn pointers with more than 3 arguments cannot be used as `dyn Fn`");
            };
            let args: Vec<Ty> = f.params.iter().copied().chain([f.ret]).collect();
            let mut g = self.insts.lock().unwrap();
            let iid = g.request(self.prog, shim, args).or_else(|e| err(span, e))?;
            drop(g);
            self.requested.push(iid);
            fns.push(iid);
        } else if crate::ast_summary::parse_fn_bound(tr).is_some() {
            let cid = match t {
                Ty::Struct(sid) => self.prog.closure_of_struct.get(&self.prog.base_of(sid)).copied(),
                _ => None,
            };
            let Some(cid) = cid else {
                return err(span, format!("`{}` is not a closure", self.prog.tn(t)));
            };
            let Ty::Struct(sid) = t else { unreachable!() };
            let args = self.prog.structs[sid as usize].args.clone();
            let mut g = self.insts.lock().unwrap();
            let iid = g.request(self.prog, cid, args).or_else(|e| err(span, e))?;
            drop(g);
            self.requested.push(iid);
            fns.push(iid);
        } else {
            let Some(decls) = self.prog.traits.get(tr) else {
                return err(span, format!("cannot find trait `{tr}`"));
            };
            if !self.prog.implements(t, tr) {
                return err(span, format!("the trait `{tr}` is not implemented for `{}`", self.prog.tn(t)));
            }
            for &d in decls {
                let ds = &self.prog.sigs[d as usize];
                let Some(Ty::Ref(m, _)) = ds.params.first() else {
                    return err(span, format!("`{}` cannot be called through `dyn` (it needs `&self`)", self.prog.names[d as usize]));
                };
                let recv = [Ty::Ref(*m, intern(t))];
                let sol = vec![None; ds.generics.len()];
                let (mut fid, _) = self.resolve_call(d, &recv, sol, None, span)?;
                // ジェネリックな impl (`impl<T> Tr for W<T>`) のメソッドはさらにインスタンス化する
                if (fid as usize) < self.prog.sigs.len() && self.prog.sigs[fid as usize].is_generic() {
                    let g = self.prog.sigs[fid as usize].generics.len();
                    let mut sol = vec![None; g];
                    if let Some(st) = self.prog.sigs[fid as usize].self_ty {
                        self.prog.match_ty(st, t, &mut sol);
                    }
                    fid = self.resolve_call(fid, &recv, sol, None, span)?.0;
                }
                fns.push(fid);
            }
        }
        // 最後の欄は drop glue (`Box<dyn ..>` の drop で使う)
        let de = self.vtable_drop_entry(t);
        fns.push(de);
        let k = match self.vtables.iter().position(|x| *x == fns) {
            Some(k) => k,
            None => {
                self.vtables.push(fns);
                self.vtables.len() - 1
            }
        };
        let vt = self.vtable_sym(k as u32);
        let fat = self.temp(Ty::Ref(false, intern(Ty::Dyn(crate::types::intern_str(tr)))));
        self.b.ins().store(mf(), v, fat, 0);
        self.b.ins().store(mf(), vt, fat, 8);
        Ok(fat)
    }

    /// vtable の idx 番目を呼ぶ。fat は `&dyn` の (データ, vtable) のアドレス
    pub(crate) fn dyn_call(&mut self, fat: Value, idx: usize, params: &[Ty], ret: Ty, args: &[&Expr], span: Sp) -> R<V> {
        self.indirect_call(Indirect::Vtable(fat, idx), params, ret, args, span)
    }

    /// 関数ポインタ / vtable 経由の呼び出し
    pub(crate) fn indirect_call(&mut self, callee: Indirect, params: &[Ty], ret: Ty, args: &[&Expr], span: Sp) -> R<V> {
        if params.len() != args.len() {
            return err(span, format!("expected {} arguments, found {}", params.len(), args.len()));
        }
        let mut sig = self.prog.sigs[0].clone();
        let env = matches!(callee, Indirect::Vtable(..));
        sig.params = env.then_some(Ty::Ptr(false, intern(Ty::U8))).into_iter().chain(params.iter().copied()).collect();
        sig.ret = ret;
        sig.is_main = false;
        sig.is_extern = false;
        let data = match callee {
            Indirect::Vtable(fat, _) => Some(self.b.ins().load(PTR, mf(), fat, 0)),
            Indirect::Ptr(_) => None,
        };
        let mut vals = vec![];
        for (a, &pt) in args.iter().zip(params) {
            let (mut v, mut t) = self.expr(a, Some(pt))?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            // (`&Box<T>` / `&&T` を `&T` の引数に: deref の強制変換)
            while let (Ty::Ref(_, _), Ty::Ref(m, inner @ (Ty::Ref(..) | Ty::Box(_))), Some(val)) = (pt, t, v) {
                if t == pt || self.prog.has_param(pt) {
                    break;
                }
                v = Some(if inner.is_fat() { val } else { self.b.ins().load(PTR, mf(), val, 0) });
                t = match inner {
                    Ty::Box(x) => Ty::Ref(m, x),
                    i => *i,
                };
            }
            self.same(SP, t, pt)?;
            vals.push(v);
        }
        let fp = match callee {
            Indirect::Vtable(fat, idx) => {
                let vt = self.b.ins().load(PTR, mf(), fat, 8);
                self.b.ins().load(PTR, mf(), vt, 8 * idx as i32)
            }
            Indirect::Ptr(fp) => fp,
        };
        let mut cv = vec![];
        let sret = if is_sret(&sig) {
            let a = self.temp(ret);
            cv.push(a);
            Some(a)
        } else {
            None
        };
        cv.extend(data);
        for (v, &pt) in vals.into_iter().zip(params) {
            if let Some(v) = v {
                self.push_arg(&mut cv, v, pt);
            }
        }
        let sr = self.b.import_signature(clif_sig(&sig));
        let call = self.b.ins().call_indirect(sr, fp, &cv);
        if ret == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(self.dead());
        }
        if let Some(a) = sret {
            return Ok((Some(a), ret));
        }
        let r = self.b.inst_results(call).first().copied();
        Ok((r, ret))
    }

    /// `&dyn Fn(A) -> B` の呼び出し (vtable の 0 番目がクロージャの本体)
    pub(crate) fn dyn_fn_call(&mut self, fat: Value, s: &str, args: &[&Expr], span: Sp) -> R<V> {
        let (ps, r) = self.dyn_fn_sig(s).or_else(|e| err(span, e))?;
        self.dyn_call(fat, 0, &ps, r, args, span)
    }

    /// `x.m(..)` (x: `&dyn Trait`)
    pub(crate) fn dyn_method_call(&mut self, fat: Value, tr: &str, mc: &Expr) -> R<V> {
        let Expr::MethodCall(_, name, _, margs) = mc else { return err(SP, "internal: expected a method call") };
        let decls = self.prog.traits.get(tr).cloned().unwrap_or_default();
        let Some(idx) = decls.iter().position(|&d| self.prog.names[d as usize].rsplit("::").next() == Some(name.as_str())) else {
            return err(SP, format!("no method named `{name}` found for `dyn {tr}`"));
        };
        let ds = &self.prog.sigs[decls[idx] as usize];
        // Self は引数・戻り値に出てこない (object safe) 前提。出てきたらエラー
        let ps: Vec<Ty> = ds.params[1..].to_vec();
        if ps.iter().chain([&ds.ret]).any(|&t| self.prog.has_param(t)) {
            return err(SP, format!("`{name}` cannot be called through `dyn {tr}` (it uses `Self` or generics)"));
        }
        let ret = ds.ret;
        let args: Vec<&Expr> = margs.iter().collect();
        self.dyn_call(fat, idx, &ps, ret, &args, SP)
    }

    /// 呼び先の引数の型 pt が `F` / `&F` / `&mut F` で、F に `Fn(A) -> B` の境界があれば (A, B)
    /// (呼び先のシグネチャの型パラメータを Param で含む)
    pub(crate) fn fn_bound_of(&self, sig: &FnSig, pt: Ty) -> Option<(Vec<Ty>, Ty)> {
        let pt = match pt {
            Ty::Ref(_, i) => *i,
            t => t,
        };
        let Ty::Param(g) = pt else { return None };
        let (_, bounds) = sig.generics.get(g as usize)?;
        let (_, ps, r) = bounds.iter().find_map(|b| crate::ast_summary::parse_fn_bound(b))?;
        let names: Vec<&str> = sig.generics.iter().map(|g| g.0.as_str()).collect();
        let ps = ps.iter().map(|t| self.prog.resolve_g(t, &names).ok()).collect::<Option<Vec<Ty>>>()?;
        Some((ps, self.prog.resolve_g(&r, &names).ok()?))
    }

    /// クロージャの型 (struct のインスタンス) なら、その引数と戻り値の型
    pub(crate) fn closure_sig(&self, t: Ty) -> Option<(Vec<Ty>, Ty)> {
        if let Ty::Fn(f) = t {
            return Some((f.params.clone(), f.ret));
        }
        let Ty::Struct(sid) = t else { return None };
        let cid = *self.prog.closure_of_struct.get(&self.prog.base_of(sid))?;
        let args = &self.prog.structs[sid as usize].args;
        let s0 = &self.prog.sigs[cid as usize];
        Some((s0.params[1..].iter().map(|&p| self.prog.subst(p, args)).collect(), self.prog.subst(s0.ret, args)))
    }

    /// 関連型の制約から型パラメータを解く (`I: Iterator<Item = T>` / `where Self: Iterator<Item = &T>`)。
    /// `collect::<Vec<_>>()` の `_` は `Self::Item` で埋める
    pub(crate) fn solve_assoc_bounds(&self, sig: &FnSig, sol: &mut [Option<Ty>]) {
        let names: Vec<&str> = sig.generics.iter().map(|g| g.0.as_str()).collect();
        for _ in 0..2 {
            for (g, (_, bounds)) in sig.generics.iter().enumerate() {
                let Some(gt) = sol[g] else { continue };
                if self.prog.has_param(gt) {
                    continue;
                }
                for b in bounds {
                    let Some((name, want)) = assoc_constraint(b) else { continue };
                    let Ok(want) = self.prog.resolve_g(&want, &names) else { continue };
                    if let Some(actual) = self.prog.assoc_of(gt, &name) {
                        self.prog.match_ty(want, actual, sol);
                    }
                }
            }
        }
        for g in 0..sol.len() {
            let Some(t) = sol[g] else { continue };
            if !has_hole(self.prog, t) || !sig.generics[g].1.iter().any(|b| b == "__FromIter") {
                continue;
            }
            let item = sol.first().copied().flatten().filter(|&s| !self.prog.has_param(s)).and_then(|s| self.prog.assoc_of(s, "Item"));
            if let Some(item) = item {
                let mut args: Vec<Ty> = (0..=crate::program::HOLE).map(Ty::Param).collect();
                args[crate::program::HOLE as usize] = item;
                sol[g] = Some(self.prog.subst(t, &args));
            }
        }
    }

    /// 値としての `a..b` / `a..=b`: prelude の Range / RangeInclusive
    pub(crate) fn range_value(&mut self, rg: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Range(start, end, incl) = rg else { return err(SP, "internal: expected a range") };
        let incl = *incl;
        // `(1..)`: 終わりのない範囲
        if let (Some(lo), None, false) = (start, end, incl) {
            let Some(&base) = self.prog.struct_index.get("RangeFrom") else {
                return err(SP, "ranges can only be used directly in `for` loops here");
            };
            let et = match expected {
                Some(Ty::Struct(sid)) if self.prog.base_of(sid) == base => self.prog.structs[sid as usize].args.first().copied(),
                _ => None,
            }
            .or_else(|| self.peek(lo));
            let (lv, lt) = self.expr(lo, et)?;
            let sid = self.prog.instantiate(base, vec![lt]).or_else(|e| err(SP, e))?;
            let st = Ty::Struct(sid);
            let a = self.temp(st);
            let (_, ft, off) = self.prog.structs[sid as usize].fields[0];
            self.write(Place::Mem(a, off as i32, ft), lv);
            return Ok((Some(a), st));
        }
        let name = if incl { "RangeInclusive" } else { "Range" };
        let (Some(lo), Some(hi)) = (start, end) else {
            return err(SP, "only `a..b` / `a..=b` / `a..` ranges can be used as values");
        };
        let Some(&base) = self.prog.struct_index.get(name) else {
            return err(SP, "ranges can only be used directly in `for` loops here");
        };
        let mut et = match expected {
            Some(Ty::Struct(sid)) if self.prog.base_of(sid) == base => self.prog.structs[sid as usize].args.first().copied(),
            _ => None,
        };
        if et.is_none() {
            et = self.peek(lo).or_else(|| self.peek(hi));
        }
        let (lv, lt) = self.expr(lo, et)?;
        let (hv, ht) = self.expr(hi, Some(lt))?;
        self.same(SP, ht, lt)?;
        let sid = self.prog.instantiate(base, vec![lt]).or_else(|e| err(SP, e))?;
        let st = Ty::Struct(sid);
        let a = self.temp(st);
        let fields = self.prog.structs[sid as usize].fields.clone();
        for (n, ft, off) in fields {
            let v = match n.as_str() {
                "start" => lv,
                "end" => hv,
                _ => Some(self.b.ins().iconst(types::I8, 0)),
            };
            self.write(Place::Mem(a, off as i32, ft), v);
        }
        Ok((Some(a), st))
    }

    /// `Fn(A, B) -> R` (dyn の中身) の引数と戻り値の型
    pub(crate) fn dyn_fn_sig(&self, s: &str) -> R<(Vec<Ty>, Ty)> {
        let Some((_, ps, r)) = crate::ast_summary::parse_fn_bound(s) else {
            return Err(format!("`dyn {s}` is not a function type"));
        };
        let ps = ps.iter().map(|t| self.prog.resolve_env(t, &self.env)).collect::<R<Vec<Ty>>>()?;
        Ok((ps, self.prog.resolve_env(&r, &self.env)?))
    }

    /// 関数でない値の呼び出し `f(x)`: クロージャ (の参照) か `&dyn Fn`
    pub(crate) fn call_value(&mut self, c: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Call(func, cargs) = c else { return err(SP, "internal: expected a call") };
        // 呼び出しはムーブしない (Fn / FnMut は参照で呼ぶ)。一時値なら文の終わりまで所有する
        let (v, mut t) = if is_place_expr(func) && !matches!(&**func, Expr::Path(..) if self.fn_item(func).is_some()) {
            let p = self.place(func)?;
            self.read(p)
        } else {
            let r = self.expr(func, None)?;
            if let (Some(v), t) = r {
                self.own_temp(v, t);
            }
            r
        };
        let Some(mut v) = v else {
            return err(SP, "expected a function");
        };
        loop {
            match t {
                Ty::Ref(_, Ty::Dyn(_)) | Ty::Box(Ty::Dyn(_)) => break,
                Ty::Ref(_, i) | Ty::Box(i) if i.is_agg() => t = *i,
                Ty::Ref(_, i) | Ty::Box(i) => {
                    v = self.b.ins().load(PTR, mf(), v, 0);
                    t = *i;
                }
                _ => break,
            }
        }
        let args: Vec<&Expr> = cargs.iter().collect();
        match t {
            Ty::Struct(sid) => {
                let Some(&cid) = self.prog.closure_of_struct.get(&self.prog.base_of(sid)) else {
                    return err(SP, format!("expected a function, found `{}`", self.prog.tn(t)));
                };
                let recv = (v, Ty::Ptr(true, intern(t)));
                self.call_fn(cid, Some(recv), &args, None, expected, SP)
            }
            Ty::Ref(_, Ty::Dyn(s)) | Ty::Box(Ty::Dyn(s)) => self.dyn_fn_call(v, s, &args, SP),
            Ty::Fn(f) => self.indirect_call(Indirect::Ptr(v), &f.params, f.ret, &args, SP),
            t => err(SP, format!("expected a function, found `{}`", self.prog.tn(t))),
        }
    }

    pub(crate) fn call(&mut self, c: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Call(func, cargs) = c else { return err(SP, "internal: expected a call") };
        if let Some(r) = self.default_call(c, expected)? {
            return Ok(r);
        }
        if let Some(e) = self.prim_method_call(c) {
            let e = self.keep_expr(e);
            return self.expr(e, expected);
        }
        if let Expr::Path(_, p) = &**func {
            if let Some((sid, vi)) = self.prog.variant_by_p(&PPath::of(p), self.self_sid) {
                let v = &self.prog.structs[sid as usize].variants[vi];
                if v.kind != crate::types::VariantKind::Tuple || v.fields.len() != cargs.len() {
                    return err(SP, format!("variant `{}` takes {} fields", v.name, v.fields.len()));
                }
                let args: Vec<(usize, &Expr)> = cargs.iter().enumerate().collect();
                return self.variant_value(sid, vi, &args, expected);
            }
            // タプル構造体の構築 `P(1, 2)`
            if let Some(sid) = self.prog.struct_by_p(&PPath::of(p), self.self_sid) {
                if self.lookup(&p.segs[0].name).is_none() && self.prog.fn_by_p(&PPath::of(p), self.self_sid).is_none() {
                    let n = self.prog.structs[sid as usize].fields.len();
                    if n != cargs.len() {
                        return err(SP, format!("this struct takes {n} fields"));
                    }
                    let args: Vec<(usize, &Expr)> = cargs.iter().enumerate().collect();
                    return self.adt_value(sid, None, &args, expected, SP);
                }
            }
        }
        if let Some(r) = self.intrinsic_call(c, expected)? {
            return Ok(r);
        }
        let Some((id, _)) = self.callee(func) else {
            // 値の呼び出し: クロージャ / `&dyn Fn`
            return self.call_value(c, expected);
        };
        let args: Vec<&Expr> = cargs.iter().collect();
        let tf: Option<&[GArg]> = match &**func {
            Expr::Path(_, p) => match p.segs.last() {
                Some(s) if !s.args.is_empty() => Some(&s.args[..]),
                _ => None,
            },
            _ => None,
        };
        // `Buf::<8>::new()` / `Vec::<i32>::new()`: 型のセグメントの型引数
        if let Expr::Path(_, p) = &**func {
            let n = p.segs.len();
            if n >= 2 && !p.segs[n - 2].args.is_empty() {
                let seg = p.segs[n - 2].clone();
                let ty = crate::parse::Ty::Path(None, Path { global: false, segs: vec![seg] });
                self.seg_self = Some(self.parse_ty(&ty)?);
            } else if n == 2 && self.prog.sigs[id as usize].self_ty.is_some_and(|t| self.prog.has_param(t)) {
                // `T::dec(r)` (T はインスタンスの中で具体的な型): ジェネリックな impl の型引数を Self から
                let seg = p.segs[0].clone();
                let ty = crate::parse::Ty::Path(None, Path { global: false, segs: vec![seg] });
                self.seg_self = self.parse_ty(&ty).ok().filter(|&t| !self.prog.has_param(t));
            }
        }
        // (期待型が無ければ型推論の記録から: `x = Tr::dec(r)?` の Self)
        let expected = expected.or_else(|| {
            let sig = &self.prog.sigs[id as usize];
            if sig.trait_of.is_some() && !sig.has_self {
                self.lets.get(&(c as *const Expr as usize)).copied().filter(|&t| !self.prog.has_param(t))
            } else {
                None
            }
        });
        let r = self.call_fn(id, None, &args, tf, expected, SP);
        self.seg_self = None;
        r
    }
}
