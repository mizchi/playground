use super::*;
use crate::parse::{Expr, GArg};
use crate::types::{Ty, intern};
use crate::acg::clif::MemFlagsData;

/// `(x)` (syn の parse_quote! の括弧と同じ形)
fn paren(e: &Expr) -> Expr {
    Expr::Paren(Box::new(e.clone()))
}

impl Cx<'_> {
    pub(crate) fn unary(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Unary(op, x, _) = e else { unreachable!() };
        let op = op.as_str();
        // `-v` / `!v` (impl Neg / Not): メソッド呼び出し
        if let ("-" | "!", Some(Ty::Struct(sid))) = (op, self.peek(x)) {
            let m = if op == "-" { "neg" } else { "not" };
            if self.prog.method(sid, m).is_some() {
                let ne = mcall(paren(x), m, vec![]);
                let ne = self.keep_expr(ne);
                return self.expr(ne, expected);
            }
        }
        // 負のリテラル (`-128i8` / `-2147483648`) はその場で値にする (検査で MIN の符号反転にしない)
        if op == "-" {
            if let Expr::Lit(t, _) = strip_paren(x) {
                if let Some(Lit::Int(i)) = lit::lit(t) {
                    let lit_e: &Expr = strip_paren(x);
                    let ty = Ty::from_suffix(i.suffix())
                        .or(expected.filter(|t| t.is_int()))
                        .or_else(|| self.lets.get(&(lit_e as *const Expr as usize)).copied().filter(|t| t.is_int()))
                        .or_else(|| self.lets.get(&(e as *const Expr as usize)).copied().filter(|t| t.is_int()))
                        .unwrap_or(Ty::I32);
                    if ty.signed() {
                        if let Ok(n) = i.base10_digits().parse::<u128>() {
                            let v = self.int_const(ty, (n as i128).wrapping_neg());
                            return Ok((Some(v), ty));
                        }
                    }
                }
            }
        }
        let (v, t) = self.expr(x, expected)?;
        let Some(v) = v else { return Ok((None, t)) };
        match op {
            "-" if t.is_int() && t.signed() => {
                if self.overflow_checks {
                    let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                    let bad = self.b.ins().icmp(IntCC::Equal, v, min);
                    self.ovf_panic(bad, 6, expr_start(e));
                }
                Ok((Some(self.b.ins().ineg(v)), t))
            }
            "-" if t.is_float() => Ok((Some(self.b.ins().fneg(v)), t)),
            "!" if t == Ty::Bool => Ok((Some(self.b.ins().bxor_imm_u(v, 1)), t)),
            "!" if t.is_int() => Ok((Some(self.b.ins().bnot(v)), t)),
            _ => err(SP, format!("unsupported unary operator for `{}`", self.prog.tn(t))),
        }
    }

    /// `a op b` (Expr::Binary) と複合代入 `a op= b` (Expr::AssignOp)
    pub(crate) fn binary(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let (op, left, right, assign) = match e {
            Expr::Binary(op, l, r, _) => (op.as_str(), &**l, &**r, false),
            Expr::AssignOp(op, l, r, _) => (op.as_str(), &**l, &**r, true),
            _ => unreachable!(),
        };
        // 短絡評価
        if !assign && (op == "&&" || op == "||") {
            let (l, lt) = self.expr(left, Some(Ty::Bool))?;
            self.same(SP, lt, Ty::Bool)?;
            let Some(l) = l else {
                return Ok((None, Ty::Never));
            };
            let rhs_blk = self.b.create_block();
            let merge = self.b.create_block();
            let res = self.b.append_block_param(merge, types::I8);
            if op == "&&" {
                self.b.ins().brif(l, rhs_blk, &[], merge, &[l]);
            } else {
                self.b.ins().brif(l, merge, &[l], rhs_blk, &[]);
            }
            self.b.switch_to_block(rhs_blk);
            let (r, rt) = self.expr(right, Some(Ty::Bool))?;
            self.same(SP, rt, Ty::Bool)?;
            match r {
                Some(r) => self.b.ins().jump(merge, &[r]),
                None => self.b.ins().trap(TrapCode::unwrap_user(1)),
            };
            self.b.switch_to_block(merge);
            return Ok((Some(res), Ty::Bool));
        }

        // 演算子のオーバーロード (`impl Add for V` / String + &str): `a.add(b)` / `a.add_assign(b)`
        if let Some(Ty::Struct(sid)) = self.peek(left) {
            let m = match (op, assign) {
                ("+", false) => "add",
                ("-", false) => "sub",
                ("*", false) => "mul",
                ("/", false) => "div",
                ("%", false) => "rem",
                ("&", false) => "bitand",
                ("|", false) => "bitor",
                ("^", false) => "bitxor",
                ("<<", false) => "shl",
                (">>", false) => "shr",
                ("+", true) => "add_assign",
                ("-", true) => "sub_assign",
                ("*", true) => "mul_assign",
                ("/", true) => "div_assign",
                ("%", true) => "rem_assign",
                ("&", true) => "bitand_assign",
                ("|", true) => "bitor_assign",
                ("^", true) => "bitxor_assign",
                ("<<", true) => "shl_assign",
                (">>", true) => "shr_assign",
                _ => "",
            };
            if !m.is_empty() && self.prog.method(sid, m).is_some() {
                let ne = mcall(paren(left), m, vec![right.clone()]);
                let ne = self.keep_expr(ne);
                return self.expr(ne, expected);
            }
        }
        // `&a + &b` (`impl Add for &M`): `(&a).__ref_add(&b)`
        if let Some(Ty::Ref(_, Ty::Struct(sid))) = self.peek(left) {
            let m = match (op, assign) {
                ("+", false) => "add",
                ("-", false) => "sub",
                ("*", false) => "mul",
                ("/", false) => "div",
                ("%", false) => "rem",
                ("&", false) => "bitand",
                ("|", false) => "bitor",
                ("^", false) => "bitxor",
                _ => "",
            };
            let rm = format!("__ref_{m}");
            if !m.is_empty() && self.prog.method(*sid, &rm).is_some() {
                let ne = mcall(paren(left), &rm, vec![right.clone()]);
                let ne = self.keep_expr(ne);
                return self.expr(ne, expected);
            }
        }
        // 複合代入 a op= b
        let compound = match op {
            "+" | "-" | "*" | "/" | "%" | "^" | "&" | "|" | "<<" | ">>" if assign => Some(op),
            _ => None,
        };
        if let Some(op) = compound {
            let p = self.place(left)?;
            let t = p.ty();
            let (l, _) = self.read(p);
            let rhint = if matches!(op, "<<" | ">>") { self.peek(right) } else { Some(t) };
            let (r, rt) = self.expr(right, rhint.or(Some(t)))?;
            let Some(mut r) = r else {
                return Ok((None, Ty::Never));
            };
            // `acc += x` (x: &i32): 整数への参照は値にする (core の `AddAssign<&i32>` 相当)
            let mut rt = rt;
            while let Ty::Ref(_, i) = rt {
                if !(i.is_int() || matches!(i, Ty::Ref(..))) || rt.is_fat() {
                    break;
                }
                r = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), r, 0);
                rt = *i;
            }
            let Some(l) = l else {
                return err(SP, "unsupported operand");
            };
            let v = self.arith(e, op, l, t, r, rt)?;
            self.write(p, Some(v));
            return Ok((None, Ty::Unit));
        }

        let is_shift = matches!(op, "<<" | ">>");
        let hint = if is_cmp(op) {
            self.peek(left).or_else(|| self.peek(right))
        } else if is_shift {
            self.peek(left).or(expected)
        } else {
            self.peek(left).or_else(|| self.peek(right)).or(expected)
        };
        // 比較は参照で比べるのでムーブしない (一時値は文の終わりまで所有する)
        let cmp = is_cmp(op);
        // `x * 10` (x: &i64): リテラルの型は参照の中身
        let deref_num = |t: Option<Ty>| match t {
            Some(Ty::Ref(_, i)) if i.is_int() || i.is_float() => Some(*i),
            t => t,
        };
        let hint = deref_num(hint);
        let (l, lt) = self.operand(left, hint, cmp)?;
        let Some(l) = l else {
            return Ok((None, Ty::Never));
        };
        let rhint = if is_shift { self.peek(right) } else { deref_num(Some(lt)) };
        // (`v < (1, 5)` / `v == 2`: 右辺がリテラルなら、左の型の `PartialOrd<R>` / `PartialEq<R>` の R を期待型にする)
        let rhint = match op {
            "<" | "<=" | ">" | ">=" => self.rhs_hint_from_impls(&self.prog.pord.clone(), lt, right).or(rhint),
            "==" | "!=" => self.rhs_hint_from_impls(&self.prog.peq.clone(), lt, right).or(rhint),
            _ => rhint,
        };
        // (文字列・バイト列のリテラルは自分の型 (`v == b"crate"` の v: Vec<u8> を期待型にしない))
        let lit_str = matches!(strip_paren(right), Expr::Lit(t, _) if matches!(lit::lit(t), Some(Lit::ByteStr(_) | Lit::Str(_))));
        let rhint = if lit_str && matches!(self.strip_refs(lt), Ty::Struct(_)) { None } else { rhint.or(Some(lt)) };
        let (r, rt) = self.operand(right, rhint, cmp)?;
        let Some(r) = r else {
            return Ok((None, Ty::Never));
        };

        // `String == &str` / `&str == String`: String を &str にして比べる
        let (mut l, mut lt, mut r, mut rt) = (l, lt, r, rt);
        // `i32 + &i32` など: 整数への参照は値にしてから計算する (core の `impl Add<&i32> for i32` 相当)
        if let (Ty::Ref(_, a), Ty::Ref(_, c)) = (lt, rt) {
            if a == c && (a.is_int() || a.is_float() || matches!(a, Ty::Char | Ty::Bool)) && !is_shift {
                let ct = clif_ty(*a).unwrap();
                l = self.b.ins().load(ct, mf(), l, 0);
                r = self.b.ins().load(ct, mf(), r, 0);
                lt = *a;
                rt = *a;
            }
        }
        if !matches!(op, "==" | "!=") || !lt.is_agg() {
            if let (true, Ty::Ref(_, i)) = (lt.is_int() || lt.is_float(), rt) {
                if *i == lt {
                    r = self.b.ins().load(clif_ty(lt).unwrap(), mf(), r, 0);
                    rt = lt;
                }
            } else if let (Ty::Ref(_, i), true) = (lt, rt.is_int() || rt.is_float()) {
                if *i == rt && !is_shift {
                    l = self.b.ins().load(clif_ty(rt).unwrap(), mf(), l, 0);
                    lt = rt;
                }
            }
        }
        // (シフトは左右の型が違ってよい: `&u8 >> 4` / `x << &n`)
        if is_shift {
            if let Ty::Ref(_, i) = lt {
                if i.is_int() {
                    l = self.b.ins().load(clif_ty(*i).unwrap(), mf(), l, 0);
                    lt = *i;
                }
            }
            if let Ty::Ref(_, i) = rt {
                if i.is_int() {
                    r = self.b.ins().load(clif_ty(*i).unwrap(), mf(), r, 0);
                    rt = *i;
                }
            }
        }
        let str_ref = Ty::Ref(false, intern(Ty::Str));
        if matches!(op, "==" | "!=") && lt != rt {
            // (`&String == &str` / `&&str == &str` も: 参照を外してから)
            if let (Ty::Ref(_, Ty::Struct(_) | Ty::Ref(..)), true) = (lt, rt == str_ref) {
                if let Ty::Ref(_, i) = lt {
                    // (`&String` の値は String の場所そのもの。`&&T` は 1 段読む。`&&str` は `&str` の表現と同じ)
                    if matches!(i, Ty::Ref(..)) && !i.is_fat() {
                        l = self.b.ins().load(PTR, mf(), l, 0);
                    }
                    lt = *i;
                }
            } else if let (true, Ty::Ref(_, Ty::Struct(_) | Ty::Ref(..))) = (lt == str_ref, rt) {
                if let Ty::Ref(_, i) = rt {
                    // (`&String` の値は String の場所そのもの。`&&T` は 1 段読む。`&&str` は `&str` の表現と同じ)
                    if matches!(i, Ty::Ref(..)) && !i.is_fat() {
                        r = self.b.ins().load(PTR, mf(), r, 0);
                    }
                    rt = *i;
                }
            }
            if let (Ty::Struct(sid), true) = (lt, rt == str_ref) {
                if let Some((f, ft)) = self.deref_to_slice(l, sid, false) {
                    l = f;
                    lt = ft;
                }
            } else if let (true, Ty::Struct(sid)) = (lt == str_ref, rt) {
                if let Some((f, ft)) = self.deref_to_slice(r, sid, false) {
                    r = f;
                    rt = ft;
                }
            }
        }
        // 型の違う `<` / `<=` / `>` / `>=`: `impl PartialOrd<R> for S` の partial_cmp (None ならどれも false)
        if matches!(op, "<" | "<=" | ">" | ">=") && lt != rt {
            let cands = self.prog.pord.clone();
            if let Some((v, t)) = self.trait_bin(&cands, l, lt, r, rt, SP)? {
                let (c, _) = self.call_named("__exr_pord_code", vec![Ty::Unit], &[(v.unwrap(), Ty::Ref(false, intern(t)))], SP)?;
                let c = c.unwrap();
                let e = self.ord_code_test(c, op);
                return Ok((Some(e), Ty::Bool));
            }
        }
        // `&str` / `String` の大小比較: prelude の __exr_str_cmp
        if matches!(op, "<" | "<=" | ">" | ">=") && self.prog.index.contains_key("__exr_str_cmp") {
            let str_ref = Ty::Ref(false, intern(Ty::Str));
            let as_str = |this: &mut Self, v: Value, t: Ty| -> Option<Value> {
                match t {
                    Ty::Ref(_, Ty::Str) => Some(v),
                    Ty::Struct(sid) => this.deref_to_slice(v, sid, false).filter(|(_, ft)| *ft == str_ref).map(|f| f.0),
                    Ty::Ref(_, Ty::Struct(sid)) => this.deref_to_slice(v, *sid, false).filter(|(_, ft)| *ft == str_ref).map(|f| f.0),
                    _ => None,
                }
            };
            if let (Some(x), Some(y)) = (as_str(self, l, lt), as_str(self, r, rt)) {
                let (c, _) = self.call_named("__exr_str_cmp", vec![Ty::Unit], &[(x, str_ref), (y, str_ref)], SP)?;
                let cc = match op {
                    "<" => IntCC::SignedLessThan,
                    "<=" => IntCC::SignedLessThanOrEqual,
                    ">" => IntCC::SignedGreaterThan,
                    _ => IntCC::SignedGreaterThanOrEqual,
                };
                return Ok((Some(self.b.ins().icmp_imm_s(cc, c.unwrap(), 0)), Ty::Bool));
            }
        }
        // タプル・struct・enum・配列・スライス・Vec などの大小比較: 型ごとの glue (__exr_cmp)
        if matches!(op, "<" | "<=" | ">" | ">=") && !(lt.is_int() || lt.is_float() || matches!(lt, Ty::Bool | Ty::Char)) {
            self.same(SP, rt, lt)?;
            let (la, t) = self.ord_addr(l, lt);
            let (ra, _) = self.ord_addr(r, rt);
            // 手書きの partial_cmp (cmp がない): None ならどれも false
            if let Ty::Struct(sid) = t {
                if !self.prog.is_tuple(self.prog.base_of(sid))
                    && self.prog.method(sid, "partial_cmp").is_some()
                    && self.prog.method(sid, "cmp").is_none()
                    && self.prog.index.contains_key("__exr_pord_code")
                {
                    let rr = Ty::Ref(false, intern(t));
                    self.scopes.push(vec![]);
                    self.bind("__exr_ca".into(), rr, Some(la));
                    self.bind("__exr_cb".into(), rr, Some(ra));
                    let ne = crate::parse::parse_expr("__exr_pord_code::<()>(&__exr_ca.partial_cmp(__exr_cb))")?;
                    let ne = self.keep_expr(ne);
                    let (c, _) = self.expr(ne, Some(Ty::I32))?;
                    self.scopes.pop();
                    let e = self.ord_code_test(c.unwrap(), op);
                    return Ok((Some(e), Ty::Bool));
                }
            }
            let c = self.cmp_call(la, ra, t)?;
            let cc = match op {
                "<" => IntCC::SignedLessThan,
                "<=" => IntCC::SignedLessThanOrEqual,
                ">" => IntCC::SignedGreaterThan,
                _ => IntCC::SignedGreaterThanOrEqual,
            };
            return Ok((Some(self.b.ins().icmp_imm_s(cc, c, 0)), Ty::Bool));
        }
        // (`v == b"crate"` / `&v == b"x"` (v: Vec<u8>): 参照を外して `Vec<T> == [T; N]` に。値の表現は同じ (場所のアドレス))
        if matches!(op, "==" | "!=") && lt != rt {
            let is_vec = |this: &Self, t: Ty| matches!(t, Ty::Struct(sid) if this.vec_fields(sid).is_some());
            let peel = |this: &Self, t: Ty| match t {
                Ty::Ref(_, i @ (Ty::Array(..) | Ty::Struct(_))) if i.is_agg() && (matches!(i, Ty::Array(..)) || is_vec(this, *i)) => *i,
                t => t,
            };
            let (pl, pr) = (peel(self, lt), peel(self, rt));
            if (is_vec(self, pl) && matches!(pr, Ty::Array(..))) || (is_vec(self, pr) && matches!(pl, Ty::Array(..))) {
                lt = pl;
                rt = pr;
            }
        }
        // 型の違う `==` / `!=`: 右辺の型を持つ PartialEq の impl (`impl PartialEq<[T; N]> for Vec<T>` など) を探して eq を呼ぶ
        if matches!(op, "==" | "!=") && lt != rt {
            if let Some(e) = self.trait_eq(l, lt, r, rt, SP)? {
                let e = if op == "!=" { self.b.ins().bxor_imm_u(e, 1) } else { e };
                return Ok((Some(e), Ty::Bool));
            }
        }
        // `&a == &b` (a, b は struct など): 参照を外して中身を比べる
        // `&&str == &&str` / `&&[T] == &&[T]`: `&&str` の値は `&str` の置き場所 (= `&str` の表現) なので型だけ外す
        while let (true, Ty::Ref(_, i)) = (lt == rt, lt) {
            if !i.is_fat() {
                break;
            }
            lt = *i;
            rt = *i;
        }
        if matches!(op, "==" | "!=") && lt == rt && matches!(lt, Ty::Ref(..)) {
            let (mut l, mut r, mut t) = (l, r, lt);
            while let Ty::Ref(_, i) = t {
                if i.is_fat() || !matches!(i, Ty::Ref(..)) && !i.is_agg() {
                    break;
                }
                if !i.is_agg() {
                    l = self.b.ins().load(PTR, mf(), l, 0);
                    r = self.b.ins().load(PTR, mf(), r, 0);
                }
                t = *i;
            }
            if t.is_agg() {
                let e = self.agg_eq(l, r, t, SP)?;
                let e = if op == "!=" { self.b.ins().bxor_imm_u(e, 1) } else { e };
                return Ok((Some(e), Ty::Bool));
            }
        }
        // `&&u32 == &&u32` / `&x < &y`: 参照を辿ってスカラーを比べる
        let (mut l, mut r) = (l, r);
        if is_cmp(op) && lt == rt {
            let mut base = lt;
            while let Ty::Ref(_, i) = base {
                base = *i;
            }
            if base != lt && !base.is_agg() && !base.is_fat() && (base.is_int() || base.is_float() || matches!(base, Ty::Bool | Ty::Char)) {
                while let Ty::Ref(_, i) = lt {
                    let ct = if matches!(i, Ty::Ref(..)) { PTR } else { clif_ty(*i).unwrap() };
                    l = self.b.ins().load(ct, mf(), l, 0);
                    r = self.b.ins().load(ct, mf(), r, 0);
                    lt = *i;
                }
                rt = lt;
            }
        }
        if is_cmp(op) {
            self.same(SP, rt, lt)?;
            // 集約型の == / != : 手書きの eq があればそれ、なければ構造的に比べる (derive(PartialEq) 相当)
            if matches!(op, "==" | "!=") && lt.is_agg() {
                let e = self.agg_eq(l, r, lt, SP)?;
                let e = if op == "!=" { self.b.ins().bxor_imm_u(e, 1) } else { e };
                return Ok((Some(e), Ty::Bool));
            }
            // 浮動小数点の比較 (NaN は == / < などで false、!= で true)
            if lt.is_float() {
                let cc = match op {
                    "==" => FloatCC::Equal,
                    "!=" => FloatCC::NotEqual,
                    "<" => FloatCC::LessThan,
                    "<=" => FloatCC::LessThanOrEqual,
                    ">" => FloatCC::GreaterThan,
                    _ => FloatCC::GreaterThanOrEqual,
                };
                return Ok((Some(self.b.ins().fcmp(cc, l, r)), Ty::Bool));
            }
            if !(lt.is_int() || matches!(lt, Ty::Bool | Ty::Char | Ty::Ptr(..))) {
                return err(SP, format!("comparison of `{}` is not supported", self.prog.tn(lt)));
            }
            let s = lt.signed();
            let cc = match op {
                "==" => IntCC::Equal,
                "!=" => IntCC::NotEqual,
                "<" if s => IntCC::SignedLessThan,
                "<" => IntCC::UnsignedLessThan,
                "<=" if s => IntCC::SignedLessThanOrEqual,
                "<=" => IntCC::UnsignedLessThanOrEqual,
                ">" if s => IntCC::SignedGreaterThan,
                ">" => IntCC::UnsignedGreaterThan,
                ">=" if s => IntCC::SignedGreaterThanOrEqual,
                ">=" => IntCC::UnsignedGreaterThanOrEqual,
                _ => unreachable!(),
            };
            return Ok((Some(self.b.ins().icmp(cc, l, r)), Ty::Bool));
        }
        let v = self.arith(e, op, l, lt, r, rt)?;
        Ok((Some(v), lt))
    }

    /// `l == r` (型が違う) を PartialEq<R> の impl で。self 型 S と右辺 R は、値の型そのものか、
    /// 参照の先 (`&[T]` と `impl .. for [T]`、`&[T; N]` と `PartialEq<[T; N]>`) で合わせる
    pub(crate) fn trait_eq(&mut self, l: Value, lt: Ty, r: Value, rt: Ty, span: Sp) -> R<Option<Value>> {
        let cands = self.prog.peq.clone();
        Ok(self.trait_bin(&cands, l, lt, r, rt, span)?.and_then(|v| v.0))
    }

    /// cands (`fn m(&self, other: &R)` の候補) から l と r の型に合うものを選んで呼ぶ
    pub(crate) fn trait_bin(&mut self, cands: &[u32], l: Value, lt: Ty, r: Value, rt: Ty, span: Sp) -> R<Option<V>> {
        // (値の型 t, 参照を足すか): 値そのものなら &t を、参照なら中身をその型として渡す
        let views = |t: Ty| -> Vec<(Ty, bool)> {
            match t {
                Ty::Ref(_, i) => vec![(t, true), (*i, false)],
                t => vec![(t, true)],
            }
        };
        for &id in cands {
            let sig = &self.prog.sigs[id as usize];
            let (Some(&Ty::Ref(_, ps)), Some(&Ty::Ref(_, pr))) = (sig.params.first(), sig.params.get(1)) else {
                continue;
            };
            let ng = sig.generics.len();
            for &(xl, addl) in &views(lt) {
                for &(xr, addr) in &views(rt) {
                    let mut sol = vec![None; ng];
                    if !(self.prog.match_ty(*ps, xl, &mut sol) && self.prog.match_ty(*pr, xr, &mut sol)) {
                        continue;
                    }
                    let Some(targs) = sol.into_iter().collect::<Option<Vec<Ty>>>() else { continue };
                    let la = if addl { self.addr_of_value(l, lt) } else { l };
                    let ra = if addr { self.addr_of_value(r, rt) } else { r };
                    let name = self.prog.names[id as usize].clone();
                    let v = self.call_named(&name, targs, &[(la, Ty::Ref(false, intern(xl))), (ra, Ty::Ref(false, intern(xr)))], span)?;
                    return Ok(Some(v));
                }
            }
        }
        Ok(None)
    }

    /// partial_cmp の結果のコード (-1 / 0 / 1、None は 2) で `<` などを決める
    pub(crate) fn ord_code_test(&mut self, c: Value, op: &str) -> Value {
        let is = |this: &mut Self, k: i64| this.b.ins().icmp_imm_s(IntCC::Equal, c, k);
        match op {
            "<" => is(self, -1),
            ">" => is(self, 1),
            // (<= は -1 か 0、>= は 0 か 1。None (2) はどれも false)
            "<=" => {
                let (x, y) = (is(self, -1), is(self, 0));
                self.b.ins().bor(x, y)
            }
            _ => {
                let (x, y) = (is(self, 1), is(self, 0));
                self.b.ins().bor(x, y)
            }
        }
    }

    /// 左の型 lt の比較の impl のうち、右辺の式 (リテラル・タプル・配列) の形に合う R が 1 つだけならそれ
    pub(crate) fn rhs_hint_from_impls(&self, cands: &[u32], lt: Ty, rhs: &Expr) -> Option<Ty> {
        if cands.is_empty() {
            return None;
        }
        let fits = |t: Ty, e: &Expr| -> bool {
            match strip_paren(e) {
                Expr::Lit(l, _) if matches!(lit::lit(l), Some(Lit::Int(_))) => t.is_int(),
                Expr::Lit(l, _) if matches!(lit::lit(l), Some(Lit::Float(_))) => t.is_float(),
                Expr::Unary(op, _, _) if op == "-" => t.is_int() || t.is_float(),
                Expr::Tuple(tp) => matches!(t, Ty::Struct(sid) if self.prog.is_tuple(self.prog.base_of(sid))
                    && self.prog.structs[sid as usize].args.len() == tp.len()),
                Expr::Array(a) => matches!(t, Ty::Array(_, n) if n as usize == a.len()),
                _ => false,
            }
        };
        let lt0 = match lt {
            Ty::Ref(_, i) => *i,
            t => t,
        };
        let mut found = None;
        for &id in cands {
            let sig = &self.prog.sigs[id as usize];
            let (Some(&Ty::Ref(_, ps)), Some(&Ty::Ref(_, pr))) = (sig.params.first(), sig.params.get(1)) else { continue };
            if sig.is_generic() || self.prog.has_param(*pr) {
                continue;
            }
            if (*ps == lt || *ps == lt0) && fits(*pr, rhs) {
                if found.is_some_and(|f| f != *pr) {
                    return None;
                }
                found = Some(*pr);
            }
        }
        found
    }

    /// t 型の値 v (集約型ならアドレス) への参照
    pub(crate) fn addr_of_value(&mut self, v: Value, t: Ty) -> Value {
        if t.is_agg() {
            return v;
        }
        let a = self.temp(t);
        self.b.ins().store(mf(), v, a, 0);
        a
    }

    /// a, b (t 型の値。集約型ならアドレス) が等しいか (i8 の 0/1)
    pub(crate) fn agg_eq(&mut self, a: Value, b: Value, t: Ty, span: Sp) -> R<Value> {
        if let Ty::Struct(sid) = t {
            if let Some(id) = self.prog.method(sid, "eq") {
                let sig0 = self.prog.sigs[id as usize].clone();
                let (fid, sig) = if sig0.is_generic() {
                    // ジェネリックな impl の eq (`impl<T: PartialEq> Vec<T>`): インスタンスにする
                    let mut sol = vec![None; sig0.generics.len()];
                    if let Some(st) = sig0.self_ty {
                        self.prog.match_ty(st, t, &mut sol);
                    }
                    let r = Ty::Ref(false, intern(t));
                    self.resolve_call(id, &[r, r], sol, None, span)?
                } else {
                    (id, sig0)
                };
                let f = self.fref(fid, &sig);
                let call = self.b.ins().call(f, &[a, b]);
                return Ok(self.b.inst_results(call)[0]);
            }
        }
        self.eq_at(a, b, t, span)
    }

    pub(crate) fn eq_at(&mut self, a: Value, b: Value, t: Ty, span: Sp) -> R<Value> {
        let one = |this: &mut Self| this.b.ins().iconst(types::I8, 1);
        match t {
            Ty::Unit => Ok(one(self)),
            t if t.is_int() || matches!(t, Ty::Bool | Ty::Char | Ty::Ptr(..)) => Ok(self.b.ins().icmp(IntCC::Equal, a, b)),
            t if t.is_float() => Ok(self.b.ins().fcmp(FloatCC::Equal, a, b)),
            // `&str` / `&[T]`: 長さと中身で比べる
            Ty::Ref(_, Ty::Str | Ty::Slice(_)) => {
                let e = match t {
                    Ty::Ref(_, Ty::Slice(e)) => **e,
                    _ => Ty::U8,
                };
                let (es, _) = self.prog.size_align(e);
                let pa = self.b.ins().load(PTR, mf(), a, 0);
                let la = self.b.ins().load(types::I64, mf(), a, 8);
                let pb = self.b.ins().load(PTR, mf(), b, 0);
                let lb = self.b.ins().load(types::I64, mf(), b, 8);
                let done = self.b.create_block();
                let res = self.b.append_block_param(done, types::I8);
                let head = self.b.create_block();
                let body = self.b.create_block();
                let i = self.b.declare_var(types::I64);
                let z = self.b.ins().iconst(types::I64, 0);
                self.b.def_var(i, z);
                let zero8 = self.b.ins().iconst(types::I8, 0);
                let same_len = self.b.ins().icmp(IntCC::Equal, la, lb);
                self.b.ins().brif(same_len, head, &[], done, &[zero8]);
                self.b.switch_to_block(head);
                let iv = self.b.use_var(i);
                let more = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, la);
                let one8 = self.b.ins().iconst(types::I8, 1);
                self.b.ins().brif(more, body, &[], done, &[one8]);
                self.b.switch_to_block(body);
                let off = self.b.ins().imul_imm_s(iv, es as i64);
                let xa = self.b.ins().iadd(pa, off);
                let xb = self.b.ins().iadd(pb, off);
                let (x, y) = match clif_ty(e) {
                    Some(ct) if !e.is_agg() => (self.b.ins().load(ct, mf(), xa, 0), self.b.ins().load(ct, mf(), xb, 0)),
                    _ => (xa, xb),
                };
                let eq = self.agg_or_scalar_eq(x, y, e, span)?;
                let next = self.b.ins().iadd_imm_s(iv, 1);
                self.b.def_var(i, next);
                let zero8 = self.b.ins().iconst(types::I8, 0);
                self.b.ins().brif(eq, head, &[], done, &[zero8]);
                self.b.switch_to_block(done);
                Ok(res)
            }
            // (Box・参照の先の struct は glue 関数で: 再帰型 (`enum T { R(&'static T) }`) をその場で展開しない)
            Ty::Box(inner @ Ty::Struct(_)) | Ty::Ref(_, inner @ Ty::Struct(_)) if self.prog.index.contains_key("__exr_eq") => {
                let r = Ty::Ref(false, intern(*inner));
                let (v, _) = self.call_named("__exr_eq", vec![*inner], &[(a, r), (b, r)], span)?;
                Ok(v.unwrap())
            }
            // 参照と Box は指す先の値で比べる
            Ty::Ref(_, inner) | Ty::Box(inner) if !t.is_fat() => {
                let (x, y) = match clif_ty(*inner) {
                    Some(ct) if !inner.is_agg() => (self.b.ins().load(ct, mf(), a, 0), self.b.ins().load(ct, mf(), b, 0)),
                    _ => (a, b),
                };
                self.agg_or_scalar_eq(x, y, *inner, span)
            }
            Ty::Array(e, n) => {
                let (es, _) = self.prog.size_align(*e);
                let mut acc = one(self);
                for i in 0..n {
                    let (x, y) = self.load_field(a, b, *e, i * es);
                    let c = self.agg_or_scalar_eq(x, y, *e, span)?;
                    acc = self.b.ins().band(acc, c);
                }
                Ok(acc)
            }
            Ty::Struct(sid) => {
                let sd = &self.prog.structs[sid as usize];
                if !sd.is_enum {
                    let fields = sd.fields.clone();
                    let mut acc = one(self);
                    for (_, ft, off) in fields {
                        let (x, y) = self.load_field(a, b, ft, off);
                        let c = self.agg_or_scalar_eq(x, y, ft, span)?;
                        acc = self.b.ins().band(acc, c);
                    }
                    return Ok(acc);
                }
                // enum: タグが同じで、そのバリアントのフィールドが等しい
                let (tag, variants) = (sd.tag, sd.variants.iter().map(|v| (v.discr, v.fields.clone())).collect::<Vec<_>>());
                let ct = clif_ty(tag).unwrap();
                let ta = self.b.ins().load(ct, mf(), a, 0);
                let tb = self.b.ins().load(ct, mf(), b, 0);
                let tag_eq = self.b.ins().icmp(IntCC::Equal, ta, tb);
                // フィールドが数値だけなら分岐せずに全バリアントを比べる (読んでも安全)
                let plain = variants
                    .iter()
                    .all(|(_, fs)| fs.iter().all(|f| f.1.is_int() || f.1.is_float() || matches!(f.1, Ty::Bool | Ty::Char | Ty::Unit)));
                if plain {
                    let mut any = self.b.ins().iconst(types::I8, 0);
                    for (discr, fields) in variants {
                        let is_v = self.b.ins().icmp_imm_s(IntCC::Equal, ta, discr);
                        let mut acc = is_v;
                        for (_, ft, off) in fields {
                            let (x, y) = self.load_field(a, b, ft, off);
                            let c = self.agg_or_scalar_eq(x, y, ft, span)?;
                            acc = self.b.ins().band(acc, c);
                        }
                        any = self.b.ins().bor(any, acc);
                    }
                    return Ok(self.b.ins().band(tag_eq, any));
                }
                // タグで分岐してから、そのバリアントのフィールドだけを比べる
                // (ほかのバリアントの場所を読むと、`None` のときの String のポインタなどを辿ってしまう)
                let done = self.b.create_block();
                let res = self.b.append_block_param(done, types::I8);
                let zero = self.b.ins().iconst(types::I8, 0);
                let start = self.b.create_block();
                self.b.ins().brif(tag_eq, start, &[], done, &[zero]);
                self.b.switch_to_block(start);
                for (discr, fields) in variants {
                    if fields.is_empty() {
                        let is_v = self.b.ins().icmp_imm_s(IntCC::Equal, ta, discr);
                        let one_v = self.b.ins().iconst(types::I8, 1);
                        let next = self.b.create_block();
                        self.b.ins().brif(is_v, done, &[one_v], next, &[]);
                        self.b.switch_to_block(next);
                        continue;
                    }
                    let is_v = self.b.ins().icmp_imm_s(IntCC::Equal, ta, discr);
                    let vb = self.b.create_block();
                    let next = self.b.create_block();
                    self.b.ins().brif(is_v, vb, &[], next, &[]);
                    self.b.switch_to_block(vb);
                    let mut acc = self.b.ins().iconst(types::I8, 1);
                    for (_, ft, off) in fields {
                        let (x, y) = self.load_field(a, b, ft, off);
                        let c = self.agg_or_scalar_eq(x, y, ft, span)?;
                        acc = self.b.ins().band(acc, c);
                    }
                    self.b.ins().jump(done, &[acc]);
                    self.b.switch_to_block(next);
                }
                let z2 = self.b.ins().iconst(types::I8, 0);
                self.b.ins().jump(done, &[z2]);
                self.b.switch_to_block(done);
                Ok(res)
            }
            t => err(span, format!("`==` is not supported for `{}`", self.prog.tn(t))),
        }
    }

    pub(crate) fn agg_or_scalar_eq(&mut self, x: Value, y: Value, t: Ty, span: Sp) -> R<Value> {
        if t.is_agg() { self.agg_eq(x, y, t, span) } else { self.eq_at(x, y, t, span) }
    }

    /// a + off / b + off にある t 型の値 (集約型ならアドレス)
    pub(crate) fn load_field(&mut self, a: Value, b: Value, t: Ty, off: u32) -> (Value, Value) {
        match clif_ty(t) {
            Some(ct) if !t.is_agg() => (self.b.ins().load(ct, mf(), a, off as i32), self.b.ins().load(ct, mf(), b, off as i32)),
            _ => (self.addr_plus(a, off as i32), self.addr_plus(b, off as i32)),
        }
    }

    /// b は演算の式全体 (Binary / AssignOp。span のためだけに受ける)、op は `=` を除いた演算子
    pub(crate) fn arith(&mut self, b_expr: &Expr, op: &str, l: Value, lt: Ty, r: Value, rt: Ty) -> R<Value> {
        if matches!(op, "<<" | ">>") {
            if !lt.is_int() || !rt.is_int() {
                return err(SP, "shift of non-integer");
            }
            if self.overflow_checks && !self.iconst_of(r).is_some_and(|k| (0..lt.bits() as i64).contains(&k)) {
                // 型の幅以上のシフト (負の量も符号なしで大きい)
                let bad = self.b.ins().icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, r, lt.bits() as i64);
                self.ovf_panic(bad, if op == "<<" { 7 } else { 8 }, expr_start(b_expr));
            }
            let ins = self.b.ins();
            return Ok(match op {
                "<<" => ins.ishl(l, r),
                _ if lt.signed() => ins.sshr(l, r),
                _ => ins.ushr(l, r),
            });
        }
        // char の範囲 (`'a'..='e'`) の prelude が `v + 1` で次の文字にする
        if lt == Ty::Char && rt.is_int() && matches!(op, "+" | "-") {
            let r32 = match rt.bits() {
                32 => r,
                b if b > 32 => self.b.ins().ireduce(types::I32, r),
                _ => self.b.ins().uextend(types::I32, r),
            };
            return Ok(if op == "+" { self.b.ins().iadd(l, r32) } else { self.b.ins().isub(l, r32) });
        }
        self.same(SP, rt, lt)?;
        if lt.is_float() {
            return Ok(match op {
                "+" => self.b.ins().fadd(l, r),
                "-" => self.b.ins().fsub(l, r),
                "*" => self.b.ins().fmul(l, r),
                "/" => self.b.ins().fdiv(l, r),
                // (Cranelift に剰余はないので libm の fmod)
                "%" => {
                    let f = if lt == Ty::F32 { "__exr_fmodf" } else { "__exr_fmod" };
                    let (v, _) = self.call_named(f, vec![], &[(l, lt), (r, lt)], SP)?;
                    v.unwrap()
                }
                _ => return err(SP, format!("unsupported operator for `{}`", self.prog.tn(lt))),
            });
        }
        let bool_ok = matches!(op, "&" | "|" | "^");
        if !(lt.is_int() || (lt == Ty::Bool && bool_ok)) {
            return err(SP, format!("unsupported operator for `{}`", self.prog.tn(lt)));
        }
        let s = lt.signed();
        // 128 bit の除算は Cranelift にないので prelude の関数
        if lt.bits() == 128 && matches!(op, "/" | "%") {
            return self.div128(l, r, lt, op == "%", SP);
        }
        // 0 除算と MIN / -1 は (rustc と同じく最適化しても) panic
        if lt.is_int() && matches!(op, "/" | "%") && self.user_code {
            let rem = op == "%";
            let cr = self.iconst_of(r);
            if cr.is_none() || cr == Some(0) {
                let zero = self.b.ins().icmp_imm_u(IntCC::Equal, r, 0);
                self.ovf_panic(zero, if rem { 4 } else { 3 }, expr_start(b_expr));
            }
            if s && (cr.is_none() || cr == Some(-1)) {
                let min = self.int_const(lt, crate::program::prim_assoc_const(lt, "MIN").unwrap());
                let am = self.b.ins().icmp(IntCC::Equal, l, min);
                let bm = self.b.ins().icmp_imm_s(IntCC::Equal, r, -1);
                let both = self.b.ins().band(am, bm);
                self.ovf_panic(both, if rem { 10 } else { 9 }, expr_start(b_expr));
            }
        }
        // 足し算・引き算・掛け算のオーバーフロー
        if lt.is_int() && self.overflow_checks && matches!(op, "+" | "-" | "*") {
            let kind = match op {
                "+" => 0,
                "-" => 1,
                _ => 2,
            };
            if lt.bits() < 128 || kind == 2 {
                let (v, of) = self.arith_ovf(kind as u8, s, l, r, lt);
                self.ovf_panic(of, kind, expr_start(b_expr));
                return Ok(v);
            }
            // (128 bit: 足し算・引き算は結果の符号と大小で見る。掛け算は検査しない)
            if kind < 2 {
                let v = if kind == 0 { self.b.ins().iadd(l, r) } else { self.b.ins().isub(l, r) };
                let of = if !s {
                    if kind == 0 { self.b.ins().icmp(IntCC::UnsignedLessThan, v, l) } else { self.b.ins().icmp(IntCC::UnsignedLessThan, l, r) }
                } else {
                    let x = if kind == 0 { self.b.ins().bxor(l, v) } else { self.b.ins().bxor(l, r) };
                    let y = if kind == 0 { self.b.ins().bxor(r, v) } else { self.b.ins().bxor(l, v) };
                    let m = self.b.ins().band(x, y);
                    let z = self.int_const(lt, 0);
                    self.b.ins().icmp(IntCC::SignedLessThan, m, z)
                };
                self.ovf_panic(of, kind, expr_start(b_expr));
                return Ok(v);
            }
        }
        let ins = self.b.ins();
        Ok(match op {
            "+" => ins.iadd(l, r),
            "-" => ins.isub(l, r),
            "*" => ins.imul(l, r),
            "/" if s => ins.sdiv(l, r),
            "/" => ins.udiv(l, r),
            "%" if s => ins.srem(l, r),
            "%" => ins.urem(l, r),
            "&" => ins.band(l, r),
            "|" => ins.bor(l, r),
            "^" => ins.bxor(l, r),
            _ => return err(SP, "unsupported binary operator"),
        })
    }

    /// 型だけで決まる条件 (`__exr_int_kind::<T>() == 0` / `needs_drop::<T>()` と `!` / `||` / `&&`) の値。
    /// prelude の型ごとの分岐を、使わない側をコンパイルせずに済ませる (インスタンスも作らない)
    pub(crate) fn type_cond(&self, e: &Expr) -> Option<bool> {
        let targ = |c: &Expr| -> Option<(String, Ty)> {
            let Expr::Call(func, _) = c else { return None };
            let Expr::Path(_, p) = &**func else { return None };
            let last = p.segs.last()?;
            let Some(GArg::Ty(t)) = last.args.first() else { return None };
            Some((last.name.clone(), self.parse_ty(t).ok()?))
        };
        let int_of = |e: &Expr| -> Option<i64> {
            match strip_paren(e) {
                Expr::Lit(l, _) => match lit::lit(l) {
                    Some(Lit::Int(i)) => i.base10_digits().parse().ok(),
                    _ => None,
                },
                _ => None,
            }
        };
        match strip_paren(e) {
            Expr::Unary(op, x, _) if op == "!" => self.type_cond(x).map(|b| !b),
            Expr::Binary(op, l, r, _) => match op.as_str() {
                "||" => match self.type_cond(l) {
                    Some(true) => Some(true),
                    Some(false) => self.type_cond(r),
                    None => None,
                },
                "&&" => match self.type_cond(l) {
                    Some(false) => Some(false),
                    Some(true) => self.type_cond(r),
                    None => None,
                },
                "==" | "!=" => {
                    let c @ Expr::Call(..) = strip_paren(l) else { return None };
                    let (name, t) = targ(c)?;
                    if name != "__exr_int_kind" || self.prog.has_param(t) {
                        return None;
                    }
                    let k = if t.is_int() && t.bits() <= 64 { 1 + t.signed() as i64 } else { 0 };
                    let n = int_of(r)?;
                    Some((k == n) == (op == "=="))
                }
                _ => None,
            },
            c @ Expr::Call(..) => {
                let (name, t) = targ(c)?;
                (name == "needs_drop" && !self.prog.has_param(t)).then(|| self.needs_drop(t))
            }
            _ => None,
        }
    }

    pub(crate) fn if_(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::If(cond, then_branch, else_branch) = e else { unreachable!() };
        if let Expr::Let(..) = &**cond {
            return self.if_let(e, cond, expected);
        }
        // (型で決まる条件なら、通る側だけ)
        match self.type_cond(cond) {
            Some(true) => return self.block(then_branch, expected),
            Some(false) => {
                return match else_branch {
                    Some(e) => self.expr(e, expected),
                    None => Ok((None, Ty::Unit)),
                };
            }
            None => {}
        }
        let (c, ct) = self.expr(cond, Some(Ty::Bool))?;
        self.same(SP, ct, Ty::Bool)?;
        let Some(c) = c else {
            return Ok((None, Ty::Never));
        };
        let then_blk = self.b.create_block();
        let else_blk = self.b.create_block();
        let merge = self.b.create_block();
        self.b.ins().brif(c, then_blk, &[], else_blk, &[]);

        let mut result_ty: Option<Ty> = None;
        let mut param: Option<Value> = None;

        self.b.switch_to_block(then_blk);
        let (tv, tt) = self.block(then_branch, expected)?;
        self.finish_arm(tv, tt, merge, &mut result_ty, &mut param, SP)?;

        self.b.switch_to_block(else_blk);
        let (ev, et) = match else_branch {
            Some(e) => self.expr(e, result_ty.or(expected))?,
            None => (None, Ty::Unit),
        };
        self.finish_arm(ev, et, merge, &mut result_ty, &mut param, SP)?;

        self.b.switch_to_block(merge);
        match result_ty {
            None => Ok((None, Ty::Never)),
            Some(t) => Ok((param, t)),
        }
    }

    pub(crate) fn finish_arm(
        &mut self,
        v: Option<Value>,
        t: Ty,
        merge: Block,
        result_ty: &mut Option<Ty>,
        param: &mut Option<Value>,
        span: Sp,
    ) -> R<()> {
        if t == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(());
        }
        match *result_ty {
            Some(rt) => self.same(span, t, rt)?,
            None => {
                *result_ty = Some(t);
                if let Some(ct) = clif_ty(t) {
                    *param = Some(self.b.append_block_param(merge, ct));
                }
            }
        }
        match v {
            Some(v) => self.b.ins().jump(merge, &[v]),
            None => self.b.ins().jump(merge, &[]),
        };
        Ok(())
    }
}

impl Cx<'_> {
    // --- enum の値 ------------------------------------------------------

    /// 一時領域にタグと各フィールドを書いて enum の値を作る。args は (フィールド番号, 式)
    pub(crate) fn variant_value(&mut self, sid: u32, vi: usize, args: &[(usize, &Expr)], expected: Option<Ty>) -> R<V> {
        self.adt_value(sid, Some(vi), args, expected, SP)
    }

    /// struct (vi = None) / enum のバリアント (vi = Some) の値を一時領域に作る。args は (フィールド番号, 式)。
    /// ジェネリックな定義なら、期待型 → 実引数の型 の順にインスタンスを決める
    pub(crate) fn adt_value(&mut self, base: u32, vi: Option<usize>, args: &[(usize, &Expr)], expected: Option<Ty>, span: Sp) -> R<V> {
        let fields_of = |this: &Self, sid: u32| -> Vec<(String, Ty, u32)> {
            let sd = &this.prog.structs[sid as usize];
            match vi {
                Some(vi) => sd.variants[vi].fields.clone(),
                None => sd.fields.clone(),
            }
        };
        let bdef = &self.prog.structs[base as usize];
        let n_gen = bdef.generics.len();
        let mut inst = if n_gen == 0 {
            Some(base)
        } else {
            match expected {
                Some(Ty::Struct(e)) if self.prog.base_of(e) == self.prog.base_of(base) && !self.prog.has_param(Ty::Struct(e)) => Some(e),
                _ => None,
            }
        };
        let gen_fields = fields_of(self, self.prog.base_of(base));
        let mut sol: Vec<Option<Ty>> = vec![None; n_gen];
        let mut vals = vec![];
        for &(fi, e) in args {
            let exp = match inst {
                Some(i) => Some(fields_of(self, i)[fi].1),
                None => Some(self.prog.partial_subst(gen_fields[fi].1, &sol)).filter(|&t| !self.prog.has_param(t)),
            };
            let (v, vt) = self.expr(e, exp)?;
            if vt == Ty::Never {
                return Ok((None, Ty::Never));
            }
            if inst.is_none() {
                self.prog.match_ty(gen_fields[fi].1, vt, &mut sol);
            }
            vals.push((fi, v, vt, SP));
        }
        let inst = match inst.take() {
            Some(i) => i,
            None => match sol.iter().copied().collect::<Option<Vec<Ty>>>() {
                Some(targs) => self.prog.instantiate(self.prog.base_of(base), targs).or_else(|e| err(span, e))?,
                None => {
                    let n = &self.prog.structs[base as usize].name;
                    return err(span, format!("type annotations needed for `{n}<..>`"));
                }
            },
        };
        let t = Ty::Struct(inst);
        let a = self.temp(t);
        let fields = fields_of(self, inst);
        if let Some(vi) = vi {
            let sd = &self.prog.structs[inst as usize];
            let (tag, discr) = (sd.tag, sd.variants[vi].discr);
            let tv = self.b.ins().iconst(clif_ty(tag).unwrap(), discr);
            self.b.ins().store(mf(), tv, a, 0);
        }
        for (fi, v, vt, sp) in vals {
            let (_, fty, off) = fields[fi];
            // (`V(x)` で x: &&T を &T のフィールドに: deref の強制変換)
            let (v, vt) = self.deref_coerce(v, vt, fty);
            self.same(sp, vt, fty)?;
            self.write(Place::Mem(a, off as i32, fty), v);
        }
        Ok((Some(a), t))
    }

    // --- タプル・配列・スライス ------------------------------------------

    pub(crate) fn make_fat(&mut self, ptr: Value, len: i64) -> Value {
        let l = self.b.ins().iconst(types::I64, len);
        self.make_fat_v(ptr, l)
    }

    /// (ポインタ, 長さ) の組を一時領域に作り、そのアドレスを返す
    pub(crate) fn make_fat_v(&mut self, ptr: Value, len: Value) -> Value {
        let ss = self.b.create_stack_slot(16, 1 << 3);
        let a = self.b.ins().stack_addr(PTR, ss, 0);
        self.b.ins().store(mf(), ptr, a, 0);
        self.b.ins().store(mf(), len, a, 8);
        a
    }

    pub(crate) fn elem_addr(&mut self, base: Value, i: Value, elem: Ty) -> Value {
        let (size, _) = self.prog.size_align(elem);
        let off = self.b.ins().imul_imm_s(i, size as i64);
        self.b.ins().iadd(base, off)
    }

    /// 足し算 (kind 0)・引き算 (1)・掛け算 (2) の (結果, あふれたか)。128 bit は Cranelift の *_overflow がないので組み立てる
    pub(crate) fn arith_ovf(&mut self, kind: u8, s: bool, l: Value, r: Value, t: Ty) -> (Value, Value) {
        if t.bits() < 128 {
            let ins = self.b.ins();
            return match (kind, s) {
                (0, true) => ins.sadd_overflow(l, r),
                (0, false) => ins.uadd_overflow(l, r),
                (1, true) => ins.ssub_overflow(l, r),
                (1, false) => ins.usub_overflow(l, r),
                (_, true) => ins.smul_overflow(l, r),
                (_, false) => ins.umul_overflow(l, r),
            };
        }
        if kind < 2 {
            let v = if kind == 0 { self.b.ins().iadd(l, r) } else { self.b.ins().isub(l, r) };
            let of = if !s {
                if kind == 0 { self.b.ins().icmp(IntCC::UnsignedLessThan, v, l) } else { self.b.ins().icmp(IntCC::UnsignedLessThan, l, r) }
            } else {
                let x = if kind == 0 { self.b.ins().bxor(l, v) } else { self.b.ins().bxor(l, r) };
                let y = if kind == 0 { self.b.ins().bxor(r, v) } else { self.b.ins().bxor(l, v) };
                let m = self.b.ins().band(x, y);
                let z = self.int_const(t, 0);
                self.b.ins().icmp(IntCC::SignedLessThan, m, z)
            };
            return (v, of);
        }
        // 掛け算: 絶対値の積を 64 bit の半分ずつで調べる
        let zero = self.int_const(t, 0);
        let (ua, ub, neg) = if s {
            let an = self.b.ins().icmp(IntCC::SignedLessThan, l, zero);
            let bn = self.b.ins().icmp(IntCC::SignedLessThan, r, zero);
            let la = self.b.ins().ineg(l);
            let ra = self.b.ins().ineg(r);
            let ua = self.b.ins().select(an, la, l);
            let ub = self.b.ins().select(bn, ra, r);
            (ua, ub, Some(self.b.ins().bxor(an, bn)))
        } else {
            (l, r, None)
        };
        let (al, ah) = self.b.ins().isplit(ua);
        let (bl, bh) = self.b.ins().isplit(ub);
        let ah_nz = self.b.ins().icmp_imm_u(IntCC::NotEqual, ah, 0);
        let bh_nz = self.b.ins().icmp_imm_u(IntCC::NotEqual, bh, 0);
        let both = self.b.ins().band(ah_nz, bh_nz);
        let (c1, o1) = self.b.ins().umul_overflow(ah, bl);
        let (c2, o2) = self.b.ins().umul_overflow(al, bh);
        let (cross, o3) = self.b.ins().uadd_overflow(c1, c2);
        let hi = self.b.ins().umulhi(al, bl);
        let (_, o4) = self.b.ins().uadd_overflow(cross, hi);
        let mut of = self.b.ins().bor(both, o1);
        of = self.b.ins().bor(of, o2);
        of = self.b.ins().bor(of, o3);
        of = self.b.ins().bor(of, o4);
        let mag = self.b.ins().imul(ua, ub);
        let v = self.b.ins().imul(l, r);
        if let Some(neg) = neg {
            // |積| は負なら 2^127 まで、正なら 2^127 - 1 まで
            let max = self.int_const(t, i128::MAX);
            let over_pos = self.b.ins().icmp(IntCC::UnsignedGreaterThan, mag, max);
            let one = self.int_const(t, 1);
            let max1 = self.b.ins().iadd(max, one);
            let over_neg = self.b.ins().icmp(IntCC::UnsignedGreaterThan, mag, max1);
            let over = self.b.ins().select(neg, over_neg, over_pos);
            of = self.b.ins().bor(of, over);
        }
        (v, of)
    }

    /// 型推論の記録がなければ (codegen が作り直したノード)、変数・フィールド・deref・括弧だけの式の型を構文から求める
    pub(crate) fn peek_or_static(&self, e: &Expr) -> Option<Ty> {
        self.peek(e).or_else(|| self.static_ty(e))
    }

    pub(crate) fn static_ty(&self, e: &Expr) -> Option<Ty> {
        match e {
            Expr::Paren(p) => self.static_ty(p),
            Expr::Path(_, p) => self.lookup(ident_of(p)?).map(|(_, t)| t),
            Expr::Unary(op, x, _) if op == "*" => match self.static_ty(x)? {
                Ty::Ref(_, i) | Ty::Box(i) | Ty::Ptr(_, i) => Some(*i),
                _ => None,
            },
            Expr::Ref(m, x) => self.static_ty(x).map(|t| Ty::Ref(*m, intern(t))),
            Expr::Field(base, name, _) => {
                let Ty::Struct(sid) = self.strip_refs(self.static_ty(base)?) else { return None };
                if let Some((t, _)) = self.prog.field(sid, name) {
                    return Some(t);
                }
                // (Deref の先のフィールド)
                let d = self.prog.method(sid, "deref")?;
                let Ty::Ref(_, Ty::Struct(ts)) = self.prog.sigs[d as usize].ret else { return None };
                self.prog.field(*ts, name).map(|(t, _)| t)
            }
            _ => None,
        }
    }

    /// e が Deref を実装した (Vec・Box・String でない) struct の値なら、`*e.deref_mut()` (なければ `*e.deref()`)
    pub(crate) fn user_deref(&self, e: &Expr) -> Option<Expr> {
        let Ty::Struct(sid) = self.peek(e)? else { return None };
        self.user_deref_sid(e, sid)
    }

    pub(crate) fn user_deref_sid(&self, e: &Expr, sid: u32) -> Option<Expr> {
        if self.vec_fields(sid).is_some() {
            return None;
        }
        let m = if self.prog.method(sid, "deref_mut").is_some() { "deref_mut" } else { "deref" };
        let did = self.prog.method(sid, m)?;
        if !matches!(self.prog.sigs[did as usize].ret, Ty::Ref(_, i) if !i.is_fat() && !matches!(i, Ty::Str | Ty::Slice(_) | Ty::Dyn(_))) {
            return None;
        }
        Some(Expr::Unary("*".into(), Box::new(mcall(paren(e), m, vec![])), crate::parse::Span::default()))
    }

    /// 添字アクセスの対象 (配列 / スライス / それらへの参照) の (先頭アドレス, 長さ, 要素型)
    pub(crate) fn indexable(&mut self, e: &Expr) -> R<(Value, Value, Ty)> {
        // (`g[i]` (g: MutexGuard<Vec<T>> など): `(*g)[i]`)
        if let Some(d) = self.user_deref(e) {
            let d = self.keep_expr(d);
            return self.indexable(d);
        }
        // (-O: ループの前で読んでおいたもの)
        if !self.hoisted.is_empty() {
            // (キーは式の Debug の文字列: syn 版のトークン列の文字列の代わり)
            let key = format!("{e:?}");
            if let Some(&(_, b, l, t, chk)) = self.hoisted.iter().rev().find(|h| h.0 == key) {
                if let Some((ok, len, i)) = chk {
                    let sp = match e {
                        Expr::Index(_, _, isp) => isp.at,
                        _ => expr_start(e),
                    };
                    self.index_check(ok, len, i, sp);
                }
                return Ok((b, l, t));
            }
        }
        let is_const = matches!(e, Expr::Path(..)) && self.const_of(e).is_some();
        let mut p = if is_place_expr(e) && !is_const {
            self.place(e)?
        } else {
            match self.expr(e, None)? {
                (Some(a), t) if t.is_agg() => Place::Mem(a, 0, t),
                (Some(v), Ty::Ref(_, i) | Ty::Box(i)) => Place::Mem(v, 0, *i),
                (_, t) => {
                    return err(SP, format!("cannot index into a value of type `{}`", self.prog.tn(t)));
                }
            }
        };
        loop {
            p = match p {
                Place::Mem(a, o, Ty::Array(t, n)) => {
                    let base = self.addr_plus(a, o);
                    let len = self.b.ins().iconst(types::I64, n as i64);
                    return Ok((base, len, *t));
                }
                Place::Mem(a, o, Ty::Ref(_, Ty::Slice(t))) => {
                    let base = self.b.ins().load(PTR, mf(), a, o);
                    let len = self.b.ins().load(types::I64, mf(), a, o + 8);
                    return Ok((base, len, **t));
                }
                // Vec<T>: (ptr, len) フィールド
                Place::Mem(a, o, Ty::Struct(sid)) if self.vec_fields(sid).is_some() => {
                    let (po, lo, t) = self.vec_fields(sid).unwrap();
                    let base = self.b.ins().load(PTR, mf(), a, o + po as i32);
                    let len = self.b.ins().load(types::I64, mf(), a, o + lo as i32);
                    return Ok((base, len, t));
                }
                Place::Mem(a, o, Ty::Ref(_, i) | Ty::Box(i)) => {
                    let ptr = self.b.ins().load(PTR, mf(), a, o);
                    Place::Mem(ptr, 0, *i)
                }
                Place::Var(v, Ty::Ref(_, i) | Ty::Box(i)) => {
                    let ptr = self.b.use_var(v);
                    Place::Mem(ptr, 0, *i)
                }
                p => {
                    return err(SP, format!("cannot index into a value of type `{}`", self.prog.tn(p.ty())));
                }
            }
        }
    }

    /// `&a[lo..hi]` / `&a[..]` / `&a[lo..=hi]` (rg は Expr::Range)
    pub(crate) fn slice_range(&mut self, base: &Expr, rg: &Expr, m: bool) -> R<V> {
        let Expr::Range(start, end, inclusive) = rg else { unreachable!() };
        // `&s[a..b]` (String / &str): バイト位置で切り、文字の境界でなければ panic
        let bt = self.peek(base).map(|t| self.strip_refs(t));
        let is_string = matches!(bt, Some(Ty::Struct(sid))
            if self.prog.structs[sid as usize].base.is_none() && self.prog.structs[sid as usize].name == "String");
        let str_fat = if is_string {
            let ne = mcall(paren(base), "as_str", vec![]);
            let ne = self.keep_expr(ne);
            Some(self.expr(ne, None)?)
        } else if matches!(self.peek(base), Some(Ty::Ref(_, Ty::Str))) {
            Some(self.expr(base, None)?)
        } else {
            None
        };
        if let Some((fv, _)) = str_fat {
            let Some(fat) = fv else { return Ok((None, Ty::Never)) };
            let ptr = self.b.ins().load(PTR, mf(), fat, 0);
            let len = self.b.ins().load(types::I64, mf(), fat, 8);
            let (lo, hi) = self.range_bounds(rg, len)?;
            let c1 = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, lo, hi);
            self.panic_unless(c1);
            let c2 = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, hi, len);
            self.panic_unless(c2);
            for idx in [lo, hi] {
                let ok = self.char_boundary(ptr, len, idx);
                self.panic_unless(ok);
            }
            let start = self.b.ins().iadd(ptr, lo);
            let n = self.b.ins().isub(hi, lo);
            let fat = self.make_fat_v(start, n);
            return Ok((Some(fat), Ty::Ref(false, intern(Ty::Str))));
        }
        let (ptr, len, elem) = self.indexable(base)?;
        let lo = match start {
            Some(e) => {
                let (v, t) = self.expr(e, Some(Ty::Usize))?;
                self.same(SP, t, Ty::Usize)?;
                v.unwrap()
            }
            None => self.b.ins().iconst(types::I64, 0),
        };
        let hi = match end {
            Some(e) => {
                let (v, t) = self.expr(e, Some(Ty::Usize))?;
                self.same(SP, t, Ty::Usize)?;
                let v = v.unwrap();
                if *inclusive { self.b.ins().iadd_imm_s(v, 1) } else { v }
            }
            None => len,
        };
        let c1 = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, lo, hi);
        self.panic_unless(c1);
        let c2 = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, hi, len);
        self.panic_unless(c2);
        let start = self.elem_addr(ptr, lo, elem);
        let n = self.b.ins().isub(hi, lo);
        let fat = self.make_fat_v(start, n);
        Ok((Some(fat), Ty::Ref(m, intern(Ty::Slice(intern(elem))))))
    }

    pub(crate) fn tuple_lit(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Tuple(elems) = e else { unreachable!() };
        let base = self.prog.tuple_base(elems.len());
        let args: Vec<(usize, &Expr)> = elems.iter().enumerate().collect();
        self.adt_value(base, None, &args, expected, SP)
    }

    pub(crate) fn array_lit(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Array(elems) = e else { unreachable!() };
        let mut elem = match expected {
            Some(Ty::Array(t, _)) | Some(Ty::Slice(t)) => Some(*t),
            // (`v == []` / `v == [1, 2]`: 比べる相手の Vec の要素の型)
            Some(Ty::Struct(sid)) if self.prog.structs[self.prog.base_of(sid) as usize].name == "Vec" => {
                self.prog.structs[sid as usize].args.first().copied()
            }
            _ => None,
        };
        // (Vec から取った要素の型は目安: 要素の型が違えばそちらにする (`v == ["a"]` で v: Vec<String>))
        let mut soft = matches!(expected, Some(Ty::Struct(_)));
        // `[&a as &dyn Tr, &b]`: 要素のどれかが `&dyn` なら全体をそれに揃える
        if !matches!(elem, Some(Ty::Ref(_, Ty::Dyn(_)))) {
            if let Some(d) = elems.iter().find_map(|e| match e {
                Expr::Cast(_, ty) => match self.parse_ty(ty) {
                    Ok(t @ Ty::Ref(_, Ty::Dyn(_))) => Some(t),
                    _ => None,
                },
                _ => None,
            }) {
                elem = Some(d);
            }
        }
        let mut vals = vec![];
        for e in elems {
            let (v, t) = self.expr(e, elem)?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            match elem {
                Some(et) if soft && t != et => elem = Some(t),
                Some(et) => self.same(SP, t, et)?,
                None => elem = Some(t),
            }
            soft = false;
            vals.push(v);
        }
        let Some(elem) = elem else {
            return err(SP, "type annotations needed for an empty array");
        };
        let t = Ty::Array(intern(elem), vals.len() as u32);
        let base = self.temp(t);
        let (size, _) = self.prog.size_align(elem);
        for (i, v) in vals.into_iter().enumerate() {
            self.write(Place::Mem(base, (i as u32 * size) as i32, elem), v);
        }
        Ok((Some(base), t))
    }

    /// 配列の長さ (リテラルか const)
    pub(crate) fn array_len(&self, e: &Expr) -> R<u32> {
        match e {
            Expr::Lit(l, _) if matches!(lit::lit(l), Some(Lit::Int(_))) => {
                let Some(Lit::Int(i)) = lit::lit(l) else { unreachable!() };
                i.base10_digits().parse::<u32>().or_else(|er| err(SP, er))
            }
            Expr::Path(..) if self.const_generic(e).is_some() => Ok(self.const_generic(e).unwrap()),
            Expr::Path(_, p) if ident_of(p).is_some() || self.const_of(e).is_some() => {
                // (関連定数 `S::N` は const の名前 `S::N` で引く)
                let name = match self.const_of(e) {
                    Some(ci) => self.prog.consts[ci as usize].name.clone(),
                    None => ident_of(p).unwrap().to_string(),
                };
                let ts = crate::types::TySyn::Array(Box::new(crate::types::TySyn::Prim(Ty::U8)), crate::types::ArrLen::Const(name));
                match self.prog.resolve(&ts) {
                    Ok(Ty::Array(_, n)) => Ok(n),
                    Ok(_) => unreachable!(),
                    Err(er) => err(SP, er),
                }
            }
            _ => err(SP, "array length must be a literal or a const"),
        }
    }

    pub(crate) fn repeat_lit(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Repeat(x, len) = e else { unreachable!() };
        let n = self.array_len(len)?;
        let exp = match expected {
            Some(Ty::Array(t, _)) => Some(*t),
            _ => None,
        };
        let (v, elem) = self.expr(x, exp)?;
        if elem == Ty::Never {
            return Ok((None, Ty::Never));
        }
        let t = Ty::Array(intern(elem), n);
        let base = self.temp(t);
        let (size, _) = self.prog.size_align(elem);
        // (定数の整数の配列 (`[0u8; 24]`): 値を並べた 8 byte ずつ書く)
        if let (Some(c), true) = (v.and_then(|v| self.iconst_of(v)), elem.is_int() && size <= 4 && size * n >= 8) {
            let mask = if size == 8 { u64::MAX } else { (1u64 << (size * 8)) - 1 };
            let mut pat = 0u64;
            for k in 0..(8 / size) {
                pat |= ((c as u64) & mask) << (k * size * 8);
            }
            let total = size * n;
            let mut fl = MemFlagsData::new();
            fl.set_notrap();
            let mut off = 0;
            for (w, ty) in [(8u32, types::I64), (4, types::I32), (2, types::I16), (1, types::I8)] {
                while off + w <= total {
                    let pv = self.b.ins().iconst(ty, (pat & if w == 8 { u64::MAX } else { (1u64 << (w * 8)) - 1 }) as i64);
                    self.b.ins().store(fl, pv, base, off as i32);
                    off += w;
                }
            }
            return Ok((Some(base), t));
        }
        if n <= 16 {
            for i in 0..n {
                self.write(Place::Mem(base, (i * size) as i32, elem), v);
            }
        } else {
            // 大きい配列はループで埋める
            let i = self.b.declare_var(types::I64);
            let z = self.b.ins().iconst(types::I64, 0);
            self.b.def_var(i, z);
            let head = self.b.create_block();
            let body = self.b.create_block();
            let exit = self.b.create_block();
            self.b.ins().jump(head, &[]);
            self.b.switch_to_block(head);
            let iv = self.b.use_var(i);
            let c = self.b.ins().icmp_imm_u(IntCC::UnsignedLessThan, iv, n as i64);
            self.b.ins().brif(c, body, &[], exit, &[]);
            self.b.switch_to_block(body);
            let addr = self.elem_addr(base, iv, elem);
            self.write(Place::Mem(addr, 0, elem), v);
            let nx = self.b.ins().iadd_imm_s(iv, 1);
            self.b.def_var(i, nx);
            self.b.ins().jump(head, &[]);
            self.b.switch_to_block(exit);
        }
        Ok((Some(base), t))
    }
}
