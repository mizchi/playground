use super::*;
use crate::parse::{Block as PBlock, Expr, GArg, Pat};
use crate::program::{FnSig, PPath};
use crate::types::{Ty, intern};
use crate::acg::clif::{MemFlagsData, types};
use crate::minclif::{FloatCC, IntCC, Type, Value};

/// turbofish (空なら None)
fn tf_of(tf: &[GArg]) -> Option<&[GArg]> {
    (!tf.is_empty()).then_some(tf)
}

impl Cx<'_> {
    /// 型 t がイテレータ (next を持つ struct) か
    pub(crate) fn is_iterator_ty(&self, t: Ty) -> bool {
        match t {
            Ty::Struct(sid) => self.prog.method(sid, "next").is_some() || self.prog.default_method(sid, "next").is_some(),
            _ => false,
        }
    }

    /// `v.extend(arg)` (Vec / BinaryHeap): IntoIterator の代わりに、イテレータでない arg を
    /// `arg.into_iter()` (値) / `arg.iter()` (参照・配列) にする。要素が `&T` なら __extend_ref (T: Copy)
    pub(crate) fn extend_call(&mut self, mc: &Expr, expected: Option<Ty>) -> R<Option<V>> {
        let Expr::MethodCall(receiver, method, _, margs) = mc else { return Ok(None) };
        if method != "extend" || margs.len() != 1 {
            return Ok(None);
        }
        // (下で作り直した `recv.extend(__exr_ext)` は普通のメソッド呼び出し)
        if matches!(&margs[0], Expr::Path(_, p) if is_ident(p, "__exr_ext")) {
            return Ok(None);
        }
        let Some(rt) = self.peek(receiver).map(|t| self.strip_refs(t)) else {
            return Ok(None);
        };
        let Ty::Struct(sid) = rt else { return Ok(None) };
        let bname = self.prog.structs[self.prog.base_of(sid) as usize].name.clone();
        let is_vec = matches!(bname.as_str(), "Vec" | "BinaryHeap") && self.prog.method(sid, "__extend_ref").is_some();
        // (ほかのコレクションと String は、引数をイテレータにしてから prelude の extend へ)
        let other = matches!(bname.as_str(), "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "String" | "VecDeque")
            && self.prog.method(sid, "extend").is_some();
        if !is_vec && !other {
            return Ok(None);
        }
        let elem = if is_vec {
            *self.prog.structs[sid as usize].args.first().ok_or("internal: bad Vec")?
        } else {
            Ty::Unit
        };
        let arg = &margs[0];
        let at = self.peek(arg);
        let paren = |e: &Expr| Expr::Paren(Box::new(e.clone()));
        let iter_e: Option<Expr> = match at {
            None => None,
            Some(t) if self.is_iterator_ty(t) => None,
            Some(Ty::Ref(_, Ty::Array(..))) if matches!(strip_paren(arg), Expr::Ref(..)) => {
                Some(mcall(path_expr("__exr_arr"), "iter", vec![]))
            }
            Some(Ty::Ref(_, _)) => match strip_paren(arg) {
                Expr::Ref(_, inner) => Some(mcall(paren(inner), "iter", vec![])),
                _ => Some(mcall(paren(arg), "iter", vec![])),
            },
            Some(Ty::Array(e, _)) => {
                if self.needs_drop(*e) {
                    return err(SP, "`extend` with an array of values that need drop is not supported");
                }
                Some(mcall(path_expr("__exr_arr"), "iter", vec![]))
            }
            Some(_) => Some(mcall(paren(arg), "into_iter", vec![])),
        };
        let iter_e: Option<&'static Expr> = iter_e.map(|e| self.keep_expr(e));
        self.scopes.push(vec![]);
        // 配列は一度ローカルに置いてから iter() する (リテラルの `[1, 2].iter()` は場所でないため)
        match (at, strip_paren(arg)) {
            (Some(Ty::Array(..)), _) => {
                let (v, t) = self.expr(arg, None)?;
                self.bind("__exr_arr".into(), t, v);
            }
            (Some(Ty::Ref(_, Ty::Array(e, _))), Expr::Ref(_, inner)) => {
                if self.needs_drop(**e) {
                    return err(SP, "`extend` with an array of values that need drop is not supported");
                }
                let (v, t) = self.expr(inner, None)?;
                self.bind("__exr_arr".into(), t, v);
            }
            _ => {}
        }
        let (v, t) = match iter_e {
            Some(e) => self.expr(e, None)?,
            None => self.expr(arg, None)?,
        };
        if t == Ty::Never {
            self.scopes.pop();
            return Ok(Some((None, Ty::Never)));
        }
        let item = self.prog.assoc_of(t, "Item");
        let by_ref = is_vec && matches!(item, Some(Ty::Ref(_, i)) if *i == elem);
        self.bind("__exr_ext".into(), t, v);
        let r = Expr::Paren(receiver.clone());
        let e: Expr = if by_ref {
            mcall(r, "__extend_ref", vec![path_expr("__exr_ext")])
        } else {
            mcall(r, "extend", vec![path_expr("__exr_ext")])
        };
        let e = self.keep_expr(e);
        let res = self.expr(e, expected)?;
        self.pop_scope();
        Ok(Some(res))
    }
}

impl Cx<'_> {
    /// break / continue の行き先のループ (ラベルがなければいちばん内側のループ。ラベル付きブロックは飛ばす)
    pub(crate) fn loop_index(&self, label: Option<&str>, is_break: bool) -> Option<usize> {
        self.loops.iter().rposition(|l| match label {
            Some(n) => l.label.as_deref() == Some(n) && (is_break || !l.is_block),
            None => !l.is_block,
        })
    }

    /// `'a: { .. break 'a v; .. }`
    pub(crate) fn labeled_block(&mut self, label: String, blk: &PBlock, expected: Option<Ty>) -> R<V> {
        let exit = self.b.create_block();
        self.loops.push(Loop {
            cont: exit,
            brk: exit,
            breaks: 0,
            label: Some(label),
            val: expected.filter(|_| false),
            is_block: true,
            depth: self.scopes.len(),
            temps: self.temps.len(),
        });
        let (v, t) = self.block(blk, expected)?;
        let lp = self.loops.pop().unwrap();
        if lp.breaks == 0 {
            return Ok((v, t));
        }
        // 末尾の値も break と同じく exit の引数で渡す
        if t != Ty::Never {
            if let Some(w) = lp.val {
                self.same(SP, t, w)?;
            } else if t != Ty::Unit {
                return err(SP, "a labeled block that breaks without a value must have type `()`");
            }
            let args: Vec<Value> = v.into_iter().collect();
            self.b.ins().jump(exit, &args);
        }
        self.b.switch_to_block(exit);
        match lp.val {
            Some(t) => Ok((self.b.block_params(exit).first().copied(), t)),
            None => Ok((None, Ty::Unit)),
        }
    }
}

/// スライスパターンの (前の要素, `..` (と `x @ ..`), 後ろの要素)
pub(crate) fn slice_parts(sp: &Pat) -> (Vec<&Pat>, Option<&Pat>, Vec<&Pat>) {
    let mut pre = vec![];
    let mut post = vec![];
    let mut rest = None;
    let Pat::Slice(elems) = sp else { return (pre, rest, post) };
    for e in elems {
        let is_rest = match e {
            Pat::Rest => true,
            Pat::Ident(_, _, _, Some(sub)) => matches!(&**sub, Pat::Rest),
            _ => false,
        };
        if is_rest && rest.is_none() {
            rest = Some(e);
        } else if rest.is_some() {
            post.push(e);
        } else {
            pre.push(e);
        }
    }
    (pre, rest, post)
}

impl Cx<'_> {
    /// ローカル変数の置き場所の値 (block の引数で渡す形: 集約型はアドレス)
    pub(crate) fn loc_arg(&mut self, loc: Loc, t: Ty) -> Option<Value> {
        match loc {
            Loc::Var(v) => Some(self.b.use_var(v)),
            Loc::Slot(ss) if t.is_agg() => Some(self.b.ins().stack_addr(PTR, ss, 0)),
            Loc::Slot(ss) => Some(self.b.ins().stack_load(clif_ty(t).unwrap_or(PTR), ss, 0)),
            Loc::Ptr(v) => Some(v),
            Loc::Zst => None,
        }
    }

    /// block の引数にするときの型 (サイズ 0 なら None)
    pub(crate) fn param_ty(&self, t: Ty) -> Option<Type> {
        if t.is_agg() || t.is_fat() { Some(PTR) } else { clif_ty(t) }
    }

    /// スライスパターンの対象: (先頭, 長さ, 要素型, 参照越しなら束縛のモード)
    pub(crate) fn slice_scrutinee(&mut self, p: Place, span: Sp) -> R<(Value, Value, Ty, Option<bool>)> {
        let mut p = p;
        let mut rmode = None;
        loop {
            match p.ty() {
                Ty::Array(e, n) => {
                    let Place::Mem(a, o, _) = p else {
                        return err(span, "internal: array pattern on a register");
                    };
                    let base = self.addr_plus(a, o);
                    let len = self.b.ins().iconst(types::I64, n as i64);
                    return Ok((base, len, *e, rmode));
                }
                Ty::Slice(e) => {
                    let Place::Mem(a, o, _) = p else {
                        return err(span, "internal: bad slice place");
                    };
                    let fat = self.addr_plus(a, o);
                    let base = self.b.ins().load(PTR, mf(), fat, 0);
                    let len = self.b.ins().load(types::I64, mf(), fat, 8);
                    return Ok((base, len, *e, rmode));
                }
                Ty::Ref(m, Ty::Slice(e)) => {
                    let fat = match p {
                        Place::Mem(a, o, _) => self.addr_plus(a, o),
                        Place::Var(v, _) => self.b.use_var(v),
                        _ => return err(span, "internal: bad slice place"),
                    };
                    let base = self.b.ins().load(PTR, mf(), fat, 0);
                    let len = self.b.ins().load(types::I64, mf(), fat, 8);
                    return Ok((base, len, **e, Some(rmode.unwrap_or(m))));
                }
                Ty::Ref(m, _) => {
                    let (q, _) = self.pat_deref_once(p);
                    p = q;
                    rmode = Some(rmode.unwrap_or(m));
                }
                t => return err(span, format!("slice pattern on `{}`", self.prog.tn(t))),
            }
        }
    }

    /// 前と後ろの要素の place
    pub(crate) fn slice_elems<'p>(
        &mut self,
        base: Value,
        len: Value,
        elem: Ty,
        pre: &[&'p Pat],
        post: &[&'p Pat],
    ) -> Vec<(Place, &'p Pat)> {
        let (es, _) = self.prog.size_align(elem);
        let mut out = vec![];
        for (i, sub) in pre.iter().enumerate() {
            let a = self.b.ins().iadd_imm_s(base, (i * es as usize) as i64);
            out.push((Place::Mem(a, 0, elem), *sub));
        }
        for (j, sub) in post.iter().enumerate() {
            let idx = self.b.ins().iadd_imm_s(len, -((post.len() - j) as i64));
            let off = self.b.ins().imul_imm_s(idx, es as i64);
            let a = self.b.ins().iadd(base, off);
            out.push((Place::Mem(a, 0, elem), *sub));
        }
        out
    }
}

impl Cx<'_> {
    /// char を UTF-8 にして一時領域に置き、`&str` の fat pointer にする
    pub(crate) fn char_to_str(&mut self, c: Value, span: Sp) -> R<Value> {
        let ss = self.b.create_stack_slot(4, 1);
        let buf = self.b.ins().stack_addr(PTR, ss, 0);
        let bt = Ty::Ptr(true, intern(Ty::U8));
        let (n, _) = self.call_named("__exr_char_utf8", vec![Ty::Unit], &[(c, Ty::Char), (buf, bt)], span)?;
        Ok(self.make_fat_v(buf, n.unwrap()))
    }

    /// `recv.name(args)` を prelude の `{prefix}{name}(recv, args)` にする (なければ None)。
    /// 引数の `&str` には char (UTF-8 にする) と `&String` も渡せる
    pub(crate) fn helper_method(&mut self, prefix: &str, name: &str, mc: &Expr, recv: (Value, Ty)) -> R<Option<V>> {
        let Expr::MethodCall(_, _, turbofish, margs) = mc else { return Ok(None) };
        let f = format!("{prefix}{name}");
        let Some(&id) = self.prog.index.get(&f) else { return Ok(None) };
        let sig = self.prog.sigs[id as usize].clone();
        if sig.params.len() != margs.len() + 1 {
            return err(SP, format!("`{name}` takes {} arguments", sig.params.len() - 1));
        }
        let args: Vec<&Expr> = margs.iter().collect();
        // 本物の型引数を持つもの (`parse::<T>`) は普通の呼び出し
        if !sig.generics.iter().all(|g| g.0 == "G") {
            let exp = self.lets.get(&(mc as *const Expr as usize)).copied();
            return self.call_fn(id, Some(recv), &args, tf_of(turbofish), exp, SP).map(Some);
        }
        let sref = Ty::Ref(false, intern(Ty::Str));
        let mut vals = vec![recv];
        for (a, &pt) in args.iter().zip(&sig.params[1..]) {
            if pt == sref && self.peek(a) == Some(Ty::Char) {
                let (v, _) = self.expr(a, Some(Ty::Char))?;
                let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
                let fat = self.char_to_str(v, SP)?;
                vals.push((fat, sref));
                continue;
            }
            let (v, mut t) = self.expr(a, Some(pt))?;
            if t == Ty::Never {
                return Ok(Some((None, Ty::Never)));
            }
            let mut v = v;
            // `&String` → `&str`
            if let (true, Ty::Ref(_, Ty::Struct(sid)), Some(val)) = (pt == sref, t, v) {
                if let Some((fat, ft)) = self.deref_to_slice(val, *sid, false) {
                    v = Some(fat);
                    t = ft;
                }
            }
            // `&&str` → `&str` (fat pointer の組を指すアドレスがそのまま &str の値)
            if let (Ty::Ref(_, Ty::Ref(_, Ty::Str)), true) = (t, pt == sref) {
                t = sref;
            }
            self.same(SP, t, pt)?;
            if let Some(v) = v {
                vals.push((v, t));
            }
        }
        self.call_named(&f, vec![Ty::Unit], &vals, SP).map(Some)
    }
}

impl Cx<'_> {
    /// 範囲 `a..b` / `a..=b` / `..b` / `a..` の (lo, hi)
    pub(crate) fn range_bounds(&mut self, rg: &Expr, len: Value) -> R<(Value, Value)> {
        let Expr::Range(start, end, inclusive) = rg else {
            return err(SP, "internal: expected a range");
        };
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
        Ok((lo, hi))
    }

    /// バイト位置 idx (<= len) が UTF-8 の文字の境界か (i8 の 0/1)
    pub(crate) fn char_boundary(&mut self, ptr: Value, len: Value, idx: Value) -> Value {
        let done = self.b.create_block();
        let res = self.b.append_block_param(done, types::I8);
        let load = self.b.create_block();
        let at_end = self.b.ins().icmp(IntCC::Equal, idx, len);
        let one = self.b.ins().iconst(types::I8, 1);
        self.b.ins().brif(at_end, done, &[one], load, &[]);
        self.b.switch_to_block(load);
        let a = self.b.ins().iadd(ptr, idx);
        let byte = self.b.ins().load(types::I8, mf(), a, 0);
        let top = self.b.ins().band_imm_u(byte, 0xC0);
        let ok = self.b.ins().icmp_imm_u(IntCC::NotEqual, top, 0x80);
        self.b.ins().jump(done, &[ok]);
        self.b.switch_to_block(done);
        res
    }
}

// --- clone (derive(Clone) など) ---------------------------------------------------

impl Cx<'_> {
    /// `x.clone()` (自分の clone メソッドがない型): 整数などはそのまま読み、ほかは __exr_clone::<T>
    pub(crate) fn clone_call(&mut self, mc: &Expr) -> R<Option<V>> {
        let Expr::MethodCall(receiver, method, _, margs) = mc else { return Ok(None) };
        if method != "clone" || !margs.is_empty() {
            return Ok(None);
        }
        if let Some(pt) = self.peek(receiver) {
            let mut t = pt;
            while let Ty::Ref(_, i) = t {
                if t.is_fat() {
                    break;
                }
                t = *i;
            }
            if let Ty::Struct(sid) = t {
                if self.prog.method(sid, "clone").is_some() {
                    return Ok(None);
                }
            }
        }
        // (`&str` / `&[T]` の clone は参照のコピー)
        let (v, vt) = self.operand(receiver, None, true)?;
        let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
        let (a, t) = self.ord_addr(v, vt);
        // (型が分からずに評価してから、自分の clone を持つ型だったとき)
        if let Ty::Struct(sid) = t {
            if let Some(id) = self.prog.method(sid, "clone") {
                return self.call_fn(id, Some((a, Ty::Ref(false, intern(t)))), &[], None, None, SP).map(Some);
            }
        }
        if !self.needs_drop(t) {
            let p = Place::Mem(a, 0, t);
            return Ok(Some(self.read(p)));
        }
        if !self.prog.index.contains_key("__exr_clone") {
            return err(SP, "internal: `__exr_clone` is missing from the prelude");
        }
        let r = Ty::Ref(false, intern(t));
        self.call_named("__exr_clone", vec![t], &[(a, r)], SP).map(Some)
    }

    /// `__exr_clone::<T>(x: &T) -> T` の本体
    pub(crate) fn clone_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let mut ps = self.b.func_params();
        let Some(Ty::Ref(_, t)) = sig.params.first() else {
            return Err("internal: bad clone glue signature".into());
        };
        let t = **t;
        if is_sret(sig) {
            let dst = ps.remove(0);
            let src = ps[0];
            self.clone_into(src, dst, t)?;
            self.b.ins().return_(&[]);
        } else {
            let src = ps[0];
            match clif_ty(t) {
                Some(ct) => {
                    // (Box などスカラーでも drop の要る型は clone してから返す)
                    let tmp = self.temp(t);
                    self.clone_into(src, tmp, t)?;
                    let v = self.b.ins().load(ct, mf(), tmp, 0);
                    self.b.ins().return_(&[v]);
                }
                None => {
                    self.b.ins().return_(&[]);
                }
            }
        }
        Ok(())
    }

    /// src にある t の値の clone を dst に書く
    pub(crate) fn clone_into(&mut self, src: Value, dst: Value, t: Ty) -> R<()> {
        if !self.needs_drop(t) {
            self.copy(dst, src, t);
            return Ok(());
        }
        match t {
            Ty::Struct(sid) => {
                // 自分の clone (String / Vec / 手書きの impl Clone): それを呼ぶ
                if let Some(id) = self.prog.method(sid, "clone") {
                    let sig0 = self.prog.sigs[id as usize].clone();
                    let (fid, sig) = if sig0.is_generic() {
                        let mut sol = vec![None; sig0.generics.len()];
                        if let Some(st) = sig0.self_ty {
                            self.prog.match_ty(st, t, &mut sol);
                        }
                        let r = Ty::Ref(false, intern(t));
                        self.resolve_call(id, &[r], sol, None, SP)?
                    } else {
                        (id, sig0)
                    };
                    let f = self.fref(fid, &sig);
                    if is_sret(&sig) {
                        self.b.ins().call(f, &[dst, src]);
                    } else {
                        let call = self.b.ins().call(f, &[src]);
                        if let Some(&v) = self.b.inst_results(call).first() {
                            self.b.ins().store(mf(), v, dst, 0);
                        }
                    }
                    return Ok(());
                }
                let sd = &self.prog.structs[sid as usize];
                if !sd.is_enum {
                    let fields = sd.fields.clone();
                    for (_, ft, off) in fields {
                        let (s2, d2) = (self.addr_plus(src, off as i32), self.addr_plus(dst, off as i32));
                        self.clone_elem(s2, d2, ft)?;
                    }
                    return Ok(());
                }
                // enum: 判別値をコピーし、そのバリアントのフィールドを clone
                let tag_ty = sd.tag;
                let variants: Vec<(i64, Vec<(String, Ty, u32)>)> = sd.variants.iter().map(|v| (v.discr, v.fields.clone())).collect();
                let (size, _) = self.prog.size_align(t);
                let _ = size;
                let ct = clif_ty(tag_ty).unwrap();
                let tag = self.b.ins().load(ct, mf(), src, 0);
                // (まずビットごとにコピーしてから、要るフィールドを clone で上書きする)
                self.copy(dst, src, t);
                let done = self.b.create_block();
                for (d, fs) in variants {
                    if !fs.iter().any(|f| self.needs_drop(f.1)) {
                        continue;
                    }
                    let yes = self.b.create_block();
                    let next = self.b.create_block();
                    let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, d);
                    self.b.ins().brif(c, yes, &[], next, &[]);
                    self.b.switch_to_block(yes);
                    for (_, ft, off) in fs {
                        let (s2, d2) = (self.addr_plus(src, off as i32), self.addr_plus(dst, off as i32));
                        self.clone_elem(s2, d2, ft)?;
                    }
                    self.b.ins().jump(done, &[]);
                    self.b.switch_to_block(next);
                }
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                Ok(())
            }
            Ty::Array(e, n) => {
                let (es, _) = self.prog.size_align(*e);
                for i in 0..n {
                    let (s2, d2) = (self.addr_plus(src, (i * es) as i32), self.addr_plus(dst, (i * es) as i32));
                    self.clone_elem(s2, d2, *e)?;
                }
                Ok(())
            }
            Ty::Box(inner) if !matches!(inner, Ty::Dyn(_)) => {
                let Some(&alloc) = self.prog.index.get(crate::prelude::BOX_ALLOC) else {
                    return Err("internal: the allocator is missing from the prelude".into());
                };
                let (size, _) = self.prog.size_align(*inner);
                let asig = self.prog.sigs[alloc as usize].clone();
                let fr = self.fref(alloc, &asig);
                let sz = self.b.ins().iconst(types::I64, size.max(1) as i64);
                let call = self.b.ins().call(fr, &[sz]);
                let np = self.b.inst_results(call)[0];
                let op = self.b.ins().load(PTR, mf(), src, 0);
                self.clone_elem(op, np, *inner)?;
                self.b.ins().store(mf(), np, dst, 0);
                Ok(())
            }
            t => Err(format!("`{}` cannot be cloned", self.prog.tn(t))),
        }
    }

    /// 要素 1 つの clone: drop の要らない型はコピー、ほかは __exr_clone::<t> を呼ぶ
    pub(crate) fn clone_elem(&mut self, src: Value, dst: Value, t: Ty) -> R<()> {
        if !self.needs_drop(t) {
            self.copy(dst, src, t);
            return Ok(());
        }
        let r = Ty::Ref(false, intern(t));
        let (v, _) = self.call_named("__exr_clone", vec![t], &[(src, r)], SP)?;
        match v {
            Some(v) if t.is_agg() => self.copy(dst, v, t),
            Some(v) => {
                self.b.ins().store(mf(), v, dst, 0);
            }
            None => {}
        }
        Ok(())
    }
}

// --- Default ---------------------------------------------------------------------

impl Cx<'_> {
    /// `Default::default()` / `T::default()` / `i32::default()` / `Vec::default()` (手書きの default がない型)
    pub(crate) fn default_call(&mut self, c: &Expr, expected: Option<Ty>) -> R<Option<V>> {
        let Expr::Call(func, cargs) = c else { return Ok(None) };
        let Expr::Path(qself, path) = &**func else { return Ok(None) };
        if !cargs.is_empty() || path.segs.last().map(|s| s.name != "default").unwrap_or(true) {
            return Ok(None);
        }
        if !self.prog.index.contains_key("__exr_default") {
            return Ok(None);
        }
        // `X::default()` で X 自身の default がある (手書きの impl Default for X / 自分の関数) ならそれを呼ぶ
        if qself.is_none() && path.segs.len() == 2 && path.segs[0].name != "Default" {
            if let Some(id) = self.prog.fn_by_p(&PPath::of(path), self.self_sid) {
                if self.prog.sigs[id as usize].has_body {
                    return Ok(None);
                }
            }
            if self.callee(func).is_some_and(|(id, _)| self.prog.sigs[id as usize].has_body) {
                return Ok(None);
            }
        }
        let exp = expected.or_else(|| self.lets.get(&(c as *const Expr as usize)).copied());
        // (`m::S::default()`: モジュールの部分は飛ばす)
        let stripped = if qself.is_none() { self.prog.strip_mods_p(&PPath::of(path)) } else { None };
        let mut path = path.clone();
        if let Some(q) = stripped {
            let k = path.segs.len() - q.segs.len();
            path = crate::parse::Path { global: false, segs: path.segs[k..].to_vec() };
        }
        let path = &path;
        let t = if let Some(q) = qself {
            self.parse_ty(q).ok()
        } else {
            let segs: Vec<String> = path.segs.iter().map(|s| s.name.clone()).collect();
            match segs.as_slice() {
                [d, _] if d == "Default" => exp,
                [x, _] => {
                    let ty = crate::parse::Ty::Path(None, path_of(&path.segs[0].name));
                    match self.parse_ty(&ty) {
                        Ok(t) => Some(t),
                        // ジェネリックな型の名前だけ (`Vec::default()`): 期待型から
                        // (ジェネリックな別名 `FxMap::default()` も)
                        Err(_) => exp.filter(|&e| {
                            self.prog.aliases.contains_key(x.as_str())
                                || matches!(e, Ty::Struct(sid) if self.prog.structs[self.prog.base_of(sid) as usize].name == *x)
                        }),
                    }
                }
                [_, _, ..] if segs.len() >= 3 && segs[segs.len() - 2] == "Default" => exp,
                _ => None,
            }
        };
        let Some(t) = t else {
            return err(SP, "cannot infer the type of `default()`; annotate it");
        };
        self.call_named("__exr_default", vec![t], &[], SP).map(Some)
    }

    /// `__exr_default::<T>() -> T` の本体
    pub(crate) fn default_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let ps = self.b.func_params();
        let t = sig.ret;
        if is_sret(sig) {
            self.default_into(ps[0], t)?;
            self.b.ins().return_(&[]);
        } else {
            match clif_ty(t) {
                Some(ct) => {
                    let tmp = self.temp(t);
                    self.default_into(tmp, t)?;
                    let v = self.b.ins().load(ct, mf(), tmp, 0);
                    self.b.ins().return_(&[v]);
                }
                None => {
                    self.b.ins().return_(&[]);
                }
            }
        }
        Ok(())
    }

    /// dst に t の既定値を書く
    pub(crate) fn default_into(&mut self, dst: Value, t: Ty) -> R<()> {
        // 手書きの `impl Default for S`
        if let Ty::Struct(sid) = t {
            let bname = self.prog.structs[self.prog.base_of(sid) as usize].name.clone();
            if let Some(&id) = self.prog.index.get(&format!("{bname}::default")) {
                let sig0 = self.prog.sigs[id as usize].clone();
                let (fid, sig) = if sig0.is_generic() {
                    let mut sol = vec![None; sig0.generics.len()];
                    self.prog.match_ty(sig0.ret, t, &mut sol);
                    self.resolve_call(id, &[], sol, Some(t), SP)?
                } else {
                    (id, sig0)
                };
                let f = self.fref(fid, &sig);
                if is_sret(&sig) {
                    self.b.ins().call(f, &[dst]);
                } else {
                    let call = self.b.ins().call(f, &[]);
                    if let Some(&v) = self.b.inst_results(call).first() {
                        self.b.ins().store(mf(), v, dst, 0);
                    }
                }
                return Ok(());
            }
        }
        // それ以外は 0 で埋め、フィールド (と Box の中身) を辿る
        let (size, _) = self.prog.size_align(t);
        if size > 0 {
            let z = self.b.ins().iconst(types::I8, 0);
            let n = self.b.ins().iconst(types::I64, size as i64);
            let _ = (z, n);
            let zero = self.b.ins().iconst(types::I64, 0);
            let mut off = 0;
            while off + 8 <= size {
                self.b.ins().store(mf(), zero, dst, off as i32);
                off += 8;
            }
            let z8 = self.b.ins().iconst(types::I8, 0);
            while off < size {
                self.b.ins().store(mf(), z8, dst, off as i32);
                off += 1;
            }
        }
        match t {
            Ty::Struct(sid) if !self.prog.structs[sid as usize].is_enum => {
                let fields = self.prog.structs[sid as usize].fields.clone();
                for (_, ft, off) in fields {
                    if matches!(ft, Ty::Struct(_) | Ty::Box(_) | Ty::Array(..)) {
                        let d = self.addr_plus(dst, off as i32);
                        self.default_into(d, ft)?;
                    }
                }
            }
            Ty::Array(e, n) => {
                let (es, _) = self.prog.size_align(*e);
                if matches!(e, Ty::Struct(_) | Ty::Box(_)) {
                    for i in 0..n {
                        let d = self.addr_plus(dst, (i * es) as i32);
                        self.default_into(d, *e)?;
                    }
                }
            }
            Ty::Box(inner) => {
                let Some(&alloc) = self.prog.index.get(crate::prelude::BOX_ALLOC) else {
                    return Err("internal: the allocator is missing from the prelude".into());
                };
                let (size, _) = self.prog.size_align(*inner);
                let asig = self.prog.sigs[alloc as usize].clone();
                let fr = self.fref(alloc, &asig);
                let sz = self.b.ins().iconst(types::I64, size.max(1) as i64);
                let call = self.b.ins().call(fr, &[sz]);
                let np = self.b.inst_results(call)[0];
                self.default_into(np, *inner)?;
                self.b.ins().store(mf(), np, dst, 0);
            }
            _ => {}
        }
        Ok(())
    }
}

impl Cx<'_> {
    /// const は式そのものを埋め込む
    pub(crate) fn const_value(&mut self, p: &Expr, ci: u32) -> R<V> {
        let _ = p;
        let prog = self.prog;
        let c = &prog.consts[ci as usize];
        let (t, src) = (c.ty, c.expr.clone());
        let ce: Expr = crate::parse::parse_expr(&src).or_else(|er| err(SP, er))?;
        let ce = self.keep_expr(ce);
        // (行番号は今は出さない: Sp は場所を持たない)
        let (v, vt) = self.expr(ce, Some(t)).map_err(|e| format!("{}:1: in constant `{}`: {e}", SP, c.name))?;
        self.same(SP, vt, t)?;
        Ok((v, t))
    }
}

// --- 浮動小数点のメソッド ---------------------------------------------------------

impl Cx<'_> {
    /// f32 / f64 の組み込みメソッド (命令になるものは命令、ほかは libm)
    pub(crate) fn float_method(&mut self, mc: &Expr, t: Ty) -> R<Option<V>> {
        let Expr::MethodCall(receiver, method, _, margs) = mc else { return Ok(None) };
        let name = method.to_string();
        let _ = clif_ty(t);
        let f32 = t == Ty::F32;
        let lib1 = [
            "exp", "exp2", "ln", "log10", "log2", "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh", "asinh", "acosh", "atanh",
            "cbrt", "round", "exp_m1", "ln_1p",
        ];
        let lib2 = ["powf", "atan2", "hypot"];
        let known = [
            "sqrt", "abs", "floor", "ceil", "trunc", "fract", "mul_add", "copysign", "min", "max", "is_nan", "is_finite", "is_infinite",
            "is_sign_negative", "is_sign_positive", "signum", "to_bits", "recip", "to_degrees", "to_radians", "clamp", "powi", "log",
            "total_cmp", "abs_sub", "is_normal",
        ];
        if !(known.contains(&name.as_str()) || lib1.contains(&name.as_str()) || lib2.contains(&name.as_str())) {
            return Ok(None);
        }
        // 受け手 (参照なら中身)
        let (a, at) = if matches!(self.peek(receiver), Some(Ty::Ref(..))) {
            self.expr(receiver, None)?
        } else {
            self.expr(receiver, Some(t))?
        };
        let Some(mut a) = a else { return Ok(Some((None, Ty::Never))) };
        let mut at = at;
        while let Ty::Ref(_, i) = at {
            a = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), a, 0);
            at = *i;
        }
        let args: Vec<&Expr> = margs.iter().collect();
        let arg = |this: &mut Self, i: usize, want: Ty| -> R<Value> {
            let Some(e) = args.get(i) else {
                return err(SP, format!("`{name}` needs an argument"));
            };
            let (v, vt) = this.expr(e, Some(want))?;
            let mut v = v.unwrap();
            let mut vt = vt;
            while let Ty::Ref(_, i) = vt {
                v = this.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), v, 0);
                vt = *i;
            }
            this.same(SP, vt, want)?;
            Ok(v)
        };
        let span = SP;
        let libm = |this: &mut Self, f: &str, vals: Vec<Value>| -> R<Value> {
            let n = format!("__exr_{f}{}", if f32 { "f" } else { "" });
            let a: Vec<(Value, Ty)> = vals.into_iter().map(|v| (v, t)).collect();
            let (r, _) = this.call_named(&n, vec![], &a, span)?;
            Ok(r.unwrap())
        };
        let fc = |this: &mut Self, x: f64| -> Value { if f32 { this.b.ins().f32const(x as f32) } else { this.b.ins().f64const(x) } };
        let r: V = match name.as_str() {
            "sqrt" => (Some(self.b.ins().sqrt(a)), t),
            "abs" => (Some(self.b.ins().fabs(a)), t),
            "floor" => (Some(self.b.ins().floor(a)), t),
            "ceil" => (Some(self.b.ins().ceil(a)), t),
            "trunc" => (Some(self.b.ins().trunc(a)), t),
            "fract" => {
                let tr = self.b.ins().trunc(a);
                (Some(self.b.ins().fsub(a, tr)), t)
            }
            "mul_add" => {
                let b = arg(self, 0, t)?;
                let c = arg(self, 1, t)?;
                (Some(self.b.ins().fma(a, b, c)), t)
            }
            "copysign" => {
                let b = arg(self, 0, t)?;
                (Some(self.b.ins().fcopysign(a, b)), t)
            }
            "recip" => {
                let one = fc(self, 1.0);
                (Some(self.b.ins().fdiv(one, a)), t)
            }
            "to_degrees" => {
                let k = fc(self, if f32 { (180.0f32 / std::f32::consts::PI) as f64 } else { 180.0 / std::f64::consts::PI });
                (Some(self.b.ins().fmul(a, k)), t)
            }
            "to_radians" => {
                let v = if f32 {
                    let k = fc(self, (std::f32::consts::PI / 180.0) as f64);
                    self.b.ins().fmul(a, k)
                } else {
                    let k = fc(self, std::f64::consts::PI / 180.0);
                    self.b.ins().fmul(a, k)
                };
                (Some(v), t)
            }
            // Rust の max / min: 片方が NaN ならもう片方
            "max" | "min" => {
                let b = arg(self, 0, t)?;
                let cc = if name == "max" { FloatCC::GreaterThan } else { FloatCC::LessThan };
                let pick_a = self.b.ins().fcmp(cc, a, b);
                let b_nan = self.b.ins().fcmp(FloatCC::Unordered, b, b);
                let use_a = self.b.ins().bor(pick_a, b_nan);
                (Some(self.b.ins().select(use_a, a, b)), t)
            }
            "clamp" => {
                let lo = arg(self, 0, t)?;
                let hi = arg(self, 1, t)?;
                let l = self.b.ins().fcmp(FloatCC::LessThan, a, lo);
                let x = self.b.ins().select(l, lo, a);
                let g = self.b.ins().fcmp(FloatCC::GreaterThan, x, hi);
                (Some(self.b.ins().select(g, hi, x)), t)
            }
            "is_nan" => (Some(self.b.ins().fcmp(FloatCC::Unordered, a, a)), Ty::Bool),
            "is_infinite" => {
                let ab = self.b.ins().fabs(a);
                let inf = fc(self, f64::INFINITY);
                (Some(self.b.ins().fcmp(FloatCC::Equal, ab, inf)), Ty::Bool)
            }
            "is_finite" => {
                let ab = self.b.ins().fabs(a);
                let inf = fc(self, f64::INFINITY);
                (Some(self.b.ins().fcmp(FloatCC::LessThan, ab, inf)), Ty::Bool)
            }
            "is_normal" => {
                let ab = self.b.ins().fabs(a);
                let inf = fc(self, f64::INFINITY);
                let mp = fc(self, if f32 { f32::MIN_POSITIVE as f64 } else { f64::MIN_POSITIVE });
                let c1 = self.b.ins().fcmp(FloatCC::LessThan, ab, inf);
                let c2 = self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, ab, mp);
                (Some(self.b.ins().band(c1, c2)), Ty::Bool)
            }
            "is_sign_negative" | "is_sign_positive" => {
                let it = if f32 { types::I32 } else { types::I64 };
                let bits = self.b.ins().bitcast(it, MemFlagsData::new(), a);
                let c = if name == "is_sign_negative" { IntCC::SignedLessThan } else { IntCC::SignedGreaterThanOrEqual };
                (Some(self.b.ins().icmp_imm_s(c, bits, 0)), Ty::Bool)
            }
            "to_bits" => {
                let it = if f32 { types::I32 } else { types::I64 };
                (Some(self.b.ins().bitcast(it, MemFlagsData::new(), a)), if f32 { Ty::U32 } else { Ty::U64 })
            }
            // NaN は NaN、それ以外は符号つきの 1
            "signum" => {
                let one = fc(self, 1.0);
                let s1 = self.b.ins().fcopysign(one, a);
                let nan = self.b.ins().fcmp(FloatCC::Unordered, a, a);
                (Some(self.b.ins().select(nan, a, s1)), t)
            }
            "powi" => {
                let n = arg(self, 0, Ty::I32)?;
                let f = if f32 { "__exr_powif" } else { "__exr_powi" };
                let (r, _) = self.call_named(f, vec![Ty::Unit], &[(a, t), (n, Ty::I32)], span)?;
                (r, t)
            }
            "abs_sub" => {
                let b = arg(self, 0, t)?;
                let d = self.b.ins().fsub(a, b);
                let z = fc(self, 0.0);
                let g = self.b.ins().fcmp(FloatCC::GreaterThan, d, z);
                (Some(self.b.ins().select(g, d, z)), t)
            }
            // log(base) = ln(x) / ln(base)
            "log" => {
                let b = arg(self, 0, t)?;
                let la = libm(self, "log", vec![a])?;
                let lb = libm(self, "log", vec![b])?;
                (Some(self.b.ins().fdiv(la, lb)), t)
            }
            "total_cmp" => {
                // ビット列を全順序の整数にして比べる
                let b = arg(self, 0, t)?;
                let it = if f32 { types::I32 } else { types::I64 };
                let sh = if f32 { 31 } else { 63 };
                let key = |this: &mut Self, v: Value| -> Value {
                    let bits = this.b.ins().bitcast(it, MemFlagsData::new(), v);
                    let s = this.b.ins().sshr_imm_s(bits, sh);
                    let m = this.b.ins().ushr_imm_u(s, 1);
                    this.b.ins().bxor(bits, m)
                };
                let ka = key(self, a);
                let kb = key(self, b);
                let c = self.cmp_scalar(ka, kb, true);
                let (o, ot) = self.call_named("__exr_ordering", vec![Ty::Unit], &[(c, Ty::I32)], span)?;
                (o, ot)
            }
            n if lib2.contains(&n) => {
                let b = arg(self, 0, t)?;
                let f = if n == "powf" { "pow" } else { n };
                (Some(libm(self, f, vec![a, b])?), t)
            }
            n => {
                let f = match n {
                    "ln" => "log",
                    "exp_m1" => "expm1",
                    "ln_1p" => "log1p",
                    m => m,
                };
                (Some(libm(self, f, vec![a])?), t)
            }
        };
        Ok(Some(r))
    }
}

impl Cx<'_> {
    /// `use std::f64::consts::PI;` で取り込んだ定数の名前 (ローカル変数でも const でもないとき)
    pub(crate) fn float_const_name(&self, p: &Expr) -> Option<f64> {
        let Expr::Path(_, path) = p else { return None };
        let id = ident_of(path)?;
        // (速い判定から: 名前は大文字だけ)
        let n = id.to_string();
        if !n.bytes().all(|b| b.is_ascii_uppercase() || b == b'_' || b.is_ascii_digit()) || n.is_empty() {
            return None;
        }
        if self.lookup(&n).is_some() || self.prog.const_index.contains_key(&n) {
            return None;
        }
        Some(match n.as_str() {
            "PI" => 3.141592653589793_f64,
            "E" => 2.718281828459045_f64,
            "TAU" => 6.283185307179586_f64,
            "SQRT_2" => 1.4142135623730951_f64,
            "LN_2" => 0.6931471805599453_f64,
            "LN_10" => 2.302585092994046_f64,
            "FRAC_PI_2" => 1.5707963267948966_f64,
            "FRAC_PI_3" => 1.0471975511965979_f64,
            "FRAC_PI_4" => 0.7853981633974483_f64,
            "FRAC_PI_6" => 0.5235987755982989_f64,
            "FRAC_1_SQRT_2" => 0.7071067811865476_f64,
            "LOG2_E" => 1.4426950408889634_f64,
            "LOG10_E" => 0.4342944819032518_f64,
            _ => return None,
        })
    }
}

/// (`-1` の式全体のアドレス: 型推論はリテラルでなく単項演算の式を記録していることがある)
/// 自前の構文木では単項演算のノードは式そのもの (`Expr::Unary`) なので、式のアドレスを返す
pub(crate) fn e_addr_of_unary(u: &Expr) -> usize {
    u as *const Expr as usize
}
