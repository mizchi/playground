use super::*;
use super::clif::{TrapCode, Type, Value, types};
use super::lit::Lit;
use crate::parse::{Block as PBlock, Expr, GArg, Pat, Stmt};
use crate::program::PPath;
use crate::types::{Ty, intern};

// --- 書き換えで式を作るための小さな helper ------------------------------------

/// `(e)`
fn paren(e: Expr) -> Expr {
    Expr::Paren(Box::new(e))
}

/// `let name = init;`
fn let_stmt(name: &str, init: Expr) -> Stmt {
    Stmt::Let(Pat::Ident(false, false, name.to_string(), None), None, Some(init), None)
}

/// `..` (添字の全範囲)
fn full_range() -> Expr {
    Expr::Range(None, None, false)
}

/// `e as T` の T (名前だけの型)
fn ty_named(n: &str) -> Box<crate::parse::Ty> {
    Box::new(crate::parse::Ty::Path(None, path_of(n)))
}

/// 名前とリテラルなどだけからなる式のソースのテキスト (`vec![..]` のトークン列を作るのに使う)
/// 式の列を `, ` でつないだテキスト (どれかが書けなければ None)
fn simple_texts(xs: &[Expr]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for x in xs {
        parts.push(simple_text(x)?);
    }
    Some(parts.join(", "))
}

fn simple_text(e: &Expr) -> Option<String> {
    Some(match e {
        Expr::Lit(t, _) => t.clone(),
        Expr::Path(None, p) => crate::parse::path_text(p),
        Expr::Unary(op, x, _) => format!("{op}{}", simple_text(x)?),
        Expr::Ref(m, x) => format!("&{}{}", if *m { "mut " } else { "" }, simple_text(x)?),
        Expr::Paren(x) => format!("({})", simple_text(x)?),
        Expr::Field(b, n, _) => format!("{}.{n}", simple_text(b)?),
        Expr::Tuple(xs) if xs.len() == 1 => format!("({},)", simple_text(&xs[0])?),
        Expr::Tuple(xs) => format!("({})", simple_texts(xs)?),
        Expr::Array(xs) => format!("[{}]", simple_texts(xs)?),
        _ => return None,
    })
}

/// `vec![es]` (syn 版の `parse_quote!(vec![#es])`)。
/// 自前の構文木には式の表示がないので、名前・リテラルなどで書けない要素は先に let で束縛してその名前を並べる
pub(crate) fn vec_of(es: &[Expr]) -> Expr {
    let mut lets = vec![];
    let mut items = vec![];
    for (i, e) in es.iter().enumerate() {
        match simple_text(e) {
            Some(t) => items.push(t),
            None => {
                let n = format!("__exr_va{i}");
                lets.push(let_stmt(&n, e.clone()));
                items.push(n);
            }
        }
    }
    let m = Expr::Macro(path_of("vec"), format!("[{}]", items.join(", ")), crate::parse::Span::default());
    if lets.is_empty() {
        return m;
    }
    lets.push(Stmt::Expr(m, false));
    Expr::Block(None, false, PBlock { stmts: lets })
}

impl Cx<'_> {
    /// `recv.m(args)`: レシーバを自動 deref して struct を見つけ、
    /// メソッドの self 型に合わせて自動参照 (`&self` / `&mut self`) かコピー (`self`) で渡す
    pub(crate) fn method_call(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::MethodCall(recv, method, gargs, margs, msp) = e else { unreachable!() };
        let msp = *msp;
        // (`v[1..].iter()`: 範囲の添字の受け手は参照にする (自動参照)。文字列は str のメソッドが受ける)
        if let Expr::Index(b, ix, _) = strip_paren(recv) {
            let bt = self.peek(b).map(|t| self.strip_refs(t));
            let is_text = bt.is_some_and(|t| t == Ty::Str || matches!(t, Ty::Struct(sid) if self.prog.structs[sid as usize].name == "String"));
            if matches!(&**ix, Expr::Range(..)) && !is_text && method != "try_into" {
                let ne = Expr::MethodCall(Box::new(Expr::Ref(false, Box::new(strip_paren(recv).clone()))), method.clone(), gargs.clone(), margs.clone(), crate::parse::Span::default());
                let ne = self.keep_expr(ne);
                return self.method_call(ne, expected);
            }
        }
        let recv: &Expr = recv;
        let method = method.as_str();
        if matches!(method, "unwrap" | "expect") {
            if let (Some(t), Expr::MethodCall(_, m2, _, a2, _)) = (expected, strip_paren(recv)) {
                if m2 == "try_into" && a2.is_empty() {
                    self.try_into_hint = Some(t);
                }
            }
        }
        // `s.get_unchecked(i)` / `get_unchecked_mut(a..)` (配列・スライス・Vec): `&s[i]` / `&mut s[a..]` (範囲は検査する)
        if matches!(method, "get_unchecked" | "get_unchecked_mut") && margs.len() == 1 {
            let seq = match self.peek(recv).map(|t| self.strip_refs(t)) {
                Some(Ty::Array(..) | Ty::Slice(_)) => true,
                Some(Ty::Struct(sid)) => self.vec_fields(sid).is_some(),
                _ => false,
            };
            if seq {
                let ix = Expr::Index(Box::new(paren(recv.clone())), Box::new(margs[0].clone()), Default::default());
                let ne = Expr::Ref(method == "get_unchecked_mut", Box::new(ix));
                let ne = self.keep_expr(ne);
                return self.expr(ne, expected);
            }
        }
        // `x.as_ref()` (AsRef: 文字列・Vec・配列・スライス): 先を期待型か、関数の境界 `T: AsRef<X>` で決める
        if method == "as_ref" && margs.is_empty() {
            if let Some(ne) = self.as_ref_rewrite(recv, expected) {
                let ne = self.keep_expr(ne);
                return self.expr(ne, expected);
            }
        }
        let tf: Option<&[GArg]> = (!gargs.is_empty()).then_some(&gargs[..]);
        // `x.unwrap()` / `x.expect(..)`: panic の場所 (メソッド名の位置) を先に書いておく (#[track_caller] 相当)
        if matches!(method, "unwrap" | "expect" | "unwrap_err" | "expect_err") && self.prog.panic_handler.is_none() {
            if let Some(&ci) = self.prog.const_index.get("__EXR_CALLER") {
                let (l, c) = self.loc(msp.at);
                let v = self.b.ins().iconst(types::I64, ((l as i64) << 16) | c as i64);
                let a = self.static_sym(ci);
                self.b.ins().store(mf(), v, a, 0);
            }
        }
        // 受け手が `a.as_slice()` (a: 配列) なら `(&a[..])` にして呼び直す (`a.as_slice().len()`)
        if let Expr::MethodCall(ir, iname, _, iargs, _) = strip_paren(recv) {
            if iargs.is_empty() && matches!(iname.as_str(), "as_slice" | "as_mut_slice") {
                if let Some(Ty::Array(..)) = self.peek(ir).map(|t| self.strip_refs(t)) {
                    let r = (**ir).clone();
                    let ix = Expr::Index(Box::new(paren(r)), Box::new(full_range()), crate::parse::Span::default());
                    let nr = paren(Expr::Ref(iname != "as_slice", Box::new(ix)));
                    let m2 = Expr::MethodCall(Box::new(nr), method.to_string(), gargs.clone(), margs.clone(), crate::parse::Span::default());
                    return self.method_call(&m2, expected);
                }
            }
        }
        // `[T; N]::as_slice()` / `as_mut_slice()`: `&a[..]` / `&mut a[..]`
        if margs.is_empty() && matches!(method, "as_slice" | "as_mut_slice") {
            if let Some(Ty::Array(..)) = self.peek(recv).map(|t| self.strip_refs(t)) {
                let ix = Expr::Index(Box::new(paren(recv.clone())), Box::new(full_range()), crate::parse::Span::default());
                let e = Expr::Ref(method != "as_slice", Box::new(ix));
                return self.expr(&e, expected);
            }
        }
        // `x.hash(state)` (x: タプル / Option で Hash の impl がない): std と同じく要素を順に (Option は判別子 (isize) と中身)
        if method == "hash" && margs.len() == 1 {
            if let Some(Ty::Struct(sid)) = self.peek(recv).map(|t| self.strip_refs(t)) {
                let st = &margs[0];
                // (参照ならそのまま使う: ローカル変数の参照を新たに取らない)
                let rv: Expr = if matches!(self.peek(recv), Some(Ty::Ref(..))) {
                    recv.clone()
                } else {
                    Expr::Ref(false, Box::new(paren(recv.clone())))
                };
                if self.prog.method(sid, "hash").is_none() {
                    if self.prog.is_tuple(sid) {
                        let n = self.prog.structs[sid as usize].fields.len();
                        let mut stmts = vec![let_stmt("__hs", st.clone()), let_stmt("__hv", rv)];
                        for i in 0..n {
                            let f = paren(Expr::Field(Box::new(path_expr("__hv")), i.to_string(), crate::parse::Span::default()));
                            let hs = Expr::Ref(true, Box::new(Expr::Unary("*".into(), Box::new(path_expr("__hs")), crate::parse::Span::default())));
                            stmts.push(Stmt::Expr(mcall(f, "hash", vec![hs]), true));
                        }
                        let e = Expr::Block(None, false, PBlock { stmts });
                        return self.expr(&e, expected);
                    }
                    if self.prog.structs[self.prog.base_of(sid) as usize].name == "Option" {
                        let mut e = crate::parse::parse_expr(
                            "{
                                let __hs = 0;
                                match 0 {
                                    None => (0isize).hash(&mut *__hs),
                                    Some(__hx) => {
                                        (1isize).hash(&mut *__hs);
                                        (*__hx).hash(&mut *__hs)
                                    }
                                }
                            }",
                        )?;
                        // (テンプレートの `0` の所に、状態と受け手を入れる)
                        if let Expr::Block(_, _, b) = &mut e {
                            if let Some(Stmt::Let(_, _, init, _)) = b.stmts.get_mut(0) {
                                *init = Some(st.clone());
                            }
                            if let Some(Stmt::Expr(Expr::Match(s, _), _)) = b.stmts.get_mut(1) {
                                **s = rv;
                            }
                        }
                        return self.expr(&e, expected);
                    }
                }
            }
        }
        // `o.flatten()` (Option<Option<T>>): `o.unwrap_or(None)`
        if method == "flatten" && margs.is_empty() {
            if let Some(Ty::Struct(sid)) = self.peek(recv) {
                let opt = |this: &Self, s: u32| this.prog.structs[this.prog.base_of(s) as usize].name == "Option";
                if opt(self, sid) && matches!(self.prog.structs[sid as usize].args.first(), Some(Ty::Struct(i)) if opt(self, *i)) {
                    let e = mcall(paren(recv.clone()), "unwrap_or", vec![path_expr("None")]);
                    return self.expr(&e, expected);
                }
            }
        }
        // `it.flatten()` (要素が &Vec<T> / &[T] / &[T; N]): `it.flat_map(|x| x.iter())`
        if method == "flatten" && margs.is_empty() {
            let item = self.peek(recv).filter(|&t| !self.prog.has_param(t)).and_then(|t| self.prog.assoc_of(t, "Item"));
            let by_ref = match item {
                Some(Ty::Ref(_, Ty::Slice(_) | Ty::Array(..))) => true,
                Some(Ty::Ref(_, Ty::Struct(sid))) => self.vec_fields(*sid).is_some(),
                _ => false,
            };
            if let (true, Some(Ty::Ref(_, inner))) = (by_ref, item) {
                let el = match inner {
                    Ty::Slice(e) | Ty::Array(e, _) => **e,
                    Ty::Struct(sid) => self.prog.structs[*sid as usize].args[0],
                    _ => unreachable!(),
                };
                let (v, t) = self.expr(recv, None)?;
                let Some(v) = v else { return Ok((None, Ty::Never)) };
                return self.call_named("__exr_flatten_ref", vec![t, el], &[(v, t)], SP);
            }
        }
        // `v.get(a..b)` (Vec): スライスにしてから (`(&v[..]).get(a..b)`)
        if (method == "get" || method == "get_mut")
            && matches!(margs.first(), Some(Expr::Range(..)))
            && matches!(self.peek(recv).map(|t| self.strip_refs(t)), Some(Ty::Struct(sid)) if self.vec_fields(sid).is_some())
        {
            let r: Expr = if matches!(self.peek(recv), Some(Ty::Ref(..))) {
                paren(Expr::Unary("*".into(), Box::new(recv.clone()), crate::parse::Span::default()))
            } else {
                recv.clone()
            };
            let ix = Expr::Index(Box::new(paren(r)), Box::new(full_range()), crate::parse::Span::default());
            let ne = Expr::MethodCall(Box::new(ix), method.to_string(), vec![], margs.clone(), crate::parse::Span::default());
            let t0 = self.lets.get(&(e as *const Expr as usize)).copied();
            if let Some(t) = t0 {
                self.lets.insert(&ne as *const Expr as usize, t);
            }
            let r = self.expr(&ne, expected);
            if t0.is_some() {
                self.lets.remove(&(&ne as *const Expr as usize));
            }
            return r;
        }
        // `v[a..b].iter()` / `s[1..].len()`: 範囲の添字の受け手は自動で借用する (書き換えるメソッドは &mut)
        if let Expr::Index(_, ix, _) = strip_paren(recv) {
            if matches!(&**ix, Expr::Range(..))
                && !matches!(
                    method,
                    "sort" | "sort_unstable" | "sort_by" | "sort_unstable_by" | "sort_by_key" | "sort_unstable_by_key" | "sort_by_cached_key"
                )
            {
                let mutating = matches!(
                    method,
                    "reverse" | "swap" | "fill" | "rotate_left" | "rotate_right" | "iter_mut" | "copy_from_slice" | "clone_from_slice" | "split_at_mut"
                );
                let nr = paren(Expr::Ref(mutating, Box::new(recv.clone())));
                let ne = Expr::MethodCall(Box::new(nr), method.to_string(), gargs.clone(), margs.clone(), crate::parse::Span::default());
                // (型推論が元の呼び出しに記録した型を、作り直したノードにも写す: `s[..4].try_into()`)
                let t0 = self.lets.get(&(e as *const Expr as usize)).copied();
                if let Some(t) = t0 {
                    self.lets.insert(&ne as *const Expr as usize, t);
                }
                let r = self.expr(&ne, expected);
                if t0.is_some() {
                    self.lets.remove(&(&ne as *const Expr as usize));
                }
                return r;
            }
        }
        // 値としての `[a, b].into_iter()`: Vec にしてから (配列の変数で要素が Copy なら iter().copied())
        if method == "into_iter" && margs.is_empty() {
            if let Expr::Array(es) = strip_paren(recv) {
                let e = mcall(vec_of(es), "into_iter", vec![]);
                return self.expr(&e, expected);
            }
            if let Some(Ty::Array(el, _)) = self.peek(recv) {
                if !self.needs_drop(*el) && self.prog.struct_index.contains_key("Copied") {
                    let e = mcall(mcall(paren(recv.clone()), "iter", vec![]), "copied", vec![]);
                    return self.expr(&e, expected);
                }
            }
        }
        // ブランケット impl のメソッドをプリミティブ型・&str で呼ぶ (`5.describe()` / `"x".describe()`)
        if let Some(&bid) = self.prog.blanket.get(method) {
            let pt = self.peek(recv);
            let prim = |t: Ty| t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char);
            let own = match pt.map(|t| self.strip_refs(t)) {
                Some(Ty::Struct(sid)) => self.prog.method(sid, method).is_some(),
                _ => false,
            };
            if !own && pt.is_some_and(|t| prim(t) || matches!(t, Ty::Ref(_, Ty::Str))) {
                let (v, t) = self.expr(recv, None)?;
                let Some(v) = v else { return Ok((None, Ty::Never)) };
                // (&self に渡すのは値のアドレス。&str は fat pointer の置き場所のアドレスがそのまま &&str)
                let addr = if t.is_fat() {
                    v
                } else {
                    let ss = self.slot(t);
                    self.b.ins().stack_store(v, ss, 0);
                    self.b.ins().stack_addr(PTR, ss, 0)
                };
                let args: Vec<&Expr> = margs.iter().collect();
                return self.call_fn(bid, Some((addr, Ty::Ref(false, intern(t)))), &args, tf, expected, SP);
            }
        }
        // `b.as_ref()` / `b.as_mut()` (b: Box<T> / &Box<T>): 中身への参照 (Box と参照は同じ表現)
        if matches!(method, "as_ref" | "as_mut") && margs.is_empty() {
            let mut t = self.peek(recv);
            while let Some(Ty::Ref(_, i)) = t {
                t = Some(*i);
            }
            if let Some(Ty::Box(inner)) = t {
                let (v, mut vt) = self.read_no_move(recv)?;
                let Some(mut v) = v else { return Ok((None, Ty::Never)) };
                while let Ty::Ref(_, i) = vt {
                    v = if i.is_fat() { v } else { self.b.ins().load(PTR, mf(), v, 0) };
                    vt = *i;
                }
                return Ok((Some(v), Ty::Ref(method == "as_mut", inner)));
            }
        }
        // prelude の `v.__push_auto(x)` (partition など): x が `&T` で v が `Vec<T>` なら `*x` を push
        if method == "__push_auto" && margs.len() == 1 {
            let a = &margs[0];
            let elem = match self.peek(recv).map(|t| self.strip_refs(t)) {
                Some(Ty::Struct(sid)) => self.vec_fields(sid).map(|f| f.2),
                _ => None,
            };
            let e = match (self.peek(a), elem) {
                (Some(Ty::Ref(_, i)), Some(el)) if *i == el => {
                    mcall(paren(recv.clone()), "push", vec![Expr::Unary("*".into(), Box::new(paren(a.clone())), crate::parse::Span::default())])
                }
                _ => mcall(paren(recv.clone()), "push", vec![a.clone()]),
            };
            return self.expr(&e, expected);
        }
        // `os_string.push("..")` (OsString は String): 文字列を足す
        if method == "push" && margs.len() == 1 {
            let is_string = |t: Option<Ty>| {
                matches!(t.map(|t| self.strip_refs(t)), Some(Ty::Struct(sid)) if self.prog.structs[sid as usize].base.is_none() && self.prog.structs[sid as usize].name == "String")
            };
            let a = &margs[0];
            let at = self.peek(a);
            if is_string(self.peek(recv)) && (matches!(at.map(|t| self.strip_refs(t)), Some(Ty::Str)) || (matches!(at, Some(Ty::Ref(..))) && is_string(at))) {
                let e = mcall(paren(recv.clone()), "push_str", vec![a.clone()]);
                return self.expr(&e, expected);
            }
        }
        // prelude の `s.__push_piece(c)` (String への collect): 要素の型で push / push_str
        if method == "__push_piece" && margs.len() == 1 {
            let a = &margs[0];
            let r = paren(recv.clone());
            let e = match self.peek(a).map(|t| self.strip_refs(t)) {
                Some(Ty::Char) if matches!(self.peek(a), Some(Ty::Ref(..))) => mcall(r, "push", vec![Expr::Unary("*".into(), Box::new(paren(a.clone())), crate::parse::Span::default())]),
                Some(Ty::Char) | None => mcall(r, "push", vec![a.clone()]),
                Some(Ty::Str) => mcall(r, "push_str", vec![a.clone()]),
                _ => mcall(r, "push_str", vec![mcall(paren(a.clone()), "as_str", vec![])]),
            };
            return self.expr(&e, expected);
        }
        // `x.into()`: 期待型の `T::from(x)` (String は String::from、整数は広げるキャスト)
        if method == "into" && margs.is_empty() {
            let recv_own = match self.peek(recv) {
                Some(Ty::Struct(sid)) => self.prog.method(sid, "into").is_some(),
                _ => false,
            };
            let target = expected.or_else(|| self.lets.get(&(e as *const Expr as usize)).copied());
            if !recv_own {
                match target {
                    // `"msg".into()` / `e.into()` → Box<dyn Error>
                    Some(Ty::Struct(sid)) if self.prog.structs[sid as usize].name == "__DynError" => {
                        let (v, t) = self.expr(recv, None)?;
                        let Some(v) = v else { return Ok((None, Ty::Never)) };
                        return self.call_named("__exr_dyn_error", vec![t], &[(v, t)], SP);
                    }
                    Some(Ty::Struct(sid)) => {
                        if let Some(id) = self.prog.method(sid, "from") {
                            let args: [&Expr; 1] = [recv];
                            return self.call_fn(id, None, &args, None, Some(Ty::Struct(sid)), SP);
                        }
                    }
                    Some(t) if t.is_int() || t.is_float() => {
                        let e = Expr::Cast(Box::new(paren(recv.clone())), ty_named(&self.prog.tn(t)));
                        return self.expr(&e, Some(t));
                    }
                    _ => {}
                }
            }
        }
        // `x.to_string()` (Display を実装した型。&str は str_method が String::from にする)
        if method == "to_string" && margs.is_empty() && self.prog.index.contains_key("__exr_display") {
            if let Some(t) = self.peek(recv) {
                let mut b = t;
                while let Ty::Ref(_, i) = b {
                    b = *i;
                }
                let own = match b {
                    Ty::Struct(sid) => self.prog.method(sid, "to_string").is_some(),
                    _ => false,
                };
                if !own && b != Ty::Str && self.fmt_ok(t, false) {
                    return self.to_string_value(recv, SP);
                }
            }
        }
        if let Some(r) = self.ord_method(e)? {
            return Ok(r);
        }
        if let Some(r) = self.extend_call(e, expected)? {
            return Ok(r);
        }
        if let Some(r) = self.clone_call(e)? {
            return Ok(r);
        }
        // `x.clamp(lo, hi)` (整数も Ord な型も): prelude の __exr_clamp
        if method == "clamp" && margs.len() == 2 && self.prog.index.contains_key("__exr_clamp") {
            let own = match self.peek(recv).map(|t| self.strip_refs(t)) {
                Some(Ty::Struct(sid)) => self.prog.method(sid, "clamp").is_some(),
                Some(_) => false,
                // (型が分からない: 自分の型に clamp があるときだけ普通のメソッド呼び出しにする)
                None => self.prog.names.iter().any(|n| n.ends_with("::clamp")),
            };
            if !own {
                let e = Expr::Call(Box::new(path_expr("__exr_clamp")), vec![recv.clone(), margs[0].clone(), margs[1].clone()]);
                return self.expr(&e, expected);
            }
        }
        // `a.max(b)` / `a.min(b)` (Ord を実装した struct / タプル / String): prelude の __exr_max / __exr_min
        if matches!(method, "max" | "min") && margs.len() == 1 && self.prog.index.contains_key("__exr_max") {
            if let Some(Ty::Struct(sid)) = self.peek(recv) {
                if self.prog.method(sid, method).is_none() && self.prog.default_method(sid, method).is_none() {
                    let f = if method == "max" { "__exr_max" } else { "__exr_min" };
                    let e = Expr::Call(Box::new(path_expr(f)), vec![recv.clone(), margs[0].clone()]);
                    return self.expr(&e, expected);
                }
            }
        }
        // (str / スライスへの trait 実装は、組み込みのメソッドより先に: `"x".hash(&mut h)`)
        if self.peek(recv).is_some_and(|t| matches!(self.strip_refs(t), Ty::Str | Ty::Slice(_) | Ty::Array(..))) {
            if let Some(r) = self.prim_trait_method(e, expected)? {
                return Ok(r);
            }
        }
        if let Some(r) = self.builtin_method(e)? {
            return Ok(r);
        }
        if let Some(r) = self.prim_trait_method(e, expected)? {
            return Ok(r);
        }
        // (メソッドがなく、Deref の先が構造体でない (`PathBuf` → str): `recv.deref().m(..)` にして str などのメソッドを探す)
        if let Some(Ty::Struct(sid)) = self.peek(recv).map(|t| self.strip_refs(t)) {
            if self.prog.method(sid, method).is_none() && self.prog.default_method(sid, method).is_none() {
                if let Some(did) = self.prog.method(sid, "deref") {
                    if matches!(self.prog.sigs[did as usize].ret, Ty::Ref(_, Ty::Str | Ty::Slice(_))) {
                        let d = mcall(paren(recv.clone()), "deref", vec![]);
                        let e = Expr::MethodCall(Box::new(d), method.to_string(), gargs.clone(), margs.clone(), crate::parse::Span::default());
                        return self.expr(&e, expected);
                    }
                }
            }
        }
        let mut recv_temp = None;
        let mut p = if is_place_expr(recv) {
            self.place(recv)?
        } else {
            let r = self.expr(recv, None)?;
            // 一時値の受け手は文の終わりまで所有する (値で受けるメソッドならムーブ)
            if let (Some(v), t) = r {
                if t != Ty::Never {
                    recv_temp = self.own_temp(v, t);
                }
            }
            match r {
                (_, Ty::Never) => return Ok((None, Ty::Never)),
                (Some(a), t) if t.is_agg() => Place::Mem(a, 0, t),
                (Some(v), Ty::Ref(_, i) | Ty::Box(i)) => Place::Mem(v, 0, *i),
                (_, t) => {
                    return err(SP, format!("no method `{}` on type `{}`", method, self.prog.tn(t)));
                }
            }
        };
        let name = method.to_string();
        // (メソッドがなく `impl Deref` があれば、deref した先で探す: `rc.borrow_mut()`)
        let (a, o, sid, id) = loop {
            let (a, o, sid) = loop {
                p = match p {
                    Place::Mem(a, o, Ty::Struct(sid)) => break (a, o, sid),
                    Place::Mem(a, o, Ty::Ref(_, Ty::Dyn(tr)) | Ty::Box(Ty::Dyn(tr))) => {
                        let fat = self.addr_plus(a, o);
                        return self.dyn_method_call(fat, tr, e);
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
                        let t = self.prog.tn(p.ty());
                        return err(SP, format!("no method `{}` on type `{t}`", method));
                    }
                }
            };
            if let Some(id) = self.prog.method(sid, &name).or_else(|| self.prog.default_method(sid, &name)) {
                break (a, o, sid, id);
            }
            let sd = &self.prog.structs[sid as usize];
            let gens = sd.args.clone();
            let base = sd.name.split('<').next().unwrap_or("").to_string();
            if let Some(did) = self.prog.method(sid, "deref") {
                let target = match self.prog.sigs[did as usize].ret {
                    Ty::Ref(_, t) => *t,
                    _ => return err(SP, "bad `deref` signature"),
                };
                let target = self.prog.partial_subst(target, &gens.iter().map(|&g| Some(g)).collect::<Vec<_>>());
                let recv = self.addr_plus(a, o);
                let _ = base;
                let (v, vt) = self.call_fn_vals(did, &[(recv, Ty::Ref(false, intern(Ty::Struct(sid))))], SP)?;
                let inner_t = match vt {
                    Ty::Ref(_, t) => *t,
                    _ => target,
                };
                p = Place::Mem(v.unwrap(), 0, inner_t);
                continue;
            }
            let sname = self.prog.structs[sid as usize].name.clone();
            return err(SP, format!("no method named `{name}` found for `{sname}`"));
        };
        let sname = self.prog.structs[sid as usize].name.clone();
        if !self.prog.sigs[id as usize].has_self {
            return err(SP, format!("`{sname}::{name}` is an associated function, not a method"));
        }
        // struct のアドレスを渡す: `&self` ならそのまま、`self` なら call_fn がコピーする
        let recv_v = self.addr_plus(a, o);
        let st = Ty::Struct(sid);
        let recv_ty = match self.prog.sigs[id as usize].params[0] {
            Ty::Ref(m, _) => Ty::Ref(m, intern(st)),
            _ => st,
        };
        // `self` を値で受ける: 受け手からムーブ
        if !matches!(recv_ty, Ty::Ref(..)) && self.needs_drop(st) {
            match recv_temp {
                Some(f) => self.clear_flag(f),
                None => self.mark_moved(recv),
            }
        }
        let args: Vec<&Expr> = margs.iter().collect();
        self.call_fn(id, Some((recv_v, recv_ty)), &args, tf, expected, SP)
    }

    /// 組み込みの AsRef (`s.as_ref()` → `s.as_bytes()` / `&s[..]`)。受け手が as_ref を持つ構造体 (Option など) なら None
    pub(crate) fn as_ref_rewrite(&self, recv: &Expr, expected: Option<Ty>) -> Option<Expr> {
        let (target, is_string, seq) = self.as_ref_target(recv, expected)?;
        let r = paren(recv.clone());
        Some(if target == "str" {
            if is_string { mcall(r, "as_str", vec![]) } else { Expr::Ref(false, Box::new(Expr::Index(Box::new(r), Box::new(Expr::Range(None, None, false)), Default::default()))) }
        } else if !seq {
            mcall(r, "as_bytes", vec![])
        } else {
            Expr::Ref(false, Box::new(Expr::Index(Box::new(r), Box::new(Expr::Range(None, None, false)), Default::default())))
        })
    }

    /// 組み込みの AsRef の結果の型
    pub(crate) fn as_ref_ty(&self, recv: &Expr, expected: Option<Ty>) -> Option<Ty> {
        let (target, _, seq) = self.as_ref_target(recv, expected)?;
        if target == "str" {
            return Some(Ty::Ref(false, intern(Ty::Str)));
        }
        if !seq {
            return Some(Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8)))));
        }
        let elem = match self.strip_refs(self.peek(recv)?) {
            Ty::Array(e, _) | Ty::Slice(e) => *e,
            Ty::Struct(sid) => self.vec_fields(sid)?.2,
            _ => return None,
        };
        Some(Ty::Ref(false, intern(Ty::Slice(intern(elem)))))
    }

    /// (先 ("str" / "[u8]" / "[T]"), 受け手が String か, 受け手が Vec・配列・スライスか)
    fn as_ref_target(&self, recv: &Expr, expected: Option<Ty>) -> Option<(String, bool, bool)> {
        let t = self.peek(recv)?;
        let base = self.strip_refs(t);
        let is_string = matches!(base, Ty::Struct(sid) if self.prog.structs[self.prog.base_of(sid) as usize].name == "String");
        let seq = matches!(base, Ty::Array(..) | Ty::Slice(_))
            || matches!(base, Ty::Struct(sid) if self.prog.structs[self.prog.base_of(sid) as usize].name == "Vec");
        if !(base == Ty::Str || is_string || seq) {
            return None;
        }
        // 先: "str" か "[..]"
        let mut target: Option<String> = match expected {
            Some(Ty::Ref(_, Ty::Str)) => Some("str".into()),
            Some(Ty::Ref(_, Ty::Slice(e))) if **e == Ty::U8 => Some("[u8]".into()),
            Some(Ty::Ref(_, Ty::Slice(_))) => Some("[T]".into()),
            _ => None,
        };
        if target.is_none() {
            let sig = &self.prog.sigs[self.root as usize];
            let mut cands: Vec<(Option<Ty>, String)> = vec![];
            for (pn, bs) in &sig.generics {
                for b in bs {
                    let b: String = b.chars().filter(|c| !c.is_whitespace()).collect();
                    if let Some(x) = b.strip_prefix("AsRef<").and_then(|x| x.strip_suffix('>')) {
                        let pt = self.env.params.iter().find(|(n, _)| n == pn).map(|p| p.1);
                        cands.push((pt, x.to_string()));
                    }
                }
            }
            target = cands
                .iter()
                .find(|(pt, _)| pt.is_some_and(|p| p == t || self.strip_refs(p) == base))
                .or(if cands.len() == 1 { cands.first() } else { None })
                .map(|c| c.1.clone());
        }
        let target = target.unwrap_or_else(|| if seq { "[T]".into() } else { "str".into() });
        Some((target, is_string, seq))
    }

    /// プリミティブ型への trait 実装のメソッド (`impl Tr for u32` の `x.m()`)
    pub(crate) fn prim_trait_method(&mut self, e: &Expr, expected: Option<Ty>) -> R<Option<V>> {
        let Expr::MethodCall(recv, method, gargs, margs, _) = e else { unreachable!() };
        let recv: &Expr = recv;
        let tf: Option<&[GArg]> = (!gargs.is_empty()).then_some(&gargs[..]);
        // (接尾辞のない整数リテラル `5.d()` は i32)
        let lit_int = matches!(strip_paren(recv), Expr::Lit(t, _) if matches!(lit::lit(t), Some(Lit::Int(i)) if i.suffix().is_empty()));
        let Some(mut t) = self.peek(recv).or(lit_int.then_some(Ty::I32)) else {
            return Ok(None);
        };
        // `impl Tr for &[T]` のメソッド (`__slice::__ref_m`、self は &[T]): 受け手の型が &[T] のとき
        if let Ty::Ref(false, Ty::Slice(_)) = t {
            let f = format!("__slice::__ref_{method}");
            if let Some(&id) = self.prog.index.get(&f) {
                let a0 = match self.prog.sigs[id as usize].params.first() {
                    Some(Ty::Ref(m, Ty::Ref(..))) => Expr::Ref(*m, Box::new(recv.clone())),
                    _ => recv.clone(),
                };
                let mut args = vec![a0];
                args.extend(margs.iter().cloned());
                let c = Expr::Call(Box::new(path_expr(&f)), args);
                return self.expr(&c, expected).map(Some);
            }
        }
        while let Ty::Ref(_, i) = t {
            t = *i;
        }
        let name = method.clone();
        // str / スライス / 配列への trait 実装 (`impl Hash for str` / `for [T]`): 受け手は &str / &[T] (fat) か &[T; N]
        if matches!(t, Ty::Str | Ty::Slice(_) | Ty::Array(..)) {
            let Some(id) = self.prog.prim_method(t, &name) else {
                return Ok(None);
            };
            if !matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Ref(..))) {
                return Ok(None);
            }
            let (v, vt) = self.expr(recv, None)?;
            let Some(mut v) = v else { return Ok(Some((None, Ty::Never))) };
            let mut vt = vt;
            // (参照の参照は 1 段ずつ外し、&str / &[T] / &[T; N] で止める)
            while let Ty::Ref(_, i) = vt {
                if matches!(i, Ty::Str | Ty::Slice(_) | Ty::Array(..)) {
                    break;
                }
                // (`&&str` の値は `&str` (fat) の置き場所のアドレスで、`&str` の表現と同じ: 読まない)
                if !i.is_fat() {
                    v = self.b.ins().load(PTR, mf(), v, 0);
                }
                vt = *i;
            }
            if matches!(vt, Ty::Array(..)) {
                let a = v;
                v = a;
                vt = Ty::Ref(false, intern(vt));
            }
            let args: Vec<&Expr> = margs.iter().collect();
            return self.call_fn(id, Some((v, vt)), &args, tf, expected, SP).map(Some);
        }
        if !(t.is_int() || matches!(t, Ty::Bool | Ty::Char)) {
            return Ok(None);
        }
        let Some(id) = self.prog.prim_method(t, &name) else {
            return Ok(None);
        };
        // 受け手: 値ならそのまま、&self ならメモリに置いてそのアドレス
        let (v, vt) = self.expr(recv, lit_int.then_some(t))?;
        let mut v = v.unwrap();
        let mut vt = vt;
        while let Ty::Ref(_, i) = vt {
            v = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), v, 0);
            vt = *i;
        }
        let (recv_v, recv_ty) = match self.prog.sigs[id as usize].params.first() {
            Some(Ty::Ref(m, _)) => {
                let a = self.temp(t);
                self.b.ins().store(mf(), v, a, 0);
                (a, Ty::Ref(*m, intern(t)))
            }
            _ => (v, t),
        };
        let args: Vec<&Expr> = margs.iter().collect();
        self.call_fn(id, Some((recv_v, recv_ty)), &args, tf, expected, SP).map(Some)
    }

    /// 型引数を解いて、呼ぶべき関数 (インスタンス / trait 実装) とそのシグネチャを決める
    /// turbofish (`f::<A, B>`) で決まる型パラメータ
    pub(crate) fn turbofish_sol(&self, sig: &FnSig, turbofish: Option<&[GArg]>) -> R<Vec<Option<Ty>>> {
        let mut sol: Vec<Option<Ty>> = vec![None; sig.generics.len()];
        if let (Some(t), Some(st)) = (self.seg_self, sig.self_ty) {
            self.prog.match_ty(st, t, &mut sol);
        }
        // trait メソッドは先頭が Self なので、turbofish はその後ろの型パラメータに当たる
        let first = sig.trait_of.is_some() as usize;
        if let Some(tf) = turbofish {
            for (i, a) in tf.iter().enumerate() {
                let ty = match a {
                    GArg::Ty(t) => self.parse_ty(t)?,
                    // `f::<3>()`: const ジェネリクスの引数
                    GArg::Const(e) => {
                        let ts = crate::ast_types::const_arg_syn(e).or_else(|er| err(SP, er))?;
                        self.prog.resolve_env(&ts, &self.env).or_else(|er| err(SP, er))?
                    }
                    _ => return err(SP, "only type arguments are supported"),
                };
                if let Some(slot) = sol.get_mut(first + i) {
                    // `_` は推論に任せる。`Vec<_>` のように一部だけ `_` なら穴として持つ
                    *slot = (ty != Ty::Param(crate::program::HOLE)).then_some(ty);
                }
            }
        }
        Ok(sol)
    }

    pub(crate) fn resolve_call(&mut self, id: u32, arg_tys: &[Ty], mut sol: Vec<Option<Ty>>, ret_hint: Option<Ty>, span: Sp) -> R<(u32, FnSig)> {
        let sig = self.prog.sigs[id as usize].clone();
        if !sig.is_generic() {
            return Ok((id, sig));
        }
        for (&pt, &at) in sig.params.iter().zip(arg_tys) {
            if !self.prog.match_ty(pt, at, &mut sol) {
                return err(span, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(pt), self.prog.tn(at)));
            }
        }
        // 引数から決まらない型パラメータは期待される戻り値型から (`let s: S<i64> = S::new()`)
        if let Some(rt) = ret_hint.filter(|_| sol.iter().any(|s| s.is_none())) {
            let mut s2 = sol.clone();
            if self.prog.match_ty(sig.ret, rt, &mut s2) {
                sol = s2;
            }
        }
        self.solve_assoc_bounds(&sig, &mut sol);
        // prelude の helper のダミーの型引数 `G` (呼ばれた分だけインスタンスにするためのもの) は ()
        for (i, g) in sig.generics.iter().enumerate() {
            if g.0 == "G" && sol[i].is_none() {
                sol[i] = Some(Ty::Unit);
            }
        }
        if let Some(i) = sol.iter().position(|s| s.is_some_and(|t| has_hole(self.prog, t))) {
            return err(span, format!("cannot infer type parameter `{}`; annotate the type", sig.generics[i].0));
        }
        let Some(args) = sol.iter().copied().collect::<Option<Vec<Ty>>>() else {
            let i = sol.iter().position(|s| s.is_none()).unwrap();
            return err(span, format!("cannot infer type parameter `{}`", sig.generics[i].0));
        };
        // trait の関数を `Tr::m(&x)` で呼んだら、実装があればそちらへ静的ディスパッチ
        if sig.trait_of.is_some() {
            let m = self.prog.names[id as usize].rsplit("::").next().unwrap().to_string();
            let imp = match args[0] {
                Ty::Struct(sid) => self.prog.method(sid, &m),
                t if t.is_int() || matches!(t, Ty::Bool | Ty::Char) => self.prog.index.get(&format!("{}::{m}", self.prog.tn(t))).copied(),
                _ => None,
            };
            if let Some(imp) = imp {
                let isig = &self.prog.sigs[imp as usize];
                if !isig.is_generic() {
                    return Ok((imp, isig.clone()));
                }
                // ジェネリックな impl (`impl<T: D> D for Vec<T>`): Self との照合で impl の型引数を、
                // メソッド自身の型引数は trait 側の解 (Self の後ろ) から
                let mut s2: Vec<Option<Ty>> = vec![None; isig.generics.len()];
                if let Some(st) = isig.self_ty {
                    self.prog.match_ty(st, args[0], &mut s2);
                }
                let own = args.len() - 1;
                let k0 = s2.len().saturating_sub(own);
                for (j, &a) in args[1..].iter().enumerate() {
                    if let Some(slot) = s2.get_mut(k0 + j) {
                        if slot.is_none() {
                            *slot = Some(a);
                        }
                    }
                }
                let Some(iargs) = s2.into_iter().collect::<Option<Vec<Ty>>>() else {
                    return err(span, format!("cannot infer the type parameters of `{}`", self.prog.names[imp as usize]));
                };
                let mut g = self.insts.lock().unwrap();
                let iid = g.request(self.prog, imp, iargs).or_else(|e| err(span, e))?;
                self.requested.push(iid);
                return Ok((iid, g.sig(self.prog, iid).clone()));
            }
            if !sig.has_body {
                return err(span, format!("`{}` has no implementation for `{}`", self.prog.names[id as usize], self.prog.tn(args[0])));
            }
        }
        let mut g = self.insts.lock().unwrap();
        let iid = g.request(self.prog, id, args).or_else(|e| err(span, e))?;
        self.requested.push(iid);
        Ok((iid, g.sig(self.prog, iid).clone()))
    }

    /// 同じ名前の trait impl の関数 (`P::from` が `From<&str>` と `From<String>`) から、最初の引数の型で選ぶ
    pub(crate) fn pick_overload(&self, id: u32, recv: Option<Ty>, args: &[&Expr]) -> u32 {
        let Some(cands) = self.prog.overloads.get(&self.prog.names[id as usize]) else {
            return id;
        };
        let Some(at) = recv.or_else(|| args.first().and_then(|a| self.peek(a))) else {
            return id;
        };
        let fits = |c: u32, at: Ty| {
            let sig = &self.prog.sigs[c as usize];
            let mut sol = vec![None; sig.generics.len()];
            sig.params.first().is_some_and(|&pt| self.prog.match_ty(pt, at, &mut sol))
        };
        if let Some(&c) = cands.iter().find(|&&c| fits(c, at)) {
            return c;
        }
        // (`&String` は `&str` の候補に: deref の強制変換)
        let deref = match at {
            Ty::Ref(m, Ty::Struct(sid)) if self.prog.structs[*sid as usize].name == "String" => Some(Ty::Ref(m, intern(Ty::Str))),
            Ty::Ref(_, Ty::Ref(m, i)) => Some(Ty::Ref(*m, i)),
            _ => None,
        };
        deref.and_then(|d| cands.iter().copied().find(|&c| fits(c, d))).unwrap_or(id)
    }

    pub(crate) fn call_fn(&mut self, id: u32, recv: Option<(Value, Ty)>, args: &[&Expr], turbofish: Option<&[GArg]>, expected: Option<Ty>, span: Sp) -> R<V> {
        let id = self.pick_overload(id, recv.map(|r| r.1), args);
        let sig0 = self.prog.sigs[id as usize].clone();
        let skip = recv.is_some() as usize;
        if sig0.params.len() != args.len() + skip {
            return err(span, format!("expected {} arguments, found {}", sig0.params.len() - skip, args.len()));
        }
        // 実引数を先に評価する (ジェネリックなら型を見てから呼び先を決める)。
        // 型パラメータは turbofish → 前の引数の順に分かった分だけ、後の引数の期待型に使う
        let mut sol = self.turbofish_sol(&sig0, turbofish)?;
        // 期待型が戻り値と同じ型の (具体的な) インスタンスなら、そこから型パラメータを解いておく
        // (`let r: RefCell<Vec<String>> = RefCell::new(vec![]);` の空の vec![] の要素型)
        if let (Some(Ty::Struct(es)), Ty::Struct(rs)) = (expected, sig0.ret) {
            if !self.prog.has_param(Ty::Struct(es)) && self.prog.has_param(sig0.ret) && self.prog.base_of(es) == self.prog.base_of(rs) {
                self.prog.match_ty(sig0.ret, Ty::Struct(es), &mut sol);
            }
        }
        let mut vals: Vec<(Value, Ty, Sp)> = vec![];
        if let Some((r, rt)) = recv {
            self.prog.match_ty(sig0.params[0], rt, &mut sol);
            vals.push((r, rt, span));
        }
        // `Iterator` を受ける引数に Vec / &Vec / スライス / 配列を渡したら (IntoIterator 相当) イテレータにする
        let conv: Vec<Option<Expr>> = args
            .iter()
            .zip(&sig0.params[skip..])
            .map(|(a, &pt)| {
                let Ty::Param(g) = pt else { return None };
                let iter_bound = sig0.generics.get(g as usize).is_some_and(|gg| gg.1.iter().any(|b| b.starts_with("Iterator")));
                if !iter_bound {
                    return None;
                }
                // (型の記録がない配列リテラル `[(&a, &b)]` も配列として)
                if self.peek(a).is_none() && matches!(strip_paren(a), Expr::Array(_) | Expr::Repeat(..)) {
                    return Some(mcall(paren((*a).clone()), "into_iter", vec![]));
                }
                let t = self.peek(a)?;
                if self.is_iterator_ty(t) {
                    return None;
                }
                let mut base = t;
                while let Ty::Ref(_, i) = base {
                    if base.is_fat() {
                        break;
                    }
                    base = *i;
                }
                let is_vec = matches!(base, Ty::Struct(sid) if self.vec_fields(sid).is_some());
                // (Option も 0 個か 1 個のイテレータ: `it.chain(Some(x))`)
                let is_opt = matches!(base, Ty::Struct(sid) if self.prog.structs[self.prog.base_of(sid) as usize].name == "Option")
                    && self.prog.index.contains_key("Option::into_iter");
                let seq = is_vec || is_opt || matches!(base, Ty::Array(..) | Ty::Ref(_, Ty::Slice(_)));
                if !seq {
                    return None;
                }
                Some(match (t, strip_paren(a)) {
                    (Ty::Ref(..), Expr::Ref(_, inner)) => mcall(paren((**inner).clone()), "iter", vec![]),
                    (Ty::Ref(..), _) => mcall(paren((*a).clone()), "iter", vec![]),
                    _ => mcall(paren((*a).clone()), "into_iter", vec![]),
                })
            })
            .collect();
        let conv_refs: Vec<&Expr> = args.iter().zip(&conv).map(|(a, c)| c.as_ref().unwrap_or(*a)).collect();
        let args: &[&Expr] = &conv_refs;
        for (a, &pt) in args.iter().zip(&sig0.params[skip..]) {
            let mut exp = Some(self.prog.partial_subst(pt, &sol)).filter(|&t| !self.prog.has_param(t));
            // (`[i64; N]` の N がまだ分からなくても、配列のリテラルには要素の型を渡す。長さはリテラルから決まる)
            if exp.is_none() {
                if let Ty::Array(e, n) = self.prog.partial_subst(pt, &sol) {
                    if n >= crate::types::CONST_LEN_BASE && !self.prog.has_param(*e) {
                        if let Expr::Array(al) = strip_paren(a) {
                            exp = Some(Ty::Array(e, al.len() as u32));
                        }
                    }
                }
            }
            // `F: Fn(A) -> B` の引数にクロージャを渡すなら、A / B をクロージャの型の手がかりにする
            let fb = self.fn_bound_of(&sig0, pt);
            // 関数アイテム (`or_insert_with(Vec::new)`) なら、境界を関数ポインタの期待型にして型引数を決める
            if let (Some((bps, bret)), None, Expr::Path(..)) = (&fb, exp, strip_paren(a)) {
                let ft = crate::types::FnTy {
                    params: bps.iter().map(|&t| self.prog.partial_subst(t, &sol)).collect(),
                    ret: self.prog.partial_subst(*bret, &sol),
                };
                if !ft.params.iter().any(|&t| self.prog.has_param(t)) && !self.prog.has_param(ft.ret) {
                    exp = Some(Ty::Fn(crate::types::intern_fn(ft)));
                }
            }
            if let (Some((bps, bret)), Expr::Closure(..)) = (&fb, strip_paren(a)) {
                let ps = bps.iter().map(|&t| self.prog.partial_subst(t, &sol)).collect();
                let r = Some(self.prog.partial_subst(*bret, &sol));
                self.closure_hint = Some((ps, r));
            }
            let (v, mut t) = self.expr(a, exp)?;
            self.closure_hint = None;
            // クロージャの実際の引数・戻り値の型から境界の型パラメータを解く (`map<U, F: FnOnce(T) -> U>`)
            if let Some((bps, bret)) = &fb {
                let mut ct = t;
                while let Ty::Ref(_, i) = ct {
                    ct = *i;
                }
                if let Some((cps, cret)) = self.closure_sig(ct) {
                    for (&b, &c) in bps.iter().zip(&cps) {
                        self.prog.match_ty(b, c, &mut sol);
                    }
                    self.prog.match_ty(*bret, cret, &mut sol);
                }
            }
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            // ジェネリックな `&[T]` に `&[U; N]` / `&Vec<U>` を渡したら、型を解く前にスライスにする
            let mut v = v;
            if let (Ty::Ref(pm, Ty::Slice(_) | Ty::Str), Ty::Ref(am, Ty::Struct(sid)), Some(val)) = (pt, t, v) {
                if am || !pm {
                    if let Some((fat, ft)) = self.deref_to_slice(val, *sid, pm) {
                        let want_str = matches!(pt, Ty::Ref(_, Ty::Str));
                        if want_str == matches!(ft, Ty::Ref(_, Ty::Str)) {
                            v = Some(fat);
                            t = ft;
                        }
                    }
                }
            }
            if let (Ty::Ref(pm, Ty::Slice(_)), Ty::Ref(am, Ty::Array(e, n)), Some(val)) = (pt, t, v) {
                if am || !pm {
                    v = Some(self.make_fat(val, *n as i64));
                    t = Ty::Ref(pm, intern(Ty::Slice(e)));
                }
            }
            // 自動 deref 強制: `&&T` を `&T` の引数に渡す
            let want = self.prog.partial_subst(pt, &sol);
            // `m.get("k")` (キーが &str / String の map: std の Borrow 相当): &str を &&str / &String にする
            if let (Ty::Ref(_, wi), Ty::Ref(_, Ty::Str), Some(val)) = (want, t, v) {
                if matches!(wi, Ty::Ref(_, Ty::Str)) {
                    let tmp = self.temp(*wi);
                    self.copy(tmp, val, *wi);
                    v = Some(tmp);
                    t = want;
                } else if let Ty::Struct(ws) = wi {
                    if self.prog.structs[*ws as usize].base.is_none()
                        && self.prog.structs[*ws as usize].name == "String"
                        && self.prog.index.contains_key("String::from")
                    {
                        let sref = Ty::Ref(false, intern(Ty::Str));
                        let (sv, st) = self.call_named("String::from", vec![], &[(val, sref)], SP)?;
                        if let Some(sv) = sv {
                            self.own_temp(sv, st);
                            v = Some(sv);
                            t = Ty::Ref(false, intern(st));
                        }
                    }
                }
            }
            // `set.get(&t)` (要素が `&T` の HashSet / HashMap: std の Borrow 相当): `&T` を `&&T` にする
            if let (Ty::Ref(_, Ty::Ref(wm, wx)), Ty::Ref(tm, tx), Some(val)) = (want, t, v) {
                if **wx == *tx && !tx.is_fat() && !self.prog.has_param(want) && (!*wm || tm) {
                    let tmp = self.temp(Ty::Ref(*wm, wx));
                    self.b.ins().store(mf(), val, tmp, 0);
                    v = Some(tmp);
                    t = want;
                }
            }
            // (`&Box<T>` も `&T` に)
            while let (Ty::Ref(_, _), Ty::Ref(m, inner @ (Ty::Ref(..) | Ty::Box(_))), Some(val)) = (want, t, v) {
                if t == want || self.prog.has_param(want) {
                    break;
                }
                // fat な参照 (`&str` / `&[T]`) はメモリ上の 16 byte のアドレスが値なのでそのまま
                v = Some(if inner.is_fat() { val } else { self.b.ins().load(PTR, mf(), val, 0) });
                t = match inner {
                    Ty::Box(x) => Ty::Ref(m, x),
                    i => *i,
                };
            }
            if let (Ty::Ref(false, wi), Ty::Ref(true, gi)) = (want, t) {
                if wi == gi {
                    t = want;
                }
            }
            self.prog.match_ty(pt, t, &mut sol);
            // (`()` の引数は値を持たない: 渡すときに飛ばす)
            let v = match v {
                Some(v) => v,
                None => self.b.ins().iconst(types::I8, 0),
            };
            vals.push((v, t, expr_start(a)));
        }
        let arg_tys: Vec<Ty> = vals.iter().map(|v| v.1).collect();
        // 引数の型から改めて解く。引数に現れない型パラメータ (クロージャの戻り値など) は sol から
        let mut sol0 = self.turbofish_sol(&sig0, turbofish)?;
        for (a, b) in sol0.iter_mut().zip(&sol) {
            if a.is_none() {
                *a = *b;
            }
        }
        // (drop の要らない型の `__exr_drop_in_place::<T>` / `drop(x)` は何もしない: インスタンスも作らない)
        if matches!(self.prog.names[id as usize].as_str(), "__exr_drop_in_place" | "__core_drop") && arg_tys.len() == 1 {
            let t = match arg_tys[0] {
                Ty::Ptr(_, t) if self.prog.names[id as usize] == "__exr_drop_in_place" => Some(*t),
                t if self.prog.names[id as usize] == "__core_drop" => Some(t),
                _ => None,
            };
            if t.is_some_and(|t| !self.prog.has_param(t) && !self.needs_drop(t)) {
                return Ok((None, Ty::Unit));
            }
        }
        let (target, sig) = self.resolve_call(id, &arg_tys, sol0, expected, span)?;
        let mut cv = vec![];
        let sret = if is_sret(&sig) {
            let a = self.temp(sig.ret);
            cv.push(a);
            Some(a)
        } else {
            None
        };
        for ((v, t, sp), &pt) in vals.into_iter().zip(&sig.params) {
            let (v, t) = if matches!(pt, Ty::Ref(false, Ty::Str)) {
                let (v2, t2) = self.deref_coerce(Some(v), t, pt);
                (v2.unwrap_or(v), t2)
            } else {
                (v, t)
            };
            // 参照の参照を参照に (deref の強制変換: `t.starts_with(k)` で k: &&str)
            let (v, t) = match (t, pt) {
                (Ty::Ref(_, a), Ty::Ref(_, pi)) if matches!(*a, Ty::Ref(_, i) if *i == *pi) => {
                    if pt.is_fat() {
                        // (&&str は fat pointer の組を指すアドレス: それがそのまま &str の値)
                        (v, pt)
                    } else {
                        (self.b.ins().load(PTR, mf(), v, 0), pt)
                    }
                }
                _ => (v, t),
            };
            self.same(sp, t, pt)?;
            self.push_arg(&mut cv, v, pt);
        }
        // (-O: 小さい prelude の関数は呼ばずに本体を展開する)
        if let Some(r) = self.try_inline(target, &sig, &cv)? {
            if let Some(a) = sret {
                return Ok((Some(a), sig.ret));
            }
            return Ok((r, sig.ret));
        }
        let f = self.fref(target, &sig);
        let call = self.b.ins().call(f, &cv);
        if sig.ret == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(self.dead());
        }
        if let Some(a) = sret {
            return Ok((Some(a), sig.ret));
        }
        let r = self.b.inst_results(call).first().copied();
        Ok((r, sig.ret))
    }

    pub(crate) fn push_arg(&mut self, vals: &mut Vec<Value>, v: Value, pt: Ty) {
        if !pt.is_agg() && clif_ty(pt).is_none() {
            return;
        }
        if pt.is_agg() {
            // 値渡し: 呼び出し側でコピーしてから渡す
            let tmp = self.temp(pt);
            self.copy(tmp, v, pt);
            vals.push(tmp);
        } else {
            vals.push(v);
        }
    }

    pub(crate) fn convert(&mut self, v: Value, from: Ty, to: Ty) -> Value {
        let (fb, tb) = (from.bits(), to.bits());
        let tt = clif_ty(to).unwrap();
        if fb == tb {
            v
        } else if fb > tb {
            self.b.ins().ireduce(tt, v)
        } else if from.signed() {
            self.b.ins().sextend(tt, v)
        } else {
            self.b.ins().uextend(tt, v)
        }
    }

    /// リテラル (e は `Expr::Lit` 全体)
    pub(crate) fn lit(&mut self, text: &str, expected: Option<Ty>) -> R<V> {
        let Some(l) = lit::lit(text) else { return err(SP, "unsupported literal") };
        match &l {
            // 浮動小数点: `1.5` / `2e3` / `1.0f32` / `1f64` (と、浮動小数点が期待される整数リテラル)
            Lit::Float(_) | Lit::Int(_)
                if matches!(l, Lit::Float(_))
                    || matches!(&l, Lit::Int(i) if matches!(i.suffix(), "f32" | "f64"))
                    || (matches!(&l, Lit::Int(i) if i.suffix().is_empty()) && expected.is_some_and(|t| t.is_float())) =>
            {
                let (digits, suffix) = match &l {
                    Lit::Float(f) => (f.base10_digits().to_string(), f.suffix().to_string()),
                    Lit::Int(i) => (i.base10_digits().to_string(), i.suffix().to_string()),
                    _ => unreachable!(),
                };
                let ty = match suffix.as_str() {
                    "f32" => Ty::F32,
                    "f64" => Ty::F64,
                    _ => expected.filter(|t| t.is_float()).unwrap_or(Ty::F64),
                };
                let x: f64 = digits.parse().map_err(|e: std::num::ParseFloatError| e.to_string())?;
                let v = if ty == Ty::F32 { self.b.ins().f32const(x as f32) } else { self.b.ins().f64const(x) };
                Ok((Some(v), ty))
            }
            Lit::Int(i) => {
                let ty = Ty::from_suffix(i.suffix()).or(expected.filter(|t| t.is_int())).unwrap_or(Ty::I32);
                // 負のリテラル (パターンの `-1` など) も来るので i128 で読む
                let n = i
                    .base10_digits()
                    .parse::<i128>()
                    .or_else(|_| i.base10_digits().parse::<u128>().map(|v| v as i128))
                    .map_err(|e| e.to_string())?;
                let v = self.int_const(ty, n);
                Ok((Some(v), ty))
            }
            Lit::Bool(b) => Ok((Some(self.b.ins().iconst(types::I8, *b as i64)), Ty::Bool)),
            Lit::Byte(b) => Ok((Some(self.b.ins().iconst(types::I8, *b as i64)), Ty::U8)),
            Lit::Char(c) => Ok((Some(self.b.ins().iconst(types::I32, *c as i64)), Ty::Char)),
            // "..." は &'static str (fat pointer)、b"..." は &'static [u8; N]
            Lit::Str(st) => {
                let bytes = st.clone().into_bytes();
                let n = bytes.len() as i64;
                let a = self.data_addr(bytes);
                Ok((Some(self.make_fat(a, n)), Ty::Ref(false, intern(Ty::Str))))
            }
            Lit::ByteStr(bs) => {
                let bytes = bs.clone();
                let n = bytes.len() as u32;
                let a = self.data_addr(bytes);
                Ok((Some(a), Ty::Ref(false, intern(Ty::Array(intern(Ty::U8), n)))))
            }
            _ => err(SP, "unsupported literal"),
        }
    }

    /// of なら None、そうでなければ Some(v)
    pub(crate) fn option_if(&mut self, v: Value, t: Ty, of: Value) -> R<V> {
        let ot = {
            let ob = *self.prog.struct_index.get("Option").ok_or("Option is not available")?;
            Ty::Struct(self.prog.instantiate(ob, vec![t])?)
        };
        let Ty::Struct(oi) = ot else { unreachable!() };
        let out = self.temp(ot);
        let some = self.prog.structs[oi as usize].variants.iter().position(|v| v.name == "Some").unwrap();
        let none = self.prog.structs[oi as usize].variants.iter().position(|v| v.name == "None").unwrap();
        let sb = self.b.create_block();
        let nb = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().brif(of, nb, &[], sb, &[]);
        self.b.switch_to_block(sb);
        self.store_tag(out, oi, some);
        let off = self.prog.structs[oi as usize].variants[some].fields[0].2;
        self.b.ins().store(mf(), v, out, off as i32);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(nb);
        self.store_tag(out, oi, none);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        Ok((Some(out), ot))
    }

    /// `(v, flag)` のタプル
    pub(crate) fn pair_with_bool(&mut self, v: Value, t: Ty, flag: Value) -> R<V> {
        let sid = self.prog.instantiate(self.prog.tuple_base(2), vec![t, Ty::Bool])?;
        let tt = Ty::Struct(sid);
        let out = self.temp(tt);
        let (o0, o1) = {
            let f = &self.prog.structs[sid as usize].fields;
            (f[0].2, f[1].2)
        };
        self.b.ins().store(mf(), v, out, o0 as i32);
        self.b.ins().store(mf(), flag, out, o1 as i32);
        Ok((Some(out), tt))
    }

    /// clz / ctz などの結果 (受け手の幅) を u32 に
    pub(crate) fn to_u32(&mut self, v: Value, ct: Type) -> Value {
        match ct.bits() {
            32 => v,
            b if b > 32 => self.b.ins().ireduce(types::I32, v),
            _ => self.b.ins().uextend(types::I32, v),
        }
    }

    /// 128 bit の除算・剰余 (prelude の関数)
    pub(crate) fn div128(&mut self, l: Value, r: Value, t: Ty, rem: bool, span: Sp) -> R<Value> {
        let f = if t.signed() { "__exr_sdiv128" } else { "__exr_udiv128" };
        let rv = self.b.ins().iconst(types::I8, rem as i64);
        let (v, _) = self.call_named(f, vec![Ty::Unit], &[(l, t), (r, t), (rv, Ty::Bool)], span)?;
        Ok(v.unwrap())
    }

    /// iconst (I128 は 64 bit から符号拡張する。Cranelift の iconst は 64 bit まで)
    pub(crate) fn iconst_t(&mut self, ct: Type, x: i64) -> Value {
        if ct == types::I128 {
            let v = self.b.ins().iconst(types::I64, x);
            self.b.ins().sextend(types::I128, v)
        } else {
            self.b.ins().iconst(ct, x)
        }
    }

    /// 整数の定数 (128 bit は 64 bit の 2 つをつなぐ)
    pub(crate) fn int_const(&mut self, ty: Ty, n: i128) -> Value {
        if ty.bits() == 128 {
            let lo = self.b.ins().iconst(types::I64, n as u64 as i64);
            let hi = self.b.ins().iconst(types::I64, (n >> 64) as u64 as i64);
            self.b.ins().iconcat(lo, hi)
        } else {
            self.b.ins().iconst(clif_ty(ty).unwrap(), n as u64 as i64)
        }
    }

    /// `i32::pow(2, 10)` / `Trait::m(&x, a)` / `Ord::cmp(&a, &b)` / `HashMap::from([..])` などを
    /// メソッド呼び出しなどに書き換えたもの (front.rs の Program::prim_method_call。e は `Expr::Call` 全体)
    pub(crate) fn prim_method_call(&self, e: &Expr) -> Option<Expr> {
        let Expr::Call(func, cargs) = e else { return None };
        let Expr::Path(qself, p) = &**func else { return None };
        if cargs.is_empty() || qself.is_some() {
            return None;
        }
        // `Trait::m(&x, a)` / `crate::m::Trait::m(..)` (self を取るユーザ trait のメソッド): `x.m(a)` に
        {
            let pp = PPath::of(p);
            let q = self.prog.strip_mods_p(&pp);
            let q = q.as_ref().unwrap_or(&pp);
            if q.segs.len() == 2 && q.segs.iter().all(|s| !s.args) {
                let tr = q.segs[0].name.clone();
                let ms = q.segs[1].name.clone();
                if let Some(ds) = self.prog.traits.get(&tr) {
                    let selfm = ds
                        .iter()
                        .any(|&d| self.prog.sigs[d as usize].has_self && self.prog.names[d as usize].rsplit("::").next() == Some(ms.as_str()));
                    if selfm && !matches!(tr.as_str(), "Ord" | "PartialOrd" | "PartialEq" | "Eq") {
                        let mut args = cargs.iter();
                        let a0 = args.next()?;
                        let rest: Vec<Expr> = args.cloned().collect();
                        let recv = match a0 {
                            Expr::Ref(_, e) => paren((**e).clone()),
                            _ => paren(a0.clone()),
                        };
                        return Some(Expr::MethodCall(Box::new(recv), ms, vec![], rest, crate::parse::Span::default()));
                    }
                }
            }
        }
        if p.segs.len() != 2 {
            return None;
        }
        let ty = p.segs[0].name.as_str();
        // `HashMap::from([(1, 2), ..])`: 配列を vec![..] にして `from(Vec)` を呼ぶ
        if p.segs[1].name == "from"
            && matches!(ty, "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "VecDeque" | "BinaryHeap")
            && cargs.len() == 1
        {
            if let Expr::Array(es) = &cargs[0] {
                return Some(Expr::Call(func.clone(), vec![vec_of(es)]));
            }
        }
        // `Ord::cmp(&a, &b)` / `PartialOrd::partial_cmp(..)` / `PartialEq::eq(..)`: メソッド呼び出しの形に
        if matches!(ty, "Ord" | "PartialOrd" | "PartialEq" | "Eq") && cargs.len() == 2 {
            let m = &p.segs[1].name;
            return Some(Expr::MethodCall(Box::new(paren(cargs[0].clone())), m.clone(), vec![], vec![cargs[1].clone()], crate::parse::Span::default()));
        }
        let t = Ty::prim(ty)?;
        if t == Ty::Char || t == Ty::Bool {
            return None;
        }
        let ms = p.segs[1].name.as_str();
        let pp = PPath::of(p);
        // `i64::from(7u32)` / `u32::from('a')`: 数値への変換はキャスト (広げる変換だけが書ける)
        if ms == "from" && cargs.len() == 1 && self.prog.fn_by_p(&pp, self.self_sid).is_none() {
            let a0 = &cargs[0];
            return Some(paren(Expr::Cast(Box::new(paren(a0.clone())), ty_named(ty))));
        }
        if ms.starts_with("from") || ms.starts_with("try_from") || ms == "default" || ms == "new" {
            return None;
        }
        if self.prog.fn_by_p(&pp, self.self_sid).is_some() {
            return None;
        }
        let mut args = cargs.iter();
        let a0 = args.next()?;
        let rest: Vec<Expr> = args.cloned().collect();
        // (リテラルは型を付ける: `i32::pow(2, 10)` の 2 は i32)
        let recv = if matches!(a0, Expr::Lit(_, _)) { paren(Expr::Cast(Box::new(a0.clone()), ty_named(ty))) } else { paren(a0.clone()) };
        Some(Expr::MethodCall(Box::new(recv), ms.to_string(), vec![], rest, crate::parse::Span::default()))
    }
}
