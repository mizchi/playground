use super::*;
use crate::acg::clif::MemFlagsData;
use crate::parse::GArg;

impl Cx<'_> {
    /// 配列 / スライスの組み込みメソッド (len / is_empty / get / get_mut / first / last / swap / contains)
    pub(crate) fn builtin_method(&mut self, mc: &Expr) -> R<Option<V>> {
        let Expr::MethodCall(mc_recv, mc_name, mc_tf, mc_args, _) = mc else { unreachable!() };
        let mc_recv: &Expr = mc_recv;
        let _ = (mc_name, mc_tf, mc_args);
        let name = mc_name.to_string();
        // 配列・スライスの sort / binary_search: prelude の関数 (`__exr_slice_sort(&mut a)`)
        let slice_fn = match name.as_str() {
            "sort" | "sort_unstable" => Some(("__exr_slice_sort", true)),
            "sort_by" | "sort_unstable_by" => Some(("__exr_slice_sort_by", true)),
            "sort_by_key" | "sort_unstable_by_key" => Some(("__exr_slice_sort_by_key", true)),
            "sort_by_cached_key" => Some(("__exr_slice_sort_by_cached_key", true)),
            "binary_search" => Some(("__exr_slice_bsearch", false)),
            "binary_search_by" => Some(("__exr_slice_bsearch_by", false)),
            "binary_search_by_key" => Some(("__exr_slice_bsearch_by_key", false)),
            "windows" => Some(("__exr_slice_windows", false)),
            "chunks" => Some(("__exr_slice_chunks", false)),
            "to_vec" => Some(("__exr_slice_to_vec", false)),
            "copy_from_slice" => Some(("__exr_slice_copy_from", true)),
            "clone_from_slice" => Some(("__exr_slice_clone_from", true)),
            "split_at" => Some(("__exr_slice_split_at", false)),
            "split_at_mut" => Some(("__exr_slice_split_at_mut", true)),
            _ => None,
        }
        .filter(|(f, _)| self.prog.index.contains_key(*f));
        // `names.join(", ")` / `parts.concat()` (要素が &str / String の Vec・配列・スライス)
        if matches!(name.as_str(), "join" | "concat")
            && mc_args.len() <= 1
            && self.prog.index.contains_key("__exr_join_str")
        {
            let own = match self.peek(&*mc_recv).map(|t| self.strip_refs(t)) {
                // (Vec でない構造体の join (`path_buf.join("x")`) は組み込みではない)
                Some(Ty::Struct(sid)) => self.vec_fields(sid).is_none(),
                _ => false,
            };
            if !own {
                // (一時値の受け手 `vec![..].join(..)` は `&` を付けて評価し、文の終わりに drop させる)
                let r = &*mc_recv;
                let re: Expr = if is_place_expr(r) { r.clone() } else { Expr::Ref(false, Box::new(Expr::Paren(Box::new(r.clone())))) };
                let (base, len, elem) = self.indexable(&re)?;
                // Vec の Vec の concat: 要素を clone して 1 つの Vec に
                if let (Ty::Struct(es), "concat") = (elem, name.as_str()) {
                    if let Some((_, _, inner)) = self.vec_fields(es) {
                        let fat = self.make_fat_v(base, len);
                        let st = Ty::Ref(false, intern(Ty::Slice(intern(elem))));
                        let (v, t) = self.call_named("__exr_concat_vec", vec![inner], &[(fat, st)], SP)?;
                        return Ok(Some((v, t)));
                    }
                }
                let f = match elem {
                    Ty::Ref(_, Ty::Str) => "__exr_join_str",
                    Ty::Struct(es) if self.prog.structs[es as usize].name == "String" => "__exr_join_string",
                    t => return err(SP, format!("`{name}` on elements of type `{}` is not supported", self.prog.tn(t))),
                };
                let fat = self.make_fat_v(base, len);
                let st = Ty::Ref(false, intern(Ty::Slice(intern(elem))));
                let sref = Ty::Ref(false, intern(Ty::Str));
                let sep = match mc_args.first() {
                    Some(a) if self.peek(a) == Some(Ty::Char) => {
                        let (v, _) = self.expr(a, Some(Ty::Char))?;
                        self.char_to_str(v.unwrap(), SP)?
                    }
                    Some(a) => {
                        let (v, t) = self.expr(a, Some(sref))?;
                        self.same(SP, t, sref)?;
                        v.unwrap()
                    }
                    None => {
                        let e: Expr = Expr::Lit("\"\"".to_string(), crate::parse::Pos::default());
                        self.expr(&e, Some(sref))?.0.unwrap()
                    }
                };
                return self.call_named(f, vec![Ty::Unit], &[(fat, st), (sep, sref)], SP).map(Some);
            }
        }
        // `v.drain(a..b)` / `v.drain(..)`: Vec::__drain(lo, hi)
        if name == "drain" && mc_args.len() == 1 {
            if let (Expr::Range(rg_start, rg_end, rg_closed), Some(Ty::Struct(sid))) = (strip_paren(&mc_args[0]), self.peek(&*mc_recv).map(|t| self.strip_refs(t))) {
                if self.vec_fields(sid).is_some() && self.prog.method(sid, "__drain").is_some() {
                    let r = &*mc_recv;
                    let lo: Expr = match rg_start {
                        Some(e) => (**e).clone(),
                        None => Expr::Lit("0usize".to_string(), crate::parse::Pos::default()),
                    };
                    let hi: Expr = match (rg_end, *rg_closed) {
                        (Some(e), true) => Expr::Binary("+".to_string(), Box::new(Expr::Paren(e.clone())), Box::new(Expr::Lit("1".to_string(), crate::parse::Pos::default())), crate::parse::Span::default()),
                        (Some(e), _) => (**e).clone(),
                        (None, _) => mcall(Expr::Paren(Box::new(r.clone())), "len", vec![]),
                    };
                    let e: Expr = mcall(Expr::Paren(Box::new(r.clone())), "__drain", vec![lo, hi]);
                    return self.expr(&e, None).map(Some);
                }
            }
        }
        // `v[..n].sort()`: 範囲の添字は自動で借用する
        if let (Some((f, m)), Expr::Index(_, ix_index, _)) = (slice_fn, strip_paren(&*mc_recv)) {
            if matches!(&**ix_index, Expr::Range(..)) {
                let r = &*mc_recv;
                let e: Expr = if m { Expr::Ref(true, Box::new(r.clone())) } else { Expr::Ref(false, Box::new(r.clone())) };
                let (v, t) = self.expr(&e, None)?;
                let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
                let id = self.prog.index[f];
                let args: Vec<&Expr> = mc_args.iter().collect();
                return self.call_fn(id, Some((v, t)), &args, None, None, SP).map(Some);
            }
        }
        let Some(mut rt) = self.peek(&*mc_recv) else {
            return Ok(None);
        };
        // (値はサイズ不定の型にならない: `[T]` / `str` と記録されていても実際は参照 (`tok(k).to_vec()`))
        if matches!(rt, Ty::Slice(_) | Ty::Str) {
            rt = Ty::Ref(false, intern(rt));
        }
        while let Ty::Ref(_, i) = rt {
            if rt.is_fat() {
                break;
            }
            rt = *i;
        }
        // String で String 自身にないメソッド (split など) は &str のメソッド
        if let Ty::Struct(sid) = rt {
            let sd = &self.prog.structs[sid as usize];
            if sd.base.is_none()
                && sd.name == "String"
                && self.prog.method(sid, &name).is_none()
                && (matches!(
                    name.as_str(),
                    "split" | "split_whitespace" | "lines" | "trim" | "starts_with" | "ends_with" | "contains"
                        | "find" | "chars"
                ) || self.prog.index.contains_key(&format!("__exr_str_{name}")))
            {
                let r = &*mc_recv;
                let e: Expr = Expr::MethodCall(
                    Box::new(mcall(Expr::Paren(Box::new(r.clone())), "as_str", vec![])),
                    mc_name.clone(),
                    mc_tf.clone(),
                    mc_args.clone(), crate::parse::Span::default()
                );
                // (型推論が元の呼び出しに記録した型を、作り直したノードにも写す: `t.parse()` の T)
                let mut keys: Vec<usize> = vec![];
                if let Some(&t) = self.lets.get(&(mc as *const Expr as usize)) {
                    self.lets.insert(&e as *const Expr as usize, t);
                    keys.push(&e as *const Expr as usize);
                    // (自前の構文木ではメソッド呼び出しのノードは式そのもの)
                }
                // (受け手 `(r).as_str()` は &str、`(r)` は元の受け手の型: 作り直したノードには型推論の記録がない)
                if let Expr::MethodCall(nm_recv, _, _, _, _) = &e {
                    let a = &**nm_recv as *const Expr as usize;
                    self.lets.insert(a, Ty::Ref(false, intern(Ty::Str)));
                    keys.push(a);
                    if let Expr::MethodCall(inner_recv, _, _, _, _) = &**nm_recv {
                        let b = &**inner_recv as *const Expr as usize;
                        self.lets.insert(b, rt);
                        keys.push(b);
                        if let Expr::Paren(pp) = &**inner_recv {
                            let c = &**pp as *const Expr as usize;
                            self.lets.insert(c, rt);
                            keys.push(c);
                        }
                    }
                }
                let r = self.expr(&e, None).map(Some);
                // (e はここで捨てるので、そのアドレスの記録も消す)
                for k in keys {
                    self.lets.remove(&k);
                }
                return r;
            }
        }
        // (Vec も、Vec 自身に同名のメソッドがなければスライスとして: `v.copy_from_slice(..)`)
        let vec_slice = matches!(rt, Ty::Struct(sid) if self.vec_fields(sid).is_some() && self.prog.method(sid, &name).is_none());
        if let (Some((f, m)), true) = (slice_fn, vec_slice || matches!(rt, Ty::Array(..) | Ty::Ref(_, Ty::Slice(_)))) {
            if let Some(&id) = self.prog.index.get(f) {
                // 受け手を `&[T]` / `&mut [T]` にして、引数 (クロージャも) は元の式のまま渡す
                let (base, len, elem) = self.indexable(&*mc_recv)?;
                let (fat, st) = (self.make_fat_v(base, len), Ty::Ref(m, intern(Ty::Slice(intern(elem)))));
                let args: Vec<&Expr> = mc_args.iter().collect();
                return self.call_fn(id, Some((fat, st)), &args, None, None, SP).map(Some);
            }
        }
        let elem = match rt {
            Ty::Array(t, _) => *t,
            Ty::Ref(_, Ty::Slice(t)) => **t,
            // &str は [u8] と同じ表現
            Ty::Ref(_, Ty::Str) => return self.str_method(mc),
            Ty::Ptr(_, t) => return self.ptr_method(mc, rt, *t),
            t if t.is_float() => return self.float_method(mc, t),
            t if t.is_int() || t == Ty::Bool || t == Ty::Char => {
                let r = self.int_method(mc, t)?;
                // char の `is_alphabetic` / `to_digit` など: prelude の `__exr_char_<m>`
                if r.is_none() && t == Ty::Char && self.prog.index.contains_key(&format!("__exr_char_{name}")) {
                    let (v, vt) = self.expr(&*mc_recv, None)?;
                    let Some(mut v) = v else { return Ok(Some((None, Ty::Never))) };
                    let mut vt = vt;
                    while let Ty::Ref(_, i) = vt {
                        v = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), v, 0);
                        vt = *i;
                    }
                    return self.helper_method("__exr_char_", &name, mc, (v, Ty::Char));
                }
                return Ok(r);
            }
            _ => return Ok(None),
        };
        let args: Vec<&Expr> = mc_args.iter().collect();
        let opt = |this: &Self, t: Ty| -> R<Ty> {
            let ob = *this
                .prog
                .struct_index
                .get("Option")
                .ok_or("Option is not available")?;
            Ok(Ty::Struct(this.prog.instantiate(ob, vec![t])?))
        };
        match name.as_str() {
            // 値としての `xs.iter()` / `xs.iter_mut()`: prelude の Iter / IterMut
            "iter" | "iter_mut" if self.prog.index.contains_key("__exr_slice_iter") || name == "iter_mut" => {
                let (base, len, elem) = self.indexable(&*mc_recv)?;
                let m = name == "iter_mut";
                let fat = self.make_fat_v(base, len);
                let f = if m { "__exr_slice_iter_mut" } else { "__exr_slice_iter" };
                self.call_named(f, vec![elem], &[(fat, Ty::Ref(m, intern(Ty::Slice(intern(elem)))))], SP)
                    .map(Some)
            }
            "len" | "is_empty" => {
                let (_, len, _) = self.indexable(&*mc_recv)?;
                if name == "len" {
                    Ok(Some((Some(len), Ty::Usize)))
                } else {
                    let c = self.b.ins().icmp_imm_u(IntCC::Equal, len, 0);
                    Ok(Some((Some(c), Ty::Bool)))
                }
            }
            // `s.try_into()`: 長さが N なら Ok([T; N]) (要素をコピー)
            "try_into" if args.is_empty() => {
                let rt = self.lets.get(&(mc as *const Expr as usize)).copied();
                let Some(Ty::Struct(rs)) = rt else {
                    return err(SP, "cannot infer the target type of `try_into`");
                };
                let Some(&dt) = self.prog.structs[rs as usize].args.first() else {
                    return err(SP, "cannot infer the target type of `try_into`");
                };
                let Ty::Array(_, n) = dt else {
                    return err(SP, "`try_into` on a slice is only supported into arrays");
                };
                let (base, len, _) = self.indexable(&*mc_recv)?;
                let out = self.temp(Ty::Struct(rs));
                let vs = &self.prog.structs[rs as usize].variants;
                let okv = vs.iter().position(|v| v.name == "Ok").unwrap();
                let errv = vs.iter().position(|v| v.name == "Err").unwrap();
                let off = vs[okv].fields[0].2;
                let c = self.b.ins().icmp_imm_u(IntCC::Equal, len, n as i64);
                let yes = self.b.create_block();
                let no = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().brif(c, yes, &[], no, &[]);
                self.b.switch_to_block(yes);
                self.store_tag(out, rs, okv);
                let dst = self.addr_plus(out, off as i32);
                self.copy(dst, base, dt);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(no);
                self.store_tag(out, rs, errv);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                Ok(Some((Some(out), Ty::Struct(rs))))
            }
            // `s.get(a..b)` / `get_mut`: 範囲が中なら Some(&s[a..b])
            "get" | "get_mut" if matches!(args.first(), Some(Expr::Range(..))) => {
                let Some(rg) = args.first().copied() else { unreachable!() };
                let (base, len, elem) = self.indexable(&*mc_recv)?;
                let (lo, hi) = self.range_bounds(rg, len)?;
                let m = name == "get_mut";
                let st = Ty::Ref(m, intern(Ty::Slice(intern(elem))));
                let ot = opt(self, st)?;
                let Ty::Struct(oi) = ot else { unreachable!() };
                let out = self.temp(ot);
                let vpos = |this: &Self, n: &str| this.prog.structs[oi as usize].variants.iter().position(|v| v.name == n).unwrap();
                let (some_vi, none_vi) = (vpos(self, "Some"), vpos(self, "None"));
                let c1 = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, lo, hi);
                let c2 = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, hi, len);
                let c = self.b.ins().band(c1, c2);
                let inb = self.b.create_block();
                let outb = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().brif(c, inb, &[], outb, &[]);
                self.b.switch_to_block(inb);
                self.store_tag(out, oi, some_vi);
                let p = self.elem_addr(base, lo, elem);
                let n = self.b.ins().isub(hi, lo);
                let fat = self.make_fat_v(p, n);
                let off = self.prog.structs[oi as usize].variants[some_vi].fields[0].2;
                let dst = self.addr_plus(out, off as i32);
                self.copy(dst, fat, st);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(outb);
                self.store_tag(out, oi, none_vi);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                Ok(Some((Some(out), ot)))
            }
            "get" | "get_mut" | "first" | "last" => {
                let (base, len, elem) = self.indexable(&*mc_recv)?;
                let i = match name.as_str() {
                    "first" => self.b.ins().iconst(types::I64, 0),
                    "last" => self.b.ins().iadd_imm_s(len, -1),
                    _ => {
                        let Some(a) = args.first() else {
                            return err(SP, "get needs an index");
                        };
                        let (v, t) = self.expr(a, Some(Ty::Usize))?;
                        self.same(SP, t, Ty::Usize)?;
                        v.unwrap()
                    }
                };
                let rty = Ty::Ref(name == "get_mut", intern(elem));
                let ot = opt(self, rty)?;
                let Ty::Struct(oi) = ot else { unreachable!() };
                let out = self.temp(ot);
                let vpos = |this: &Self, n: &str| {
                    this.prog.structs[oi as usize]
                        .variants
                        .iter()
                        .position(|v| v.name == n)
                        .unwrap()
                };
                let (some_vi, none_vi) = (vpos(self, "Some"), vpos(self, "None"));
                let inb = self.b.create_block();
                let outb = self.b.create_block();
                let done = self.b.create_block();
                // len - 1 が負 (空) でも符号なし比較で範囲外になる
                let c = self.b.ins().icmp(IntCC::UnsignedLessThan, i, len);
                self.b.ins().brif(c, inb, &[], outb, &[]);
                self.b.switch_to_block(inb);
                self.store_tag(out, oi, some_vi);
                let addr = self.elem_addr(base, i, elem);
                let off = self.prog.structs[oi as usize].variants[some_vi].fields[0].2;
                self.b.ins().store(mf(), addr, out, off as i32);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(outb);
                self.store_tag(out, oi, none_vi);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                Ok(Some((Some(out), ot)))
            }
            "swap" => {
                let (base, len, elem) = self.indexable(&*mc_recv)?;
                let [a1, a2] = args[..] else {
                    return err(SP, "swap needs two indices");
                };
                let (i, _) = self.expr(a1, Some(Ty::Usize))?;
                let (j, _) = self.expr(a2, Some(Ty::Usize))?;
                let (i, j) = (i.unwrap(), j.unwrap());
                let c1 = self.b.ins().icmp(IntCC::UnsignedLessThan, i, len);
                self.panic_unless(c1);
                let c2 = self.b.ins().icmp(IntCC::UnsignedLessThan, j, len);
                self.panic_unless(c2);
                let pi = self.elem_addr(base, i, elem);
                let pj = self.elem_addr(base, j, elem);
                let tmp = self.temp(elem);
                let (vi, _) = self.read(Place::Mem(pi, 0, elem));
                let (vj, _) = self.read(Place::Mem(pj, 0, elem));
                self.write(Place::Mem(tmp, 0, elem), vi);
                let (vt, _) = self.read(Place::Mem(tmp, 0, elem));
                self.write(Place::Mem(pi, 0, elem), vj);
                self.write(Place::Mem(pj, 0, elem), vt);
                Ok(Some((None, Ty::Unit)))
            }
            "contains" => {
                if !(elem.is_int() || matches!(elem, Ty::Bool | Ty::Char)) {
                    if let Some(&id) = self.prog.index.get("__exr_slice_contains") {
                        let (base, len, elem) = self.indexable(&*mc_recv)?;
                        let fat = self.make_fat_v(base, len);
                        let st = Ty::Ref(false, intern(Ty::Slice(intern(elem))));
                        return self.call_fn(id, Some((fat, st)), &args, None, None, SP).map(Some);
                    }
                    return err(
                        SP,
                        "contains is only supported for integer / bool / char elements",
                    );
                }
                let (base, len, elem) = self.indexable(&*mc_recv)?;
                let Some(a) = args.first() else {
                    return err(SP, "contains needs an argument");
                };
                let (x, xt) = self.expr(a, Some(Ty::Ref(false, intern(elem))))?;
                self.same(SP, xt, Ty::Ref(false, intern(elem)))?;
                let xv = self
                    .b
                    .ins()
                    .load(clif_ty(elem).unwrap(), mf(), x.unwrap(), 0);
                let i = self.b.declare_var(types::I64);
                let z = self.b.ins().iconst(types::I64, 0);
                self.b.def_var(i, z);
                let head = self.b.create_block();
                let body = self.b.create_block();
                let done = self.b.create_block();
                let res = self.b.append_block_param(done, types::I8);
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(head);
                let iv = self.b.use_var(i);
                let c = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, len);
                let f0 = self.b.ins().iconst(types::I8, 0);
                self.b.ins().brif(c, body, &[], done, &[f0]);
                self.b.switch_to_block(body);
                let addr = self.elem_addr(base, iv, elem);
                let ev = self.b.ins().load(clif_ty(elem).unwrap(), mf(), addr, 0);
                let eq = self.b.ins().icmp(IntCC::Equal, ev, xv);
                let nx = self.b.ins().iadd_imm_s(iv, 1);
                self.b.def_var(i, nx);
                let t1 = self.b.ins().iconst(types::I8, 1);
                self.b.ins().brif(eq, done, &[t1], head, &[]);
                self.b.switch_to_block(done);
                Ok(Some((Some(res), Ty::Bool)))
            }
            "as_ptr" | "as_mut_ptr" => {
                let (base, _, elem) = self.indexable(&*mc_recv)?;
                Ok(Some((
                    Some(base),
                    Ty::Ptr(name == "as_mut_ptr", intern(elem)),
                )))
            }
            "iter" | "iter_mut" | "into_iter" | "enumerate" | "rev" | "copied" | "cloned" => err(
                SP,
                format!("`.{name}()` is only supported directly in a `for` loop"),
            ),
            _ => Ok(None),
        }
    }

    /// &str: len / is_empty / as_bytes / as_ptr (表現は &[u8] と同じ)
    pub(crate) fn str_method(&mut self, mc: &Expr) -> R<Option<V>> {
        let Expr::MethodCall(mc_recv, mc_name, mc_tf, mc_args, _) = mc else { unreachable!() };
        let mc_recv: &Expr = mc_recv;
        let _ = (mc_name, mc_tf, mc_args);
        let (v, mut t) = self.expr(&*mc_recv, None)?;
        let Some(mut fat) = v else {
            return Ok(Some((None, Ty::Never)));
        };
        // `&&&str` など: fat な参照 (の置き場所のアドレス) まで辿る
        while let Ty::Ref(_, i) = t {
            if t.is_fat() {
                break;
            }
            if !i.is_fat() {
                fat = self.b.ins().load(PTR, mf(), fat, 0);
            }
            t = *i;
        }
        let u8s = Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8))));
        Ok(Some(match mc_name.to_string().as_str() {
            "len" => (Some(self.b.ins().load(types::I64, mf(), fat, 8)), Ty::Usize),
            "is_empty" => {
                let l = self.b.ins().load(types::I64, mf(), fat, 8);
                (Some(self.b.ins().icmp_imm_u(IntCC::Equal, l, 0)), Ty::Bool)
            }
            "as_bytes" => (Some(fat), u8s),
            "as_ptr" => (
                Some(self.b.ins().load(PTR, mf(), fat, 0)),
                Ty::Ptr(false, intern(Ty::U8)),
            ),
            // split / lines / trim / starts_with / ends_with / contains / find: prelude の関数
            "split" | "split_whitespace" | "lines" | "trim" | "starts_with" | "ends_with" | "contains" | "find"
                if !(mc_name.to_string() == "split" && mc_args.first().is_some_and(|a| self.peek(a) != Some(Ty::Char)))
                    && !matches!(mc_args.first(), Some(Expr::Closure(..)))
                    && self.prog.index.contains_key(match mc_name.to_string().as_str() {
                    "split" => "__exr_str_split",
                    "split_whitespace" => "__exr_str_split_ws",
                    "lines" => "__exr_str_lines",
                    "trim" => "__exr_str_trim",
                    "starts_with" => "__exr_str_starts_with",
                    "ends_with" => "__exr_str_ends_with",
                    _ => "__exr_str_find",
                }) =>
            {
                let sref = Ty::Ref(false, intern(Ty::Str));
                let name = mc_name.to_string();
                let f = match name.as_str() {
                    "split" => "__exr_str_split",
                    "split_whitespace" => "__exr_str_split_ws",
                    "lines" => "__exr_str_lines",
                    "trim" => "__exr_str_trim",
                    "starts_with" => "__exr_str_starts_with",
                    "ends_with" => "__exr_str_ends_with",
                    _ => "__exr_str_find",
                };
                let mut args = vec![(fat, sref)];
                for a in mc_args {
                    // char のパターン (`s.contains('x')`) は UTF-8 にして &str で渡す
                    let char_pat = name != "split" && self.peek(a) == Some(Ty::Char);
                    let exp = if name == "split" || char_pat { Ty::Char } else { sref };
                    let (v, t) = self.expr(a, Some(exp))?;
                    if name == "split" && t != Ty::Char {
                        return err(SP, "only `char` separators are supported in `split`");
                    }
                    // (`&&str` / `&String` のパターンは &str に)
                    let (v, t) = match t {
                        Ty::Ref(_, Ty::Ref(_, Ty::Str)) => (v, sref),
                        Ty::Ref(_, Ty::Struct(sid)) if exp == sref => match v.and_then(|x| self.deref_to_slice(x, *sid, false)) {
                            Some((fat, ft)) => (Some(fat), ft),
                            None => (v, t),
                        },
                        _ => (v, t),
                    };
                    self.same(SP, t, exp)?;
                    if char_pat {
                        let ss = self.b.create_stack_slot(4, 1 << 0);
                        let buf = self.b.ins().stack_addr(PTR, ss, 0);
                        let bt = Ty::Ptr(true, intern(Ty::U8));
                        let (n, _) = self.call_named(
                            "__exr_char_utf8",
                            vec![Ty::Unit],
                            &[(v.unwrap(), Ty::Char), (buf, bt)],
                            SP,
                        )?;
                        let fat = self.make_fat_v(buf, n.unwrap());
                        args.push((fat, sref));
                        continue;
                    }
                    args.push((v.unwrap(), t));
                }
                let r = self.call_named(f, vec![Ty::Unit], &args, SP)?;
                // contains は find の結果が Some か
                if name == "contains" {
                    let (v, t) = r;
                    let v = v.unwrap();
                    let tag = self.b.ins().load(types::I8, mf(), v, 0);
                    let _ = t;
                    let some = self.b.ins().icmp_imm_u(IntCC::NotEqual, tag, 0);
                    return Ok(Some((Some(some), Ty::Bool)));
                }
                return Ok(Some(r));
            }
            // `s.chars()` / `s.bytes()`
            "chars" => {
                return self
                    .call_named("__exr_str_chars", vec![Ty::Unit], &[(fat, Ty::Ref(false, intern(Ty::Str)))], SP)
                    .map(Some);
            }
            // `"..".to_string()` / `to_owned()` は prelude の String::from
            "to_string" | "to_owned" => {
                return self
                    .call_named("String::from", vec![], &[(fat, Ty::Ref(false, intern(Ty::Str)))], SP)
                    .map(Some);
            }
            m => {
                // prelude の `__exr_str_<m>` (`s.split("ab")` は __exr_str_split_str)
                // (パターンがクロージャなら `__exr_str_<m>_fn`)
                let fname = format!("{m}_fn");
                let h = if m == "split" && !matches!(mc_args.first(), Some(Expr::Closure(..))) {
                    "split_str"
                } else if matches!(mc_args.first(), Some(Expr::Closure(..)))
                    && self.prog.index.contains_key(&format!("__exr_str_{fname}"))
                {
                    fname.as_str()
                } else {
                    m
                };
                if let Some(r) = self.helper_method("__exr_str_", h, mc, (fat, Ty::Ref(false, intern(Ty::Str))))? {
                    return Ok(Some(r));
                }
                return err(SP, format!("no method `{m}` on `&str`"));
            }
        }))
    }

    /// 生ポインタ: add / sub / offset / is_null / read / write / read_volatile / write_volatile / cast
    pub(crate) fn ptr_method(&mut self, mc: &Expr, pt: Ty, elem: Ty) -> R<Option<V>> {
        let Expr::MethodCall(mc_recv, mc_name, mc_tf, mc_args, _) = mc else { unreachable!() };
        let mc_recv: &Expr = mc_recv;
        let _ = (mc_name, mc_tf, mc_args);
        let (p, _) = self.expr(&*mc_recv, Some(pt))?;
        let p = p.unwrap();
        let args: Vec<&Expr> = mc_args.iter().collect();
        let name = mc_name.to_string();
        Ok(Some(match name.as_str() {
            "add" | "sub" | "offset" | "wrapping_add" | "wrapping_sub" | "wrapping_offset" => {
                let it = if name.contains("offset") {
                    Ty::Isize
                } else {
                    Ty::Usize
                };
                let (n, _) = self.expr(args[0], Some(it))?;
                let mut n = n.unwrap();
                if name.ends_with("sub") {
                    n = self.b.ins().ineg(n);
                }
                (Some(self.elem_addr(p, n, elem)), pt)
            }
            "is_null" => (Some(self.b.ins().icmp_imm_u(IntCC::Equal, p, 0)), Ty::Bool),
            "read" | "read_volatile" | "read_unaligned" => {
                if elem.is_agg() {
                    // 指す先が後で書き換えられても値が変わらないようにコピーする
                    let a = self.temp(elem);
                    self.copy(a, p, elem);
                    (Some(a), elem)
                } else {
                    self.read(Place::Mem(p, 0, elem))
                }
            }
            "write" | "write_volatile" | "write_unaligned" => {
                let (v, vt) = self.expr(args[0], Some(elem))?;
                self.same(SP, vt, elem)?;
                self.write(Place::Mem(p, 0, elem), v);
                (None, Ty::Unit)
            }
            "cast" => {
                let to = match mc_tf.is_empty() {
                    false => match mc_tf.first() {
                        Some(GArg::Ty(t)) => self.parse_ty(t)?,
                        _ => return err(SP, "cast needs a type argument"),
                    },
                    true => return err(SP, "`cast` needs a turbofish: `p.cast::<T>()`"),
                };
                let Ty::Ptr(m, _) = pt else { unreachable!() };
                (Some(p), Ty::Ptr(m, intern(to)))
            }
            m => {
                return err(
                    SP,
                    format!("unsupported raw pointer method `{m}`"),
                );
            }
        }))
    }

    /// 整数の組み込みメソッド (wrapping_* / checked_* / saturating_* / min / max / abs / pow /
    /// count_ones / leading_zeros / trailing_zeros / is_power_of_two / rotate_* / swap_bytes / signum)
    pub(crate) fn int_method(&mut self, mc: &Expr, t: Ty) -> R<Option<V>> {
        let Expr::MethodCall(mc_recv, mc_name, mc_tf, mc_args, _) = mc else { unreachable!() };
        let mc_recv: &Expr = mc_recv;
        let _ = (mc_name, mc_tf, mc_args);
        let name = mc_name.to_string();
        let args: Vec<&Expr> = mc_args.iter().collect();
        let known = [
            "wrapping_add",
            "wrapping_sub",
            "wrapping_mul",
            "wrapping_neg",
            "checked_add",
            "checked_sub",
            "checked_mul",
            "checked_div",
            "checked_rem",
            "checked_pow",
            "wrapping_pow",
            "saturating_pow",
            "overflowing_pow",
            "overflowing_add",
            "overflowing_sub",
            "overflowing_mul",
            "rem_euclid",
            "div_euclid",
            "next_multiple_of",
            "next_power_of_two",
            "isqrt",
            "try_into",
            "checked_abs",
            "checked_neg",
            "leading_ones",
            "trailing_ones",
            "saturating_add",
            "saturating_sub",
            "min",
            "max",
            "abs",
            "pow",
            "count_ones",
            "count_zeros",
            "leading_zeros",
            "trailing_zeros",
            "is_power_of_two",
            "rotate_left",
            "rotate_right",
            "swap_bytes",
            "signum",
            "is_positive",
            "is_negative",
            "abs_diff",
            "is_ascii_digit",
            "is_ascii_alphabetic",
            "is_ascii_uppercase",
            "is_ascii_lowercase",
            "to_ascii_uppercase",
            "to_ascii_lowercase",
            "is_ascii_whitespace",
            "is_ascii_alphanumeric",
            "is_ascii_hexdigit",
            "is_ascii_punctuation",
            "is_ascii_graphic",
            "is_ascii_control",
            "is_ascii",
            "to_le_bytes",
            "to_be_bytes",
            "to_ne_bytes",
        ];
        if !known.contains(&name.as_str()) {
            return Ok(None);
        }
        // 受け手が `&u64` / `&mut u64` なら指す先の値 (自動 deref)
        let (a, a_ty) = if matches!(self.peek(&*mc_recv), Some(Ty::Ref(..))) {
            self.expr(&*mc_recv, None)?
        } else {
            self.expr(&*mc_recv, Some(t))?
        };
        let Some(mut a) = a else {
            return Ok(Some((None, Ty::Never)));
        };
        let mut at = a_ty;
        while let Ty::Ref(_, i) = at {
            a = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), a, 0);
            at = *i;
        }
        let ct = clif_ty(t).unwrap();
        let s = t.signed();
        let arg = |this: &mut Self, i: usize, want: Ty| -> R<Value> {
            let Some(e) = args.get(i) else {
                return err(SP, format!("`{name}` needs an argument"));
            };
            let (v, vt) = this.expr(e, Some(want))?;
            this.same(SP, vt, want)?;
            Ok(v.unwrap())
        };
        let ins_bool = |_this: &mut Self, c: Value| -> V { (Some(c), Ty::Bool) };
        let ascii = |this: &mut Self, lo: u8, hi: u8| -> Value {
            let x = this.b.ins().iadd_imm_s(a, -(lo as i64));
            this.b
                .ins()
                .icmp_imm_u(IntCC::UnsignedLessThanOrEqual, x, (hi - lo) as i64)
        };
        let r: V = match name.as_str() {
            "wrapping_add" => {
                let b = arg(self, 0, t)?;
                (Some(self.b.ins().iadd(a, b)), t)
            }
            "wrapping_sub" => {
                let b = arg(self, 0, t)?;
                (Some(self.b.ins().isub(a, b)), t)
            }
            "wrapping_mul" => {
                let b = arg(self, 0, t)?;
                (Some(self.b.ins().imul(a, b)), t)
            }
            "wrapping_neg" => (Some(self.b.ins().ineg(a)), t),
            "min" | "max" => {
                let b = arg(self, 0, t)?;
                let ins = self.b.ins();
                let v = match (name.as_str(), s) {
                    ("min", true) => ins.smin(a, b),
                    ("min", false) => ins.umin(a, b),
                    ("max", true) => ins.smax(a, b),
                    _ => ins.umax(a, b),
                };
                (Some(v), t)
            }
            "abs" => {
                // (debug と同じく MIN の abs は panic)
                if self.overflow_checks && s {
                    let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                    let bad = self.b.ins().icmp(IntCC::Equal, a, min);
                    self.ovf_panic(bad, 6, expr_start(mc));
                }
                (Some(self.b.ins().iabs(a)), t)
            }
            "checked_abs" | "checked_neg" => {
                let v = if name == "checked_abs" { self.b.ins().iabs(a) } else { self.b.ins().ineg(a) };
                let bad = if s {
                    let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                    self.b.ins().icmp(IntCC::Equal, a, min)
                } else if name == "checked_neg" {
                    // (符号なしは 0 だけ)
                    self.b.ins().icmp_imm_u(IntCC::NotEqual, a, 0)
                } else {
                    self.b.ins().iconst(types::I8, 0)
                };
                self.option_if(v, t, bad)?
            }
            "abs_diff" => {
                let b = arg(self, 0, t)?;
                let ins = self.b.ins();
                let (hi, lo) = if s {
                    (ins.smax(a, b), self.b.ins().smin(a, b))
                } else {
                    (ins.umax(a, b), self.b.ins().umin(a, b))
                };
                let ut = match t {
                    Ty::I8 => Ty::U8,
                    Ty::I16 => Ty::U16,
                    Ty::I32 => Ty::U32,
                    Ty::I64 => Ty::U64,
                    Ty::Isize => Ty::Usize,
                    t => t,
                };
                (Some(self.b.ins().isub(hi, lo)), ut)
            }
            "signum" => {
                let z = self.iconst_t(ct, 0);
                let gt = self.b.ins().icmp(IntCC::SignedGreaterThan, a, z);
                let lt = self.b.ins().icmp(IntCC::SignedLessThan, a, z);
                let g = self.b.ins().uextend(ct, gt);
                let l = self.b.ins().uextend(ct, lt);
                (Some(self.b.ins().isub(g, l)), t)
            }
            "is_positive" => {
                let c = self.b.ins().icmp_imm_s(IntCC::SignedGreaterThan, a, 0);
                ins_bool(self, c)
            }
            "is_negative" => {
                let c = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, a, 0);
                ins_bool(self, c)
            }
            "count_ones" => {
                let v = self.b.ins().popcnt(a);
                (Some(self.convert(v, t, Ty::U32)), Ty::U32)
            }
            "count_zeros" => {
                let n = self.b.ins().bnot(a);
                let v = self.b.ins().popcnt(n);
                (Some(self.convert(v, t, Ty::U32)), Ty::U32)
            }
            "leading_zeros" => {
                let v = self.b.ins().clz(a);
                (Some(self.convert(v, t, Ty::U32)), Ty::U32)
            }
            "trailing_zeros" => {
                let v = self.b.ins().ctz(a);
                (Some(self.convert(v, t, Ty::U32)), Ty::U32)
            }
            "is_power_of_two" => {
                let c = self.b.ins().popcnt(a);
                let c = self.b.ins().icmp_imm_u(IntCC::Equal, c, 1);
                ins_bool(self, c)
            }
            "rotate_left" | "rotate_right" => {
                let b = arg(self, 0, Ty::U32)?;
                let v = if name == "rotate_left" {
                    self.b.ins().rotl(a, b)
                } else {
                    self.b.ins().rotr(a, b)
                };
                (Some(v), t)
            }
            "swap_bytes" => (
                Some(if t.bits() == 8 {
                    a
                } else {
                    self.b.ins().bswap(a)
                }),
                t,
            ),
            "pow" | "wrapping_pow" | "checked_pow" | "saturating_pow" | "overflowing_pow" => {
                // 繰り返し二乗法 (あふれの印も持つ: 掛けた結果と、まだ使う二乗)
                let checked = ((name != "pow" && name != "wrapping_pow") || (name == "pow" && self.overflow_checks)) && t.bits() < 128;
                let e0 = arg(self, 0, Ty::U32)?;
                let ofv = self.b.declare_var(types::I8);
                let f0 = self.b.ins().iconst(types::I8, 0);
                self.b.def_var(ofv, f0);
                let r = self.b.declare_var(ct);
                let bv = self.b.declare_var(ct);
                let ev = self.b.declare_var(types::I32);
                let one = self.iconst_t(ct, 1);
                self.b.def_var(r, one);
                self.b.def_var(bv, a);
                self.b.def_var(ev, e0);
                let head = self.b.create_block();
                let body = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(head);
                let e = self.b.use_var(ev);
                self.b.ins().brif(e, body, &[], done, &[]);
                self.b.switch_to_block(body);
                let e = self.b.use_var(ev);
                let bit = self.b.ins().band_imm_u(e, 1);
                let rv = self.b.use_var(r);
                let bb = self.b.use_var(bv);
                let (rm, b2, e2) = if checked {
                    let (rm, o1) = if s { self.b.ins().smul_overflow(rv, bb) } else { self.b.ins().umul_overflow(rv, bb) };
                    let (b2, o2) = if s { self.b.ins().smul_overflow(bb, bb) } else { self.b.ins().umul_overflow(bb, bb) };
                    let e2 = self.b.ins().ushr_imm_u(e, 1);
                    // (最後の二乗は使わないので、あふれても数えない)
                    let more = self.b.ins().icmp_imm_u(IntCC::NotEqual, e2, 0);
                    let o2 = self.b.ins().band(o2, more);
                    let bitb = self.b.ins().icmp_imm_u(IntCC::NotEqual, bit, 0);
                    let o1 = self.b.ins().band(o1, bitb);
                    let o = self.b.ins().bor(o1, o2);
                    let old = self.b.use_var(ofv);
                    let o = self.b.ins().bor(old, o);
                    self.b.def_var(ofv, o);
                    (rm, b2, e2)
                } else {
                    let rm = self.b.ins().imul(rv, bb);
                    let b2 = self.b.ins().imul(bb, bb);
                    (rm, b2, self.b.ins().ushr_imm_u(e, 1))
                };
                let rn = self.b.ins().select(bit, rm, rv);
                self.b.def_var(r, rn);
                self.b.def_var(bv, b2);
                self.b.def_var(ev, e2);
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(done);
                let v = self.b.use_var(r);
                let of = self.b.use_var(ofv);
                match name.as_str() {
                    "checked_pow" => self.option_if(v, t, of)?,
                    "overflowing_pow" => self.pair_with_bool(v, t, of)?,
                    "pow" if checked => {
                        self.ovf_panic(of, 2, expr_start(mc));
                        (Some(v), t)
                    }
                    "saturating_pow" => {
                        // 負の底の奇数乗なら MIN、それ以外は MAX
                        let max = self.int_const(t, crate::program::prim_assoc_const(t, "MAX").unwrap());
                        let sat = if s {
                            let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                            let z = self.iconst_t(ct, 0);
                            let nb = self.b.ins().icmp(IntCC::SignedLessThan, a, z);
                            let odd = self.b.ins().band_imm_u(e0, 1);
                            let odd = self.b.ins().icmp_imm_u(IntCC::NotEqual, odd, 0);
                            let negr = self.b.ins().band(nb, odd);
                            self.b.ins().select(negr, min, max)
                        } else {
                            max
                        };
                        (Some(self.b.ins().select(of, sat, v)), t)
                    }
                    _ => (Some(v), t),
                }
            }
            "saturating_add" | "saturating_sub" => {
                let b = arg(self, 0, t)?;
                // (Cranelift の *_sat はベクトル用なので、あふれを見て端の値に差し替える)
                let add = name == "saturating_add";
                let (v, of) = match (add, s) {
                    (true, false) => self.b.ins().uadd_overflow(a, b),
                    (true, true) => self.b.ins().sadd_overflow(a, b),
                    (false, false) => self.b.ins().usub_overflow(a, b),
                    (false, true) => self.b.ins().ssub_overflow(a, b),
                };
                let max = self.int_const(t, crate::program::prim_assoc_const(t, "MAX").unwrap());
                let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                let sat = if s {
                    // 足す数が負なら下に、引く数が負なら上にあふれる
                    let z = self.iconst_t(ct, 0);
                    let bneg = self.b.ins().icmp(IntCC::SignedLessThan, b, z);
                    if add { self.b.ins().select(bneg, min, max) } else { self.b.ins().select(bneg, max, min) }
                } else if add {
                    max
                } else {
                    min
                };
                (Some(self.b.ins().select(of, sat, v)), t)
            }
            "checked_add" | "checked_sub" | "checked_mul" | "checked_div" | "checked_rem" => {
                let b = arg(self, 0, t)?;
                let (v, of) = match name.as_str() {
                    "checked_add" => self.arith_ovf(0, s, a, b, t),
                    "checked_sub" => self.arith_ovf(1, s, a, b, t),
                    "checked_mul" => self.arith_ovf(2, s, a, b, t),
                    _ => {
                        // 0 除算 (と符号付きの MIN / -1) は None。安全な除数に差し替えてから割る
                        let zero = self.b.ins().icmp_imm_u(IntCC::Equal, b, 0);
                        let bad = if s {
                            let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                            let am = self.b.ins().icmp(IntCC::Equal, a, min);
                            let bm = self.b.ins().icmp_imm_s(IntCC::Equal, b, -1);
                            let both = self.b.ins().band(am, bm);
                            self.b.ins().bor(zero, both)
                        } else {
                            zero
                        };
                        let one = self.iconst_t(ct, 1);
                        let safe = self.b.ins().select(bad, one, b);
                        if t.bits() == 128 {
                            let span = SP;
                            let q = self.div128(a, safe, t, name == "checked_rem", span)?;
                            (q, bad)
                        } else {
                        let ins = self.b.ins();
                        let q = match (name.as_str(), s) {
                            ("checked_div", true) => ins.sdiv(a, safe),
                            ("checked_div", false) => ins.udiv(a, safe),
                            ("checked_rem", true) => ins.srem(a, safe),
                            _ => ins.urem(a, safe),
                        };
                        (q, bad)
                        }
                    }
                };
                self.option_if(v, t, of)?
            }
            // (値, あふれたか) のタプル
            "overflowing_add" | "overflowing_sub" | "overflowing_mul" => {
                let b = arg(self, 0, t)?;
                let (v, of) = match name.as_str() {
                    "overflowing_add" => self.arith_ovf(0, s, a, b, t),
                    "overflowing_sub" => self.arith_ovf(1, s, a, b, t),
                    _ => self.arith_ovf(2, s, a, b, t),
                };
                self.pair_with_bool(v, t, of)?
            }
            // `a.next_multiple_of(b)` (符号なし): a % b == 0 なら a、でなければ a + (b - a % b)
            "next_multiple_of" if !s => {
                let b = arg(self, 0, t)?;
                let r = self.b.ins().urem(a, b);
                let d = self.b.ins().isub(b, r);
                let up = self.b.ins().iadd(a, d);
                let z = self.b.ins().icmp_imm_u(IntCC::Equal, r, 0);
                (Some(self.b.ins().select(z, a, up)), t)
            }
            "rem_euclid" | "div_euclid" => {
                let b = arg(self, 0, t)?;
                if !s {
                    let v = if name == "rem_euclid" { self.b.ins().urem(a, b) } else { self.b.ins().udiv(a, b) };
                    (Some(v), t)
                } else {
                    // r = a % b; r < 0 なら r + |b|、商は q - sign(b)
                    let rr = self.b.ins().srem(a, b);
                    let q = self.b.ins().sdiv(a, b);
                    let z = self.iconst_t(ct, 0);
                    let neg = self.b.ins().icmp(IntCC::SignedLessThan, rr, z);
                    if name == "rem_euclid" {
                        let ab = self.b.ins().iabs(b);
                        let fixed = self.b.ins().iadd(rr, ab);
                        (Some(self.b.ins().select(neg, fixed, rr)), t)
                    } else {
                        let bpos = self.b.ins().icmp(IntCC::SignedGreaterThan, b, z);
                        let one = self.iconst_t(ct, 1);
                        let qm = self.b.ins().isub(q, one);
                        let qp = self.b.ins().iadd(q, one);
                        let adj = self.b.ins().select(bpos, qm, qp);
                        (Some(self.b.ins().select(neg, adj, q)), t)
                    }
                }
            }
            "next_power_of_two" => {
                // x <= 1 なら 1、それ以外は 1 << (bits - clz(x - 1))
                let one = self.iconst_t(ct, 1);
                let xm = self.b.ins().isub(a, one);
                let lz = self.b.ins().clz(xm);
                let bits = self.iconst_t(ct, t.bits() as i64);
                let sh = self.b.ins().isub(bits, lz);
                let p = self.b.ins().ishl(one, sh);
                let small = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, a, one);
                (Some(self.b.ins().select(small, one, p)), t)
            }
            "isqrt" if t.bits() <= 64 => {
                let w = if ct == types::I64 { a } else { self.b.ins().uextend(types::I64, a) };
                let (v, _) = self.call_named("__exr_isqrt", vec![Ty::Unit], &[(w, Ty::U64)], SP)?;
                let v = v.unwrap();
                let v = if ct == types::I64 { v } else { self.b.ins().ireduce(ct, v) };
                (Some(v), t)
            }
            "try_into" => {
                // 型は型推論の記録 (Result<T, TryFromIntError>) から
                let rt = self.lets.get(&(mc as *const Expr as usize)).copied().or_else(|| {
                    // (pos_key の記録は自前の構文木にはないので peek で)
                    if self.in_fmt_args > 0 {
                        self.peek(mc)
                    } else {
                        None
                    }
                });
                let Some(Ty::Struct(rs)) = rt else {
                    return err(SP, "cannot infer the target type of `try_into`");
                };
                let Some(&dt) = self.prog.structs[rs as usize].args.first() else {
                    return err(SP, "cannot infer the target type of `try_into`");
                };
                if !dt.is_int() {
                    return err(SP, "`try_into` is only supported between integer types");
                }
                // i128 に広げて範囲を見る (u128 の上半分は符号なしで比べる)
                let w = if ct == types::I128 {
                    a
                } else if s {
                    self.b.ins().sextend(types::I128, a)
                } else {
                    self.b.ins().uextend(types::I128, a)
                };
                let z = self.int_const(Ty::I128, 0);
                let lo_ok = if s && !dt.signed() {
                    self.b.ins().icmp(IntCC::SignedGreaterThanOrEqual, w, z)
                } else if s {
                    let lo = self.int_const(Ty::I128, crate::program::prim_assoc_const(dt, "MIN").unwrap());
                    self.b.ins().icmp(IntCC::SignedGreaterThanOrEqual, w, lo)
                } else {
                    self.b.ins().iconst(types::I8, 1)
                };
                let hi_ok = if dt == Ty::U128 {
                    self.b.ins().iconst(types::I8, 1)
                } else {
                    let hi = self.int_const(Ty::I128, crate::program::prim_assoc_const(dt, "MAX").unwrap());
                    if t == Ty::U128 {
                        self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, w, hi)
                    } else {
                        self.b.ins().icmp(IntCC::SignedLessThanOrEqual, w, hi)
                    }
                };
                let ok = self.b.ins().band(lo_ok, hi_ok);
                let dct = clif_ty(dt).unwrap();
                let v = if dct == types::I128 { w } else { self.b.ins().ireduce(dct, w) };
                let out = self.temp(Ty::Struct(rs));
                let vs = &self.prog.structs[rs as usize].variants;
                let okv = vs.iter().position(|v| v.name == "Ok").unwrap();
                let errv = vs.iter().position(|v| v.name == "Err").unwrap();
                let off = vs[okv].fields[0].2;
                let yes = self.b.create_block();
                let no = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().brif(ok, yes, &[], no, &[]);
                self.b.switch_to_block(yes);
                self.store_tag(out, rs, okv);
                self.b.ins().store(mf(), v, out, off as i32);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(no);
                self.store_tag(out, rs, errv);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                (Some(out), Ty::Struct(rs))
            }
            "leading_ones" | "trailing_ones" => {
                let n = self.b.ins().bnot(a);
                let c = if name == "leading_ones" { self.b.ins().clz(n) } else { self.b.ins().ctz(n) };
                (Some(self.to_u32(c, ct)), Ty::U32)
            }
            "is_ascii_digit" => {
                let c = ascii(self, b'0', b'9');
                ins_bool(self, c)
            }
            "is_ascii_uppercase" => {
                let c = ascii(self, b'A', b'Z');
                ins_bool(self, c)
            }
            "is_ascii_lowercase" => {
                let c = ascii(self, b'a', b'z');
                ins_bool(self, c)
            }
            "is_ascii_alphabetic" => {
                let u = ascii(self, b'A', b'Z');
                let l = ascii(self, b'a', b'z');
                let c = self.b.ins().bor(u, l);
                ins_bool(self, c)
            }
            "is_ascii_alphanumeric" => {
                let u = ascii(self, b'A', b'Z');
                let l = ascii(self, b'a', b'z');
                let d = ascii(self, b'0', b'9');
                let c = self.b.ins().bor(u, l);
                let c = self.b.ins().bor(c, d);
                ins_bool(self, c)
            }
            "is_ascii_hexdigit" => {
                let d = ascii(self, b'0', b'9');
                let u = ascii(self, b'A', b'F');
                let l = ascii(self, b'a', b'f');
                let c = self.b.ins().bor(u, l);
                let c = self.b.ins().bor(c, d);
                ins_bool(self, c)
            }
            // (! から / 、: から @、[ から `、{ から ~)
            "is_ascii_punctuation" => {
                let mut acc = self.b.ins().iconst(types::I8, 0);
                for (lo, hi) in [(b'!', b'/'), (b':', b'@'), (b'[', b'`'), (b'{', b'~')] {
                    let c = ascii(self, lo, hi);
                    acc = self.b.ins().bor(acc, c);
                }
                ins_bool(self, acc)
            }
            "is_ascii_graphic" => {
                let c = ascii(self, b'!', b'~');
                ins_bool(self, c)
            }
            "is_ascii_control" => {
                let lo = self.b.ins().icmp_imm_u(IntCC::UnsignedLessThan, a, 0x20);
                let del = self.b.ins().icmp_imm_u(IntCC::Equal, a, 0x7f);
                let c = self.b.ins().bor(lo, del);
                ins_bool(self, c)
            }
            "is_ascii" => {
                let c = self.b.ins().icmp_imm_u(IntCC::UnsignedLessThan, a, 0x80);
                ins_bool(self, c)
            }
            // (x86-64 はリトルエンディアン: そのまま書くと le / ne。be はバイトを逆にしてから)
            "to_le_bytes" | "to_be_bytes" | "to_ne_bytes" => {
                let n = t.bits() / 8;
                let at = Ty::Array(intern(Ty::U8), n);
                let out = self.temp(at);
                let v = if name == "to_be_bytes" && n > 1 { self.b.ins().bswap(a) } else { a };
                let mut fl = MemFlagsData::new();
                fl.set_notrap();
                self.b.ins().store(fl, v, out, 0);
                (Some(out), at)
            }
            "is_ascii_whitespace" => {
                let mut acc = self.b.ins().iconst(types::I8, 0);
                for ch in [b' ', b'\t', b'\n', 0x0c, b'\r'] {
                    let c = self.b.ins().icmp_imm_u(IntCC::Equal, a, ch as i64);
                    acc = self.b.ins().bor(acc, c);
                }
                ins_bool(self, acc)
            }
            "to_ascii_uppercase" | "to_ascii_lowercase" => {
                let up = name == "to_ascii_uppercase";
                let c = if up {
                    ascii(self, b'a', b'z')
                } else {
                    ascii(self, b'A', b'Z')
                };
                let flipped = self.b.ins().bxor_imm_u(a, 0x20);
                (Some(self.b.ins().select(c, flipped, a)), t)
            }
            _ => unreachable!(),
        };
        Ok(Some(r))
    }
}
