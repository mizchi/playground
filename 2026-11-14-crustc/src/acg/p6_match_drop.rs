use super::*;
use crate::parse::{Block as PBlock, Expr, Pat, Path};
use crate::types::{Ty, intern};
use clif::{AbiParam, Block, IntCC, Signature, StackSlot, TrapCode, Value, types};
use lit::Lit;

/// 式がこの種類のリテラルか
fn lit_kind(e: &Expr, f: fn(&Lit) -> bool) -> bool {
    matches!(e, Expr::Lit(t) if lit::lit(t).is_some_and(|l| f(&l)))
}

/// ループのラベル (syn の `l.name.ident` と同じく `'` を除く)
fn label_name(l: &Option<String>) -> Option<String> {
    l.as_ref().map(|s| s.trim_start_matches('\'').to_string())
}

impl Cx<'_> {
    // --- match / if let / while let --------------------------------------

    /// match 対象を place にする。値なら一時変数に置く。None は発散
    pub(crate) fn scrutinee(&mut self, e: &Expr) -> R<Option<Place>> {
        self.scrutinee_as(e, None)
    }

    /// `let (a, b): (Vec<i32>, Vec<i32>) = ..`: 型注釈を期待型として渡す
    pub(crate) fn scrutinee_as(&mut self, e: &Expr, expected: Option<Ty>) -> R<Option<Place>> {
        self.last_scrut = None;
        // `v[..]` / `a[1..]` (サイズ不定の [T] の場所): fat pointer の置き場所を `[T]` 型の place として渡す
        // (スライスパターンだけが受け付け、要素は値で束縛する)
        if let Expr::Index(_, ix) = strip_paren(e) {
            if matches!(&**ix, Expr::Range(..)) {
                let r = self.keep_expr(Expr::Ref(false, Box::new(e.clone())));
                let (v, t) = self.expr(r, None)?;
                let (Some(v), Ty::Ref(_, Ty::Slice(el))) = (v, t) else {
                    return err(SP, "unsupported range scrutinee");
                };
                return Ok(Some(Place::Mem(v, 0, Ty::Slice(el))));
            }
        }
        if is_place_expr(e) {
            return Ok(Some(self.place(e)?));
        }
        let (v, t) = self.expr(e, expected)?;
        // 一時値の match 対象は文の終わりまで所有する (パターンがムーブしたらフラグを下ろす)
        if let Some(v) = v {
            if t != Ty::Never {
                self.last_scrut = self.own_temp(v, t);
            }
        }
        Ok(match (v, t) {
            (_, Ty::Never) => None,
            (Some(a), t) if t.is_agg() => Some(Place::Mem(a, 0, t)),
            (Some(v), t) => {
                let var = self.b.declare_var(clif_ty(t).unwrap());
                self.b.def_var(var, v);
                Some(Place::Var(var, t))
            }
            (None, t) => Some(Place::Zst(t)),
        })
    }

    pub(crate) fn match_(&mut self, m: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Match(scrut, arms) = m else {
            return err(SP, "internal: match_ on a non-match expression");
        };
        let Some(place) = self.scrutinee(scrut)? else {
            return Ok((None, Ty::Never));
        };
        let sflag = self.last_scrut;
        let merge = self.b.create_block();
        let mut result_ty: Option<Ty> = None;
        let mut param: Option<Value> = None;
        // 各 arm を上から順に試す (判定 → 束縛 → ガード → 本体)
        for arm in arms {
            let next = self.b.create_block();
            self.pat_test(&arm.pat, place, next)?;
            self.scopes.push(vec![]);
            let ob = self.owned_binds;
            self.pat_bind(&arm.pat, place, BindMode::Move)?;
            if self.owned_binds > ob {
                self.release_outer(scrut, sflag);
            }
            if let Some(g) = &arm.guard {
                let (c, ct) = self.expr(g, Some(Ty::Bool))?;
                self.same(SP, ct, Ty::Bool)?;
                if let Some(c) = c {
                    self.require(c, next);
                }
            }
            let t0 = self.temps.len();
            let (v, t) = self.expr(&arm.body, result_ty.or(expected))?;
            self.drop_tail_temps(t0, (v, t));
            self.pop_scope();
            self.finish_arm(v, t, merge, &mut result_ty, &mut param, SP)?;
            self.b.switch_to_block(next);
        }
        // どの arm にも合わない (rustc が網羅性を保証するので到達しない)
        self.b.ins().trap(TrapCode::unwrap_user(2));
        self.b.switch_to_block(merge);
        Ok(match result_ty {
            None => (None, Ty::Never),
            Some(t) => (param, t),
        })
    }

    pub(crate) fn if_let(&mut self, i: &Expr, l: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::If(_, then_branch, else_branch) = i else {
            return err(SP, "internal: if_let on a non-if expression");
        };
        let Expr::Let(lpat, lexpr) = l else {
            return err(SP, "internal: if_let without `let`");
        };
        let Some(place) = self.scrutinee(lexpr)? else {
            return Ok((None, Ty::Never));
        };
        let sflag = self.last_scrut;
        let else_blk = self.b.create_block();
        let merge = self.b.create_block();
        let mut result_ty: Option<Ty> = None;
        let mut param: Option<Value> = None;
        self.pat_test(lpat, place, else_blk)?;
        self.scopes.push(vec![]);
        let ob = self.owned_binds;
        self.pat_bind(lpat, place, BindMode::Move)?;
        if self.owned_binds > ob {
            self.release_outer(lexpr, sflag);
        }
        let (tv, tt) = self.block(then_branch, expected)?;
        self.pop_scope();
        self.finish_arm(tv, tt, merge, &mut result_ty, &mut param, SP)?;
        self.b.switch_to_block(else_blk);
        let (ev, et) = match else_branch {
            Some(e) => self.expr(e, result_ty.or(expected))?,
            None => (None, Ty::Unit),
        };
        self.finish_arm(ev, et, merge, &mut result_ty, &mut param, SP)?;
        self.b.switch_to_block(merge);
        Ok(match result_ty {
            None => (None, Ty::Never),
            Some(t) => (param, t),
        })
    }

    pub(crate) fn while_let(&mut self, w: &Expr) -> R<V> {
        let Expr::While(label, cond, body) = w else {
            unreachable!()
        };
        self.pending_label = label_name(label);
        let Expr::Let(pat, e) = &**cond else {
            unreachable!()
        };
        self.while_let_parts(pat, e, body)
    }

    /// `while let pat = e { body }` (for ループのイテレータ版からも使う)
    pub(crate) fn while_let_parts(&mut self, pat: &Pat, e: &Expr, body: &PBlock) -> R<V> {
        let header = self.b.create_block();
        let exit = self.b.create_block();
        self.b.ins().jump(header, &[]);
        self.b.switch_to_block(header);
        // 毎回 match 対象を評価し直す (一時値は 1 回ごとに drop する)
        let t0 = self.temps.len();
        let Some(place) = self.scrutinee(e)? else {
            return Ok((None, Ty::Never));
        };
        let sflag = self.last_scrut;
        self.pat_test(pat, place, exit)?;
        self.scopes.push(vec![]);
        let ob = self.owned_binds;
        self.pat_bind(pat, place, BindMode::Move)?;
        if self.owned_binds > ob {
            self.release_outer(e, sflag);
        }
        self.loops.push(Loop {
            cont: header,
            brk: exit,
            breaks: 0,
            label: self.pending_label.take(),
            val: None,
            is_block: false,
            depth: self.scopes.len() - 1,
            temps: t0,
        });
        let (_, bt) = self.block(body, Some(Ty::Unit))?;
        self.loops.pop();
        self.pop_scope();
        if bt != Ty::Never {
            self.same(SP, bt, Ty::Unit)?;
            self.drop_for_exit(usize::MAX, t0);
            self.b.ins().jump(header, &[]);
        } else {
            self.b.ins().trap(TrapCode::unwrap_user(1));
        }
        self.b.switch_to_block(exit);
        self.drop_temps(t0);
        Ok((None, Ty::Unit))
    }

    pub(crate) fn no_variant_msg(&self, p: &Path) -> String {
        let segs: Vec<String> = p.segs.iter().map(|s| s.name.clone()).collect();
        match segs.as_slice() {
            [t, v] => format!("no variant named `{v}` in `{t}`"),
            _ => "unsupported path pattern (only enum variants)".into(),
        }
    }

    /// cond が偽なら fail へ。真なら新しいブロックで続ける
    pub(crate) fn require(&mut self, cond: Value, fail: Block) {
        let next = self.b.create_block();
        self.b.ins().brif(cond, next, &[], fail, &[]);
        self.b.switch_to_block(next);
    }

    /// 参照に対して参照でないパターンを当てるときは自動で deref し、束縛モードを Ref にする
    pub(crate) fn pat_deref(&mut self, mut p: Place, mut mode: BindMode) -> (Place, BindMode) {
        loop {
            let (ptr, m, inner) = match p {
                Place::Var(v, Ty::Ref(m, i)) => (self.b.use_var(v), m, *i),
                Place::Mem(a, o, Ty::Ref(m, i)) => (self.b.ins().load(PTR, mf(), a, o), m, *i),
                _ => return (p, mode),
            };
            mode = match mode {
                BindMode::Ref(false) => BindMode::Ref(false),
                _ => BindMode::Ref(m),
            };
            p = Place::Mem(ptr, 0, inner);
        }
    }

    pub(crate) fn field_place(p: Place, fty: Ty, off: u32) -> Place {
        match p {
            Place::Mem(a, o, _) => Place::Mem(a, o + off as i32, fty),
            _ => unreachable!("aggregates always live in memory"),
        }
    }

    /// パターンの判定値 (リテラル / 範囲の端) を place の型で作る
    pub(crate) fn pat_const(&mut self, e: &Expr, t: Ty) -> R<Value> {
        // (`i32::MIN..=-1` や const の名前は式として評価する)
        let Expr::Lit(l) = e else {
            let (v, vt) = self.expr(e, Some(t))?;
            self.same(SP, vt, t)?;
            return v.ok_or_else(|| "range pattern bound has no value".to_string());
        };
        let (v, vt) = self.lit(l, Some(t))?;
        self.same(SP, vt, t)?;
        Ok(v.unwrap())
    }

    /// enum のタグが vi のバリアントか調べる
    pub(crate) fn test_variant(&mut self, p: Place, sid: u32, vi: usize, span: Sp, fail: Block) -> R<()> {
        if p.ty() != Ty::Struct(sid) {
            let got = self.prog.tn(p.ty());
            return err(span, format!("mismatched types: expected `{got}`, found `{}`", self.prog.tn(Ty::Struct(sid))));
        }
        let sd = &self.prog.structs[sid as usize];
        if sd.variants.len() == 1 {
            return Ok(()); // バリアントが 1 つなら判定不要
        }
        let (tag, discr) = (sd.tag, sd.variants[vi].discr);
        let Place::Mem(a, o, _) = p else {
            unreachable!()
        };
        let tv = self.b.ins().load(clif_ty(tag).unwrap(), mf(), a, o);
        let d = self.b.ins().iconst(clif_ty(tag).unwrap(), discr);
        let c = self.b.ins().icmp(IntCC::Equal, tv, d);
        self.require(c, fail);
        Ok(())
    }

    /// パターン列 (`V(a, .., z)`) とフィールド列の対応 (`..` は途中を飛ばす)
    pub(crate) fn tuple_fields<'e>(elems: impl Iterator<Item = &'e Pat>, nfields: usize, span: Sp) -> R<Vec<(usize, &'e Pat)>> {
        let elems: Vec<&Pat> = elems.collect();
        let rest = elems.iter().position(|p| matches!(p, Pat::Rest));
        let n_named = elems.len() - rest.is_some() as usize;
        if n_named > nfields || (rest.is_none() && n_named != nfields) {
            return err(span, format!("this pattern has {n_named} fields, but the corresponding variant has {nfields}"));
        }
        let mut out = vec![];
        for (i, p) in elems.iter().enumerate() {
            match rest {
                Some(r) if i == r => {}
                Some(r) if i > r => out.push((nfields - (elems.len() - i), *p)),
                _ => out.push((i, *p)),
            }
        }
        Ok(out)
    }

    /// パターンの判定だけをする (合わなければ fail へ飛ぶ)。束縛は pat_bind で行う
    pub(crate) fn pat_test(&mut self, pat: &Pat, p: Place, fail: Block) -> R<()> {
        match pat {
            Pat::Wild | Pat::Rest => Ok(()),
            // `None` のように unit バリアントに解決される識別子は、束縛ではなくパスのパターン
            Pat::Ident(..) if unit_variant_ident(self.prog, pat).is_some() => {
                let path = unit_variant_ident(self.prog, pat).unwrap();
                let (p, _) = self.pat_deref(p, BindMode::Move);
                let Some((sid, vi)) = self.variant_on(&path, p.ty()) else {
                    unreachable!()
                };
                self.test_variant(p, sid, vi, SP, fail)
            }
            Pat::Ident(..) if const_lit_pat(self.prog, pat).is_some() => {
                let lp = const_lit_pat(self.prog, pat).unwrap();
                self.pat_test(&lp, p, fail)
            }
            Pat::Ident(_, _, _, sub) => match sub {
                Some(sub) => self.pat_test(sub, p, fail),
                None => Ok(()),
            },
            Pat::Ref(_, inner) => {
                let Ty::Ref(_, _) = p.ty() else {
                    return err(SP, format!("mismatched types: `{}` is not a reference", self.prog.tn(p.ty())));
                };
                let (q, _) = self.pat_deref_once(p);
                self.pat_test(inner, q, fail)
            }
            Pat::Slice(_) => {
                let (base, len, elem, _) = self.slice_scrutinee(p, SP)?;
                let (pre, rest, post) = slice_parts(pat);
                let fixed = (pre.len() + post.len()) as i64;
                let c = if rest.is_some() {
                    self.b.ins().icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, len, fixed)
                } else {
                    self.b.ins().icmp_imm_u(IntCC::Equal, len, fixed)
                };
                self.require(c, fail);
                for (q, sub) in self.slice_elems(base, len, elem, &pre, &post) {
                    self.pat_test(sub, q, fail)?;
                }
                Ok(())
            }
            Pat::Or(cases) => {
                let ok = self.b.create_block();
                let n = cases.len();
                for (i, case) in cases.iter().enumerate() {
                    let nf = if i + 1 == n { fail } else { self.b.create_block() };
                    self.pat_test(case, p, nf)?;
                    self.b.ins().jump(ok, &[]);
                    if i + 1 != n {
                        self.b.switch_to_block(nf);
                    }
                }
                self.b.switch_to_block(ok);
                Ok(())
            }
            // バイト列のパターン (`match w { b"pub" => .. }`、w: &[u8] / &[u8; N]): 長さと中身で比べる
            Pat::Lit(l) if lit_kind(l, |x| matches!(x, Lit::ByteStr(_))) => {
                let Expr::Lit(text) = &**l else { unreachable!() };
                let Some(Lit::ByteStr(bs)) = lit::lit(text) else { unreachable!() };
                let mut p = p;
                // (&&[u8] などは 1 段ずつ外し、&[u8] / &[u8; N] で止める)
                while let Ty::Ref(_, i) = p.ty() {
                    if matches!(i, Ty::Slice(_) | Ty::Array(..)) {
                        break;
                    }
                    let (v, _) = self.read(p);
                    p = Place::Mem(v.unwrap(), 0, *i);
                }
                let u8s = Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8))));
                let fat = match p.ty() {
                    Ty::Ref(_, Ty::Slice(Ty::U8)) => self.read(p).0.unwrap(),
                    Ty::Ref(_, Ty::Array(Ty::U8, n)) => {
                        let (v, _) = self.read(p);
                        let n = self.b.ins().iconst(types::I64, *n as i64);
                        self.make_fat_v(v.unwrap(), n)
                    }
                    t => return err(SP, format!("mismatched types: expected `{}`, found `&[u8]`", self.prog.tn(t))),
                };
                let (c, _) = self.lit(text, None)?;
                let n = self.b.ins().iconst(types::I64, bs.len() as i64);
                let cf = self.make_fat_v(c.unwrap(), n);
                let cond = self.eq_at(fat, cf, u8s, SP)?;
                self.require(cond, fail);
                Ok(())
            }
            // 文字列のパターン (`match s { "add" => .. }`): &str (と &&str) の中身で比べる
            Pat::Lit(l) if lit_kind(l, |x| matches!(x, Lit::Str(_))) => {
                let Expr::Lit(text) = &**l else { unreachable!() };
                let sref = Ty::Ref(false, intern(Ty::Str));
                let mut p = p;
                while let Ty::Ref(_, i) = p.ty() {
                    if p.ty() == sref || *i == Ty::Str {
                        break;
                    }
                    let (v, _) = self.read(p);
                    p = Place::Mem(v.unwrap(), 0, *i);
                }
                if !matches!(p.ty(), Ty::Ref(_, Ty::Str)) {
                    return err(SP, format!("mismatched types: expected `{}`, found `&str`", self.prog.tn(p.ty())));
                }
                let (v, _) = self.read(p);
                let (c, _) = self.lit(text, Some(sref))?;
                let cond = self.eq_at(v.unwrap(), c.unwrap(), sref, SP)?;
                self.require(cond, fail);
                Ok(())
            }
            _ => {
                let (p, _) = self.pat_deref(p, BindMode::Move);
                let t = p.ty();
                match pat {
                    Pat::Lit(l) => {
                        let (v, _) = self.read(p);
                        // (負のリテラル `-1` は式として評価する)
                        let (c, ct) = match &**l {
                            Expr::Lit(text) => self.lit(text, Some(t))?,
                            e => self.expr(e, Some(t))?,
                        };
                        self.same(SP, ct, t)?;
                        if !(t.is_int() || t == Ty::Bool || t == Ty::Char) {
                            return err(SP, "literal patterns need an integer, char, bool or string");
                        }
                        let cond = self.b.ins().icmp(IntCC::Equal, v.unwrap(), c.unwrap());
                        self.require(cond, fail);
                        Ok(())
                    }
                    Pat::Range(start, end, inclusive) => {
                        if !(t.is_int() || t == Ty::Char) {
                            return err(SP, "range patterns need an integer or char");
                        }
                        let (v, _) = self.read(p);
                        let v = v.unwrap();
                        let s = t.signed();
                        if let Some(lo) = start {
                            let lo = self.pat_const(lo, t)?;
                            let cc = if s { IntCC::SignedGreaterThanOrEqual } else { IntCC::UnsignedGreaterThanOrEqual };
                            let c = self.b.ins().icmp(cc, v, lo);
                            self.require(c, fail);
                        }
                        if let Some(hi) = end {
                            let hi = self.pat_const(hi, t)?;
                            let cc = match (*inclusive, s) {
                                (true, true) => IntCC::SignedLessThanOrEqual,
                                (true, false) => IntCC::UnsignedLessThanOrEqual,
                                (_, true) => IntCC::SignedLessThan,
                                (_, false) => IntCC::UnsignedLessThan,
                            };
                            let c = self.b.ins().icmp(cc, v, hi);
                            self.require(c, fail);
                        }
                        Ok(())
                    }
                    Pat::Path(path) => {
                        let Some((sid, vi)) = self.variant_on(path, t) else {
                            return err(SP, self.no_variant_msg(path));
                        };
                        self.test_variant(p, sid, vi, SP, fail)
                    }
                    Pat::TupleStruct(path, elems) => {
                        let fields = match self.variant_on(path, t) {
                            Some((sid, vi)) => {
                                self.test_variant(p, sid, vi, SP, fail)?;
                                self.prog.structs[sid as usize].variants[vi].fields.clone()
                            }
                            None => self.tuple_struct_fields(path, t, SP)?,
                        };
                        for (fi, sub) in Self::tuple_fields(elems.iter(), fields.len(), SP)? {
                            let (_, fty, off) = fields[fi];
                            self.pat_test(sub, Self::field_place(p, fty, off), fail)?;
                        }
                        Ok(())
                    }
                    Pat::Tuple(elems) => {
                        let fields = self.tuple_pat_fields(p, elems.len(), SP)?;
                        for (fi, sub) in Self::tuple_fields(elems.iter(), fields.len(), SP)? {
                            let (_, fty, off) = fields[fi];
                            self.pat_test(sub, Self::field_place(p, fty, off), fail)?;
                        }
                        Ok(())
                    }
                    Pat::Struct(..) => {
                        let fields = self.struct_pat_fields(pat, p, Some(fail))?;
                        for (sub, fp) in fields {
                            self.pat_test(sub, fp, fail)?;
                        }
                        Ok(())
                    }
                    _ => err(SP, "unsupported pattern"),
                }
            }
        }
    }

    /// struct パターン (`P(a, b)` / `S { .. }`) のフィールド。match 対象の型 t のインスタンスで引く
    pub(crate) fn tuple_struct_fields(&self, path: &Path, t: Ty, span: Sp) -> R<Vec<(String, Ty, u32)>> {
        let Some(sid) = self.prog.struct_by_p(&PPath::of(path), self.self_sid) else {
            return err(span, self.no_variant_msg(path));
        };
        match t {
            Ty::Struct(x) if self.prog.base_of(x) == self.prog.base_of(sid) => Ok(self.prog.structs[x as usize].fields.clone()),
            _ => err(span, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(t), self.prog.structs[sid as usize].name)),
        }
    }

    /// タプルパターンのフィールド (match 対象はタプルのインスタンス)
    pub(crate) fn tuple_pat_fields(&self, p: Place, n_pat: usize, span: Sp) -> R<Vec<(String, Ty, u32)>> {
        match p.ty() {
            Ty::Struct(x) if self.prog.is_tuple(x) => Ok(self.prog.structs[x as usize].fields.clone()),
            // `()` のパターン (`Ok(())`)
            Ty::Unit if n_pat == 0 => Ok(vec![]),
            t => err(span, format!("mismatched types: expected a tuple of {n_pat} elements, found `{}`", self.prog.tn(t))),
        }
    }

    pub(crate) fn pat_deref_once(&mut self, p: Place) -> (Place, bool) {
        match p {
            Place::Var(v, Ty::Ref(m, i)) => (Place::Mem(self.b.use_var(v), 0, *i), m),
            Place::Mem(a, o, Ty::Ref(m, i)) => (Place::Mem(self.b.ins().load(PTR, mf(), a, o), 0, *i), m),
            p => (p, false),
        }
    }

    /// `S { a, b: p, .. }` / `E::V { a, .. }` の各フィールドの (サブパターン, place)。
    /// fail があればバリアントの判定もする
    pub(crate) fn struct_pat_fields<'e>(&mut self, ps: &'e Pat, p: Place, fail: Option<Block>) -> R<Vec<(&'e Pat, Place)>> {
        let Pat::Struct(path, pfields, has_rest) = ps else {
            return err(SP, "internal: struct_pat_fields on a non-struct pattern");
        };
        let fields: Vec<(String, Ty, u32)> = if let Some((sid, vi)) = self.variant_on(path, p.ty()) {
            if let Some(fail) = fail {
                self.test_variant(p, sid, vi, SP, fail)?;
            }
            self.prog.structs[sid as usize].variants[vi].fields.clone()
        } else if self.prog.struct_by_p(&PPath::of(path), self.self_sid).is_some() {
            self.tuple_struct_fields(path, p.ty(), SP)?
        } else {
            return err(SP, "cannot find struct or variant");
        };
        let mut out = vec![];
        for (fid, fpat) in pfields {
            // (`S { 0: x }` の番号のフィールドは受け付けない)
            if fid.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return err(SP, "expected a field name");
            }
            let Some(&(_, fty, off)) = fields.iter().find(|f| f.0 == *fid) else {
                return err(SP, format!("no field `{fid}`"));
            };
            out.push((fpat, Self::field_place(p, fty, off)));
        }
        if !*has_rest && out.len() != fields.len() {
            return err(SP, "pattern does not mention all fields (add `..`)");
        }
        Ok(out)
    }

    /// 判定に通った後で、パターン中の名前を束縛する
    pub(crate) fn pat_bind(&mut self, pat: &Pat, p: Place, mode: BindMode) -> R<()> {
        match pat {
            Pat::Wild | Pat::Rest | Pat::Lit(_) | Pat::Range(..) | Pat::Path(_) => Ok(()),
            Pat::Ident(..) if unit_variant_ident(self.prog, pat).is_some() => Ok(()),
            Pat::Ident(..) if const_lit_pat(self.prog, pat).is_some() => Ok(()),
            Pat::Ident(by_ref, mutable, name, subpat) => {
                if let Some(sub) = subpat {
                    self.pat_bind(sub, p, mode)?;
                }
                let name = name.clone();
                let mode = if *by_ref { BindMode::Ref(*mutable) } else { mode };
                match mode {
                    BindMode::Move => {
                        let (v, t) = self.read(p);
                        self.bind(name, t, v);
                    }
                    BindMode::Ref(m) => {
                        let Place::Mem(a, o, t) = p else {
                            return err(SP, "cannot bind by reference here");
                        };
                        let addr = self.addr_plus(a, o);
                        self.bind(name, Ty::Ref(m, intern(t)), Some(addr));
                    }
                }
                Ok(())
            }
            Pat::Ref(_, inner) => {
                let (q, _) = self.pat_deref_once(p);
                self.pat_bind(inner, q, BindMode::Move)
            }
            Pat::Or(cases) => {
                if !cases.iter().any(has_bindings) {
                    return Ok(());
                }
                // どの選択肢に合ったかを判定し直し、その選択肢で束縛した値を merge の引数で渡す
                let mut names = vec![];
                crate::ast_closure::pat_idents(&cases[0], &mut names);
                let merge = self.b.create_block();
                let mut tys: Option<Vec<(String, Ty)>> = None;
                for case in cases {
                    let next = self.b.create_block();
                    self.pat_test(case, p, next)?;
                    let o0 = self.owned.len();
                    let ob = self.owned_binds;
                    self.scopes.push(vec![]);
                    self.pat_bind(case, p, mode)?;
                    let mut vals = vec![];
                    let mut tl = vec![];
                    for n in &names {
                        if let Some((loc, t)) = self.lookup(n) {
                            if let Some(v) = self.loc_arg(loc, t) {
                                vals.push(v);
                            }
                            tl.push((n.clone(), t));
                        }
                    }
                    self.scopes.pop();
                    self.owned.truncate(o0);
                    self.owned_binds = ob;
                    match &tys {
                        None => {
                            for (_, t) in &tl {
                                if let Some(ct) = self.param_ty(*t) {
                                    self.b.append_block_param(merge, ct);
                                }
                            }
                            tys = Some(tl);
                        }
                        Some(t0) => {
                            if t0.iter().map(|x| x.1).ne(tl.iter().map(|x| x.1)) {
                                return err(SP, "`|` alternatives bind different types");
                            }
                        }
                    }
                    let args: Vec<Value> = vals.into_iter().collect();
                    self.b.ins().jump(merge, &args);
                    self.b.switch_to_block(next);
                }
                self.b.ins().trap(TrapCode::unwrap_user(2));
                self.b.switch_to_block(merge);
                let params = self.b.block_params(merge).to_vec();
                let mut pi = params.into_iter();
                for (n, t) in tys.unwrap_or_default() {
                    let v = if self.param_ty(t).is_some() { pi.next() } else { None };
                    self.bind(n, t, v);
                }
                Ok(())
            }
            Pat::Slice(_) => {
                let (base, len, elem, rmode) = self.slice_scrutinee(p, SP)?;
                let mode = rmode.map(BindMode::Ref).unwrap_or(mode);
                let (pre, rest, post) = slice_parts(pat);
                for (q, sub) in self.slice_elems(base, len, elem, &pre, &post) {
                    self.pat_bind(sub, q, mode)?;
                }
                // `rest @ ..`: 残りの部分スライス (`&[T]`)
                if let Some(Pat::Ident(_, _, rname, _)) = rest {
                    let (es, _) = self.prog.size_align(elem);
                    let start = self.b.ins().iadd_imm_s(base, pre.len() as i64 * es as i64);
                    let n = self.b.ins().iadd_imm_s(len, -((pre.len() + post.len()) as i64));
                    let fat = self.make_fat_v(start, n);
                    let m = matches!(mode, BindMode::Ref(true));
                    self.bind(rname.clone(), Ty::Ref(m, intern(Ty::Slice(intern(elem)))), Some(fat));
                }
                Ok(())
            }
            Pat::TupleStruct(path, elems) => {
                let (p, mode) = self.pat_deref(p, mode);
                let fields = match self.variant_on(path, p.ty()) {
                    Some((sid, vi)) => self.prog.structs[sid as usize].variants[vi].fields.clone(),
                    None => self.tuple_struct_fields(path, p.ty(), SP)?,
                };
                for (fi, sub) in Self::tuple_fields(elems.iter(), fields.len(), SP)? {
                    let (_, fty, off) = fields[fi];
                    self.pat_bind(sub, Self::field_place(p, fty, off), mode)?;
                }
                Ok(())
            }
            Pat::Tuple(elems) => {
                let (p, mode) = self.pat_deref(p, mode);
                let fields = self.tuple_pat_fields(p, elems.len(), SP)?;
                for (fi, sub) in Self::tuple_fields(elems.iter(), fields.len(), SP)? {
                    let (_, fty, off) = fields[fi];
                    self.pat_bind(sub, Self::field_place(p, fty, off), mode)?;
                }
                Ok(())
            }
            Pat::Struct(..) => {
                let (p, mode) = self.pat_deref(p, mode);
                for (sub, fp) in self.struct_pat_fields(pat, p, None)? {
                    self.pat_bind(sub, fp, mode)?;
                }
                Ok(())
            }
            _ => err(SP, "unsupported pattern"),
        }
    }
}

// ---------------------------------------------------------------------------
// Drop
// ---------------------------------------------------------------------------
//
// - drop が要る型 ([`Program::needs_drop`]) の値を持つローカル変数・一時値は、所有者として
//   `owned` / `temps` に drop フラグ (スタック上の 1 byte) と一緒に登録する。
// - ムーブ (値としての読み出し・値で受ける self・move キャプチャ・値で束縛するパターン) でフラグを 0 にする。
//   フィールドだけのムーブ (部分ムーブ) も全体のフラグを 0 にする (残りはリークする)。
// - スコープの終わり・return / break / continue / `?` で、フラグが 1 のものを登録と逆順に drop する。
// - drop 本体は `__exr_drop_in_place::<T>` のインスタンス (drop glue)。codegen が型ごとに
//   1 段だけ展開した本体を作り、入れ子の型は呼び出しにする (再帰的な型でも展開が止まる)。
// - panic では drop しない (unwind しない)。

impl Cx<'_> {
    pub(crate) fn needs_drop(&self, t: Ty) -> bool {
        self.prog.has_drop && self.prog.needs_drop(t)
    }

    pub(crate) fn new_flag(&mut self) -> StackSlot {
        let ss = self.b.create_stack_slot(1, 1);
        // (入口のブロックは最初に作るブロック: Block(0))
        if self.b.current_block() != Some(Block(0)) {
            self.late_flags.push(ss);
        }
        let one = self.b.ins().iconst(types::I8, 1);
        self.b.ins().stack_store(one, ss, 0);
        ss
    }

    pub(crate) fn clear_flag(&mut self, flag: StackSlot) {
        let z = self.b.ins().iconst(types::I8, 0);
        self.b.ins().stack_store(z, flag, 0);
    }

    /// 今のスコープのローカル変数として所有する
    pub(crate) fn own_local(&mut self, loc: Loc, ty: Ty) {
        if !self.needs_drop(ty) {
            return;
        }
        let flag = self.new_flag();
        let depth = self.scopes.len() - 1;
        self.owned.push(Owned { loc, ty, flag, depth });
    }

    /// 一時値 v (集約型ならアドレス) を文の終わりまで所有する。drop が要らなければ None
    pub(crate) fn own_temp(&mut self, v: Value, ty: Ty) -> Option<StackSlot> {
        if !self.needs_drop(ty) {
            return None;
        }
        let loc = if ty.is_agg() {
            Loc::Ptr(v)
        } else {
            let ss = self.slot(ty);
            self.b.ins().stack_store(v, ss, 0);
            Loc::Slot(ss)
        };
        let flag = self.new_flag();
        self.temps.push(Owned { loc, ty, flag, depth: 0 });
        Some(flag)
    }

    /// 0 で埋めた状態が「drop しても何もしない値」になる型か (Box は null、enum は判別値 0 のバリアント)。
    /// `impl Drop` を含む型は 0 にすると drop が走ってしまうので不可
    pub(crate) fn zeroable(&self, t: Ty) -> bool {
        match t {
            Ty::Box(_) => true,
            Ty::Array(e, _) => !self.needs_drop(*e) || self.zeroable(*e),
            Ty::Struct(sid) => {
                // (prelude の Vec / String は 0 (null の ptr、長さ 0) でも drop が何もしない)
                let bname = &self.prog.structs[self.prog.base_of(sid) as usize].name;
                if bname == "Vec" || bname == "String" {
                    return true;
                }
                if self.prog.drop_impls.contains(&self.prog.base_of(sid)) {
                    return false;
                }
                let sd = &self.prog.structs[sid as usize];
                let ok = |fs: &[(String, Ty, u32)]| fs.iter().all(|f| !self.needs_drop(f.1) || self.zeroable(f.1));
                if sd.is_enum { sd.variants.iter().find(|v| v.discr == 0).is_some_and(|v| ok(&v.fields)) } else { ok(&sd.fields) }
            }
            _ => !self.needs_drop(t),
        }
    }

    pub(crate) fn zero(&mut self, dst: Value, t: Ty) {
        let (size, _) = self.prog.size_align(t);
        let z = self.b.ins().iconst(types::I8, 0);
        for i in 0..size {
            self.b.ins().store(mf(), z, dst, i as i32);
        }
    }

    /// prelude の Vec<T> のインスタンスなら (ptr のオフセット, len のオフセット, T)
    pub(crate) fn vec_fields(&self, sid: u32) -> Option<(u32, u32, Ty)> {
        let sd = &self.prog.structs[sid as usize];
        let b = sd.base?;
        if self.prog.structs[b as usize].name != "Vec" {
            return None;
        }
        let po = sd.fields.iter().find(|f| f.0 == "ptr")?.2;
        let lo = sd.fields.iter().find(|f| f.0 == "len")?.2;
        Some((po, lo, *sd.args.first()?))
    }

    /// `&Vec<T>` (アドレス a) → `&[T]`、`&String` → `&str` の fat pointer
    pub(crate) fn deref_to_slice(&mut self, a: Value, sid: u32, m: bool) -> Option<(Value, Ty)> {
        if let Some((po, lo, e)) = self.vec_fields(sid) {
            let p = self.b.ins().load(PTR, mf(), a, po as i32);
            let n = self.b.ins().load(types::I64, mf(), a, lo as i32);
            return Some((self.make_fat_v(p, n), Ty::Ref(m, intern(Ty::Slice(intern(e))))));
        }
        let sd = &self.prog.structs[sid as usize];
        if sd.base.is_none() && sd.name == "String" {
            let (_, vt, vo) = sd.fields.iter().find(|f| f.0 == "vec")?.clone();
            let Ty::Struct(vs) = vt else { return None };
            let va = self.addr_plus_v(a, vo);
            let (fat, _) = self.deref_to_slice(va, vs, m)?;
            return Some((fat, Ty::Ref(m, intern(Ty::Str))));
        }
        None
    }

    pub(crate) fn addr_plus_v(&mut self, a: Value, off: u32) -> Value {
        self.addr_plus(a, off as i32)
    }

    /// prelude の関数 (ジェネリックなら型引数 targs のインスタンス) を、評価済みの引数で呼ぶ
    /// 受け手の struct の型引数で impl のメソッド (`Rc::<T>::deref`) を値の引数で呼ぶ
    pub(crate) fn call_fn_vals(&mut self, id: u32, args: &[(Value, Ty)], span: Sp) -> R<V> {
        let targs = match args.first().map(|a| self.strip_refs(a.1)) {
            Some(Ty::Struct(sid)) if !self.prog.sigs[id as usize].generics.is_empty() => self.prog.structs[sid as usize].args.clone(),
            _ => vec![],
        };
        let name = self.prog.names[id as usize].clone();
        self.call_named(&name, targs, args, span)
    }

    pub(crate) fn call_named(&mut self, name: &str, targs: Vec<Ty>, args: &[(Value, Ty)], span: Sp) -> R<V> {
        let Some(&id) = self.prog.index.get(name) else {
            return err(span, format!("internal: `{name}` is missing from the prelude"));
        };
        let iid = if targs.is_empty() {
            id
        } else {
            let mut g = self.insts.lock().unwrap();
            let i = g.request(self.prog, id, targs).or_else(|e| err(span, e))?;
            drop(g);
            self.requested.push(i);
            i
        };
        let sig = self.insts.lock().unwrap().sig(self.prog, iid).clone();
        let mut cv = vec![];
        let sret = if is_sret(&sig) {
            let a = self.temp(sig.ret);
            cv.push(a);
            Some(a)
        } else {
            None
        };
        for (&(v, t), &pt) in args.iter().zip(&sig.params) {
            self.same(span, t, pt)?;
            self.push_arg(&mut cv, v, pt);
        }
        let f = self.fref(iid, &sig);
        let call = self.b.ins().call(f, &cv);
        if let Some(a) = sret {
            return Ok((Some(a), sig.ret));
        }
        let r = self.b.inst_results(call).first().copied();
        Ok((r, sig.ret))
    }

    /// 二項演算の被演算子。比較 (cmp) ならムーブせずに読み、一時値は所有する
    pub(crate) fn operand(&mut self, e: &Expr, hint: Option<Ty>, cmp: bool) -> R<V> {
        if !cmp {
            return self.expr(e, hint);
        }
        // `a[..2] == [1, 2]`: サイズ不定のスライスは参照で比べる (`[T]` の PartialEq の &self)
        if let Expr::Index(_, ix) = strip_paren(e) {
            if matches!(&**ix, Expr::Range(..)) {
                let r = self.keep_expr(Expr::Ref(false, Box::new(e.clone())));
                return self.expr(r, None);
            }
        }
        // (`None` などローカル変数でない名前は値として評価する)
        let non_local = matches!(strip_paren(e), Expr::Path(_, p)
            if ident_of(p).is_some_and(|id| self.lookup(id).is_none()));
        if is_place_expr(e) && !non_local {
            let r = self.read_no_move(e)?;
            if self.needs_drop(r.1) || hint.is_none() {
                return Ok(r);
            }
            // (整数リテラルの型合わせなど、期待型の変換が要る場合は通常の評価)
            if hint == Some(r.1) {
                return Ok(r);
            }
            return self.expr(e, hint);
        }
        let r = self.expr(e, hint)?;
        if let (Some(v), t) = r {
            if t != Ty::Never {
                self.own_temp(v, t);
            }
        }
        Ok(r)
    }

    pub(crate) fn strip_refs(&self, mut t: Ty) -> Ty {
        while let Ty::Ref(_, i) = t {
            t = *i;
        }
        t
    }

    /// 値を読むがムーブはしない (deref・呼び出しの対象など)
    pub(crate) fn read_no_move(&mut self, e: &Expr) -> R<V> {
        let se = strip_paren(e);
        if is_place_expr(e) && !(matches!(se, Expr::Path(..)) && (self.fn_item(se).is_some() || self.const_of(se).is_some())) {
            let p = self.place(e)?;
            Ok(self.read(p))
        } else {
            self.expr(e, None)
        }
    }

    /// アドレス a にある一時値を所有する
    pub(crate) fn own_temp_at(&mut self, a: Value, ty: Ty) {
        if !self.needs_drop(ty) {
            return;
        }
        let flag = self.new_flag();
        self.temps.push(Owned { loc: Loc::Ptr(a), ty, flag, depth: 0 });
    }

    /// ローカル変数 (の場所) の drop フラグ
    pub(crate) fn flag_of_loc(&self, loc: Loc) -> Option<StackSlot> {
        self.owned.iter().rev().find(|o| o.loc == loc).map(|o| o.flag)
    }

    /// mark_moved が下ろすフラグ (下ろさずに返す)
    pub(crate) fn moved_flag(&self, e: &Expr) -> Option<StackSlot> {
        let mut e = e;
        loop {
            match e {
                Expr::Paren(p) => e = p,
                Expr::Field(b, _) => e = b,
                Expr::Unary(op, x) if op == "*" => e = x,
                Expr::Path(_, p) => {
                    let (loc, t) = self.lookup(ident_of(p)?)?;
                    if matches!(t, Ty::Ref(..) | Ty::Ptr(..)) {
                        return None;
                    }
                    return self.flag_of_loc(loc);
                }
                _ => return None,
            }
        }
    }

    /// 式 e (ローカル変数か、そのフィールド) から値をムーブした: 変数のフラグを下ろす
    pub(crate) fn mark_moved(&mut self, e: &Expr) {
        let mut e = e;
        let mut via_field = false;
        loop {
            match e {
                Expr::Paren(p) => e = p,
                // 参照越しのフィールドからはムーブできない (Copy 以外は rustc が弾く)
                Expr::Field(b, _) => {
                    via_field = true;
                    e = b
                }
                // `*b` (b: Box<T>) から中身をムーブした: b も drop しない (Box の領域はリークする)
                Expr::Unary(op, x) if op == "*" => {
                    via_field = true;
                    e = x
                }
                Expr::Path(_, p) => {
                    let Some(id) = ident_of(p) else { return };
                    if let Some((loc, t)) = self.lookup(id) {
                        if matches!(t, Ty::Ref(..) | Ty::Ptr(..)) {
                            return;
                        }
                        // (Box の中身のフィールドをムーブしたときも Box ごとムーブ済みにする: 領域はリークする)
                        let _ = via_field;
                        if let Some(f) = self.flag_of_loc(loc) {
                            self.clear_flag(f);
                        }
                    }
                    return;
                }
                _ => return,
            }
        }
    }

    /// パターンが match 対象 e から値でムーブした (flag は e が一時値のときのフラグ)
    pub(crate) fn release(&mut self, e: &Expr, flag: Option<StackSlot>) {
        if is_place_expr(e) {
            self.mark_moved(e);
        } else if let Some(f) = flag {
            self.clear_flag(f);
        }
    }

    /// パターンの束縛のスコープ (いちばん内側) を除いて release する:
    /// `let nn = ..; if let Some(nn) = nn { .. }` の match 対象 `nn` は外側の変数 (内側の束縛と同じ名前)
    pub(crate) fn release_outer(&mut self, e: &Expr, flag: Option<StackSlot>) {
        let top = self.scopes.pop();
        self.release(e, flag);
        if let Some(t) = top {
            self.scopes.push(t);
        }
    }

    /// 代入で上書きする前に古い値を drop する。ローカル変数ならフラグを見て、代入後に立て直すフラグを返す
    pub(crate) fn drop_before_assign(&mut self, left: &Expr, p: Place) -> Option<StackSlot> {
        if let Expr::Path(_, pp) = strip_paren(left) {
            if let Some(id) = ident_of(pp) {
                if let Some((loc, ty)) = self.lookup(id) {
                    if let Some(flag) = self.flag_of_loc(loc) {
                        self.emit_owned_drop(Owned { loc, ty, flag, depth: 0 });
                        return Some(flag);
                    }
                }
            }
        }
        // フィールド・`*p`・`a[i]`: 初期化済みなので必ず drop する
        let addr = match p {
            Place::Mem(a, o, _) => self.addr_plus(a, o),
            Place::Var(v, t) => {
                let x = self.b.use_var(v);
                let ss = self.slot(t);
                self.b.ins().stack_store(x, ss, 0);
                self.b.ins().stack_addr(PTR, ss, 0)
            }
            Place::Zst(_) => return None,
        };
        self.drop_in_place(addr, p.ty());
        None
    }

    pub(crate) fn mark_moved_loc(&mut self, loc: Loc) {
        if let Some(f) = self.flag_of_loc(loc) {
            self.clear_flag(f);
        }
    }

    /// フラグが立っていれば drop する
    pub(crate) fn emit_owned_drop(&mut self, o: Owned) {
        let f = self.b.ins().stack_load(types::I8, o.flag, 0);
        let yes = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().brif(f, yes, &[], done, &[]);
        self.b.switch_to_block(yes);
        self.clear_flag(o.flag);
        let addr = match o.loc {
            Loc::Slot(ss) => self.b.ins().stack_addr(PTR, ss, 0),
            // (codegen.rs はスタックの一時値のアドレスを作り直していた: 支配しない枝で作られた値のため。
            //  minclif の値はフレーム上に置かれるので、そのまま使える)
            Loc::Ptr(a) => a,
            Loc::Var(v) => {
                let x = self.b.use_var(v);
                let ss = self.slot(o.ty);
                self.b.ins().stack_store(x, ss, 0);
                self.b.ins().stack_addr(PTR, ss, 0)
            }
            Loc::Zst => self.b.ins().iconst(PTR, 0),
        };
        self.drop_in_place(addr, o.ty);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
    }

    /// addr にある t の値を drop する (drop glue の呼び出し)
    pub(crate) fn drop_in_place(&mut self, addr: Value, t: Ty) {
        if !self.needs_drop(t) {
            return;
        }
        let Some(&g) = self.prog.index.get(crate::prelude::DROP_GLUE) else {
            return;
        };
        let iid = {
            let mut ins = self.insts.lock().unwrap();
            match ins.request(self.prog, g, vec![t]) {
                Ok(i) => i,
                Err(_) => return,
            }
        };
        self.requested.push(iid);
        let sig = self.insts.lock().unwrap().sig(self.prog, iid).clone();
        let f = self.fref(iid, &sig);
        self.b.ins().call(f, &[addr]);
    }

    /// スコープを閉じる: そのスコープのローカル変数を逆順に drop する
    pub(crate) fn pop_scope(&mut self) {
        let depth = self.scopes.len() - 1;
        while self.owned.last().is_some_and(|o| o.depth >= depth) {
            let o = self.owned.pop().unwrap();
            self.emit_owned_drop(o);
        }
        self.scopes.pop();
    }

    /// 早期脱出 (return / break / continue): 深さ depth 以上のローカル変数と temps[t..] を drop する
    pub(crate) fn drop_for_exit(&mut self, depth: usize, t: usize) {
        let temps: Vec<Owned> = self.temps[t.min(self.temps.len())..].to_vec();
        for o in temps.into_iter().rev() {
            self.emit_owned_drop(o);
        }
        let locals: Vec<Owned> = self.owned.iter().filter(|o| o.depth >= depth).copied().collect();
        for o in locals.into_iter().rev() {
            self.emit_owned_drop(o);
        }
    }

    /// 文の終わり: temps[t..] を drop して捨てる
    pub(crate) fn drop_temps(&mut self, t: usize) {
        while self.temps.len() > t {
            let o = self.temps.pop().unwrap();
            self.emit_owned_drop(o);
        }
    }

    /// ブロックの末尾の式・match の arm の一時値は、その場で drop する (Rust 2024 と同じ)。
    /// 分岐の中で作った値を文の終わりで触ると支配関係が崩れるため。結果そのものの一時値は残す
    pub(crate) fn drop_tail_temps(&mut self, t: usize, result: V) {
        if result.1 == Ty::Never {
            self.temps.truncate(t.min(self.temps.len()));
            return;
        }
        let keep: Vec<Owned> =
            self.temps[t.min(self.temps.len())..].iter().filter(|o| result.0.is_some_and(|v| o.loc == Loc::Ptr(v))).copied().collect();
        let rest: Vec<Owned> = self.temps.drain(t.min(self.temps.len())..).collect();
        for o in rest.into_iter().rev() {
            if !keep.iter().any(|k| k.loc == o.loc) {
                self.emit_owned_drop(o);
            }
        }
        self.temps.extend(keep);
    }

    /// let の初期化式の一時値は、ブロックの終わりまで延ばす (rustc の一時値の寿命延長の近似)
    pub(crate) fn extend_temps(&mut self, t: usize) {
        let depth = self.scopes.len() - 1;
        let ts: Vec<Owned> = self.temps.drain(t.min(self.temps.len())..).collect();
        for mut o in ts {
            o.depth = depth;
            self.owned.push(o);
        }
    }

    pub(crate) fn free(&mut self, p: Value) {
        if let Some(&fr) = self.prog.index.get(crate::prelude::BOX_FREE) {
            let sig = self.prog.sigs[fr as usize].clone();
            let f = self.fref(fr, &sig);
            self.b.ins().call(f, &[p]);
        }
    }

    /// vtable の中で drop glue が入っている位置 (メソッドの後ろ)
    pub(crate) fn vtable_drop_index(&self, tr: &str) -> usize {
        if crate::ast_summary::parse_fn_bound(tr).is_some() { 1 } else { self.prog.traits.get(tr).map(|v| v.len()).unwrap_or(0) }
    }

    /// vtable の drop の欄: t の drop glue (要らなければ u32::MAX = null)
    pub(crate) fn vtable_drop_entry(&mut self, t: Ty) -> u32 {
        if !self.needs_drop(t) {
            return u32::MAX;
        }
        let Some(&g) = self.prog.index.get(crate::prelude::DROP_GLUE) else {
            return u32::MAX;
        };
        let mut ins = self.insts.lock().unwrap();
        match ins.request(self.prog, g, vec![t]) {
            Ok(i) => {
                drop(ins);
                self.requested.push(i);
                i
            }
            Err(_) => u32::MAX,
        }
    }

    /// `__exr_drop_in_place::<T>(p: *mut T)` の本体
    pub(crate) fn drop_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let p = self.b.func_params()[0];
        let Some(Ty::Ptr(_, t)) = sig.params.first() else {
            return Err("internal: bad drop glue signature".into());
        };
        let t = **t;
        match t {
            Ty::Box(Ty::Dyn(tr)) => {
                // 中身の型は vtable の drop 欄が知っている。data が 0 ならムーブ済み
                let data = self.b.ins().load(PTR, mf(), p, 0);
                let live = self.b.create_block();
                let out = self.b.create_block();
                self.b.ins().brif(data, live, &[], out, &[]);
                self.b.switch_to_block(live);
                let vt = self.b.ins().load(PTR, mf(), p, 8);
                let idx = self.vtable_drop_index(tr);
                let dfn = self.b.ins().load(PTR, mf(), vt, 8 * idx as i32);
                let call = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().brif(dfn, call, &[], done, &[]);
                self.b.switch_to_block(call);
                let mut s = Signature::default();
                s.params.push(AbiParam::new(PTR));
                let sr = self.b.import_signature(s);
                self.b.ins().call_indirect(sr, dfn, &[data]);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                self.free(data);
                self.b.ins().jump(out, &[]);
                self.b.switch_to_block(out);
            }
            Ty::Box(inner) => {
                let ptr = self.b.ins().load(PTR, mf(), p, 0);
                let live = self.b.create_block();
                let out = self.b.create_block();
                self.b.ins().brif(ptr, live, &[], out, &[]);
                self.b.switch_to_block(live);
                self.drop_in_place(ptr, *inner);
                self.free(ptr);
                self.b.ins().jump(out, &[]);
                self.b.switch_to_block(out);
            }
            Ty::Array(e, n) => {
                let (size, _) = self.prog.size_align(*e);
                // 要素ごとに drop (長さは静的に分かるので展開しないループ)
                let i = self.b.declare_var(types::I64);
                let z = self.b.ins().iconst(types::I64, 0);
                self.b.def_var(i, z);
                let head = self.b.create_block();
                let body = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(head);
                let iv = self.b.use_var(i);
                let more = self.b.ins().icmp_imm_u(IntCC::UnsignedLessThan, iv, n as i64);
                self.b.ins().brif(more, body, &[], done, &[]);
                self.b.switch_to_block(body);
                let off = self.b.ins().imul_imm_s(iv, size as i64);
                let a = self.b.ins().iadd(p, off);
                self.drop_in_place(a, *e);
                let next = self.b.ins().iadd_imm_s(iv, 1);
                self.b.def_var(i, next);
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(done);
            }
            Ty::Struct(sid) => {
                // 先に `impl Drop` の drop(&mut self)、それからフィールド
                if self.prog.drop_impls.contains(&self.prog.base_of(sid)) {
                    if let Some(m) = self.prog.method(sid, "drop") {
                        let recv = [Ty::Ref(true, intern(t))];
                        let mut fid = m;
                        let ms = &self.prog.sigs[m as usize];
                        if ms.is_generic() {
                            let mut sol = vec![None; ms.generics.len()];
                            if let Some(st) = ms.self_ty {
                                self.prog.match_ty(st, t, &mut sol);
                            }
                            fid = self.resolve_call(m, &recv, sol, None, SP)?.0;
                        }
                        let fs = self.insts.lock().unwrap().sig(self.prog, fid).clone();
                        let f = self.fref(fid, &fs);
                        self.b.ins().call(f, &[p]);
                    }
                }
                let sd = &self.prog.structs[sid as usize];
                if sd.is_enum {
                    let tag_ty = sd.tag;
                    let vs: Vec<(i64, Vec<(Ty, u32)>)> = sd
                        .variants
                        .iter()
                        .map(|v| (v.discr, v.fields.iter().filter(|f| self.needs_drop(f.1)).map(|f| (f.1, f.2)).collect()))
                        .collect();
                    let tag = self.b.ins().load(clif_ty(tag_ty).unwrap(), mf(), p, 0);
                    for (d, fs) in vs {
                        if fs.is_empty() {
                            continue;
                        }
                        let yes = self.b.create_block();
                        let next = self.b.create_block();
                        let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, d);
                        self.b.ins().brif(c, yes, &[], next, &[]);
                        self.b.switch_to_block(yes);
                        for (ft, off) in fs {
                            let a = self.addr_plus(p, off as i32);
                            self.drop_in_place(a, ft);
                        }
                        self.b.ins().jump(next, &[]);
                        self.b.switch_to_block(next);
                    }
                } else {
                    let fs: Vec<(Ty, u32)> = sd.fields.iter().filter(|f| self.needs_drop(f.1)).map(|f| (f.1, f.2)).collect();
                    for (ft, off) in fs {
                        let a = self.addr_plus(p, off as i32);
                        self.drop_in_place(a, ft);
                    }
                }
            }
            _ => {}
        }
        self.b.ins().return_(&[]);
        Ok(())
    }
}
