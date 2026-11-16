//! 自前の構文木 (src/parse.rs) から minclif (src/minclif.rs) で機械語を作るコード生成。
//! syn と Cranelift に依存しないコンパイラの経路 (parse → ast_summary → program → ast_infer → ここ → minclif → ELF) の最後の段。
//! codegen.rs (syn + Cranelift) を置き換えるために、扱える言語の範囲を少しずつ広げていく。
//!
//! いま扱えるもの: 整数・bool・char・キャスト、let / 代入 / 複合代入、四則・ビット演算・比較・`&&` / `||`、
//! if / while / loop / break / continue / return、ユーザの関数と extern "C" の関数の呼び出し。
//! 外部クレートに依存しないので crustc 自身でもコンパイルできる。tests/ast_codegen.rs が rustc と出力を比べる。

use crate::ast_infer::LetTypes;
use crate::lexer::{ItemKind as LexKind, SrcItem};
use crate::minclif::{Block as CBlock, FunctionBuilder, IntCC, Type, Value, Variable};
use crate::parse::{Block, Expr, ItemKind, Pat, Path, Stmt};
use crate::program::{FnSig, PPath, Program, TyEnv};
use crate::types::{Summary, Ty};

pub type R<T> = Result<T, String>;

/// Ty → 機械の型 (値を持たない型は None)
pub fn clif_ty(t: Ty) -> Option<Type> {
    Some(match t {
        Ty::I8 | Ty::U8 | Ty::Bool => Type::I8,
        Ty::I16 | Ty::U16 => Type::I16,
        Ty::I32 | Ty::U32 | Ty::Char => Type::I32,
        Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize | Ty::Ptr(..) | Ty::Ref(..) | Ty::Fn(_) => Type::I64,
        Ty::I128 | Ty::U128 => Type::I128,
        Ty::F32 => Type::F32,
        Ty::F64 => Type::F64,
        _ => return None,
    })
}

/// 式の値と型
type V = (Option<Value>, Ty);

struct Loop {
    cont: CBlock,
    brk: CBlock,
}

struct Cx<'a> {
    b: FunctionBuilder,
    prog: &'a Program,
    lets: LetTypes,
    scopes: Vec<Vec<(String, Variable, Ty)>>,
    loops: Vec<Loop>,
    ret: Ty,
    env: TyEnv,
}

fn ident(p: &Path) -> Option<&str> {
    if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() && p.segs[0].fn_args.is_none() {
        Some(&p.segs[0].name)
    } else {
        None
    }
}

/// 整数・bool・char・バイトのリテラルの値
fn lit_value(t: &str) -> Option<i64> {
    if t == "true" {
        return Some(1);
    }
    if t == "false" {
        return Some(0);
    }
    if t.starts_with("b'") || t.starts_with('\'') {
        let inner = t.trim_start_matches('b');
        let inner = &inner[1..inner.len() - 1];
        let s = format!("\"{inner}\"");
        let (_, bytes) = crate::ast_consteval::str_lit(&s)?;
        let st = String::from_utf8(bytes).ok()?;
        return st.chars().next().map(|c| c as i64);
    }
    let s: String = t.chars().filter(|&c| c != '_').collect();
    let (radix, body) = if let Some(h) = s.strip_prefix("0x") {
        (16, h)
    } else if let Some(b) = s.strip_prefix("0b") {
        (2, b)
    } else if let Some(o) = s.strip_prefix("0o") {
        (8, o)
    } else {
        (10, s.as_str())
    };
    let digits: String = body.chars().take_while(|c| c.is_digit(radix)).collect();
    u64::from_str_radix(&digits, radix).ok().map(|v| v as i64)
}

impl Cx<'_> {
    /// 書かれた型
    fn parse_ty(&self, t: &crate::parse::Ty) -> Option<Ty> {
        let ts = crate::ast_types::tysyn(t, Some(&crate::types::TySyn::Named("Self".into()))).ok()?;
        self.prog.resolve_env(&ts, &self.env).ok()
    }

    fn err<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(msg.into())
    }

    fn lookup(&self, n: &str) -> Option<(Variable, Ty)> {
        self.scopes.iter().rev().flat_map(|s| s.iter().rev()).find(|x| x.0 == n).map(|x| (x.1, x.2))
    }

    fn bind(&mut self, n: &str, t: Ty, v: Option<Value>) -> R<()> {
        let Some(ct) = clif_ty(t) else {
            // (() の変数: 値は持たない)
            let var = self.b.declare_var(Type::I8);
            self.scopes.last_mut().unwrap().push((n.to_string(), var, t));
            return Ok(());
        };
        let var = self.b.declare_var(ct);
        if let Some(v) = v {
            self.b.def_var(var, v);
        }
        self.scopes.last_mut().unwrap().push((n.to_string(), var, t));
        Ok(())
    }

    /// 推論が記録した式の型
    fn peek(&self, e: &Expr) -> Option<Ty> {
        self.lets.get(&(e as *const Expr as usize)).copied()
    }

    /// 発散した後のコード用のブロック (どこからも飛ばない)
    fn dead_block(&mut self) {
        let d = self.b.create_block();
        self.b.switch_to_block(d);
    }

    fn block(&mut self, blk: &Block, expected: Option<Ty>) -> R<V> {
        self.scopes.push(vec![]);
        let n = blk.stmts.len();
        let mut res: V = (None, Ty::Unit);
        for (i, st) in blk.stmts.iter().enumerate() {
            match st {
                Stmt::Let(pat, ann, init, _) => {
                    let Pat::Ident(_, _, name, None) = pat else {
                        return self.err("only `let name = ..` is supported");
                    };
                    let t = self.lets.get(&(st as *const Stmt as usize)).copied();
                    let t = match (ann, t) {
                        (_, Some(t)) => t,
                        (Some(_), None) | (None, None) => return self.err(format!("cannot infer the type of `{name}`")),
                    };
                    let v = match init {
                        Some(e) => {
                            let (v, et) = self.expr(e, Some(t))?;
                            if et == Ty::Never {
                                self.dead_block();
                            }
                            v
                        }
                        None => None,
                    };
                    self.bind(name, t, v)?;
                }
                Stmt::Expr(e, semi) => {
                    let last = i + 1 == n && !*semi;
                    let r = self.expr(e, if last { expected } else { None })?;
                    if r.1 == Ty::Never {
                        self.dead_block();
                        res = (None, Ty::Never);
                    } else if last {
                        res = r;
                    }
                }
                Stmt::Item(_) => {}
            }
        }
        self.scopes.pop();
        Ok(res)
    }

    fn int_cc(op: &str, signed: bool) -> Option<IntCC> {
        Some(match (op, signed) {
            ("==", _) => IntCC::Equal,
            ("!=", _) => IntCC::NotEqual,
            ("<", true) => IntCC::SignedLessThan,
            ("<=", true) => IntCC::SignedLessThanOrEqual,
            (">", true) => IntCC::SignedGreaterThan,
            (">=", true) => IntCC::SignedGreaterThanOrEqual,
            ("<", false) => IntCC::UnsignedLessThan,
            ("<=", false) => IntCC::UnsignedLessThanOrEqual,
            (">", false) => IntCC::UnsignedGreaterThan,
            (">=", false) => IntCC::UnsignedGreaterThanOrEqual,
            _ => return None,
        })
    }

    /// 整数の 2 項演算 (op は `+` など)
    fn arith(&mut self, op: &str, l: Value, r: Value, t: Ty) -> R<Value> {
        let s = t.signed();
        Ok(match op {
            "+" => self.b.iadd(l, r),
            "-" => self.b.isub(l, r),
            "*" => self.b.imul(l, r),
            "/" if s => self.b.sdiv(l, r),
            "/" => self.b.udiv(l, r),
            "%" if s => self.b.srem(l, r),
            "%" => self.b.urem(l, r),
            "&" => self.b.band(l, r),
            "|" => self.b.bor(l, r),
            "^" => self.b.bxor(l, r),
            "<<" => self.b.ishl(l, r),
            ">>" if s => self.b.sshr(l, r),
            ">>" => self.b.ushr(l, r),
            _ => return self.err(format!("unsupported operator `{op}`")),
        })
    }

    fn binary(&mut self, op: &str, le: &Expr, re: &Expr, expected: Option<Ty>) -> R<V> {
        if op == "&&" || op == "||" {
            // 短絡評価: 結果はブロック引数
            let (l, _) = self.expr(le, Some(Ty::Bool))?;
            let l = l.ok_or("bad operand")?;
            let rhs = self.b.create_block();
            let done = self.b.create_block();
            let res = self.b.append_block_param(done, Type::I8);
            if op == "&&" {
                self.b.brif(l, rhs, &[], done, &[l]);
            } else {
                self.b.brif(l, done, &[l], rhs, &[]);
            }
            self.b.switch_to_block(rhs);
            let (r, _) = self.expr(re, Some(Ty::Bool))?;
            let r = r.ok_or("bad operand")?;
            self.b.jump(done, &[r]);
            self.b.switch_to_block(done);
            return Ok((Some(res), Ty::Bool));
        }
        let cmp = matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=");
        let hint = if cmp { self.peek(le).or(self.peek(re)) } else { expected.or(self.peek(le)) };
        let (l, lt) = self.expr(le, hint)?;
        let rhint = if matches!(op, "<<" | ">>") { None } else { Some(lt) };
        let (r, rt) = self.expr(re, rhint)?;
        let (Some(l), Some(r)) = (l, r) else { return self.err("bad operand") };
        if cmp {
            let cc = Self::int_cc(op, lt.signed()).ok_or("bad comparison")?;
            return Ok((Some(self.b.icmp(cc, l, r)), Ty::Bool));
        }
        // (シフト量は左辺の型に合わせる)
        let r = if matches!(op, "<<" | ">>") && rt != lt { self.convert(r, rt, lt) } else { r };
        Ok((Some(self.arith(op, l, r, lt)?), lt))
    }

    /// 整数の型の変換 (`as`)
    fn convert(&mut self, v: Value, from: Ty, to: Ty) -> Value {
        let (Some(ft), Some(tt)) = (clif_ty(from), clif_ty(to)) else { return v };
        if ft == tt {
            return v;
        }
        if tt.bytes() < ft.bytes() {
            self.b.ireduce(tt, v)
        } else if from.signed() {
            self.b.sextend(tt, v)
        } else {
            self.b.uextend(tt, v)
        }
    }

    fn call(&mut self, f: &Expr, args: &[Expr]) -> R<V> {
        let Expr::Path(None, p) = f else { return self.err("unsupported call") };
        let Some(id) = self.prog.fn_by_p(&PPath::of(p), None) else {
            return self.err(format!("cannot find function `{}`", p.segs.last().map(|s| s.name.as_str()).unwrap_or("")));
        };
        let sig = self.prog.sigs[id as usize].clone();
        if sig.is_generic() {
            return self.err("generic functions are not supported yet");
        }
        let mut vals = vec![];
        for (a, &pt) in args.iter().zip(&sig.params) {
            let (v, _) = self.expr(a, Some(pt))?;
            if let Some(v) = v {
                vals.push(v);
            }
        }
        let fr = self.b.import_function(&sig.symbol);
        let rets: Vec<Type> = clif_ty(sig.ret).into_iter().collect();
        let r = self.b.call(fr, &vals, &rets);
        if sig.ret == Ty::Never {
            self.b.trap();
            return Ok((None, Ty::Never));
        }
        Ok((r.first().copied(), sig.ret))
    }

    fn place_var(&self, e: &Expr) -> R<(Variable, Ty)> {
        match e {
            Expr::Path(None, p) => {
                let n = ident(p).ok_or("unsupported assignment target")?;
                self.lookup(n).ok_or_else(|| format!("cannot find `{n}`"))
            }
            Expr::Paren(x) => self.place_var(x),
            _ => Err("unsupported assignment target".to_string()),
        }
    }

    fn expr(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        match e {
            Expr::Lit(t) => {
                let ty = self.peek(e).or(expected).unwrap_or(if t == "true" || t == "false" {
                    Ty::Bool
                } else if t.starts_with("b'") {
                    Ty::U8
                } else if t.starts_with('\'') {
                    Ty::Char
                } else {
                    Ty::I32
                });
                let x = lit_value(t).ok_or_else(|| format!("unsupported literal `{t}`"))?;
                let ct = clif_ty(ty).ok_or("bad literal type")?;
                Ok((Some(self.b.iconst(ct, x)), ty))
            }
            Expr::Paren(x) => self.expr(x, expected),
            Expr::Path(None, p) => {
                let n = ident(p).ok_or("unsupported path")?;
                let (var, t) = self.lookup(n).ok_or_else(|| format!("cannot find value `{n}`"))?;
                if clif_ty(t).is_none() {
                    return Ok((None, t));
                }
                Ok((Some(self.b.use_var(var)), t))
            }
            Expr::Unary(op, x) => {
                let (v, t) = self.expr(x, expected)?;
                let v = v.ok_or("bad operand")?;
                Ok((
                    Some(match op.as_str() {
                        "-" => self.b.ineg(v),
                        "!" if t == Ty::Bool => {
                            let one = self.b.iconst(Type::I8, 1);
                            self.b.bxor(v, one)
                        }
                        "!" => self.b.bnot(v),
                        _ => return self.err(format!("unsupported unary `{op}`")),
                    }),
                    t,
                ))
            }
            Expr::Binary(op, l, r) => self.binary(op, l, r, expected),
            Expr::Assign(l, r) => {
                let (var, t) = self.place_var(l)?;
                let (v, _) = self.expr(r, Some(t))?;
                if let Some(v) = v {
                    self.b.def_var(var, v);
                }
                Ok((None, Ty::Unit))
            }
            Expr::AssignOp(op, l, r) => {
                let (var, t) = self.place_var(l)?;
                let cur = self.b.use_var(var);
                let rh = if matches!(op.as_str(), "<<" | ">>") { None } else { Some(t) };
                let (v, rt) = self.expr(r, rh)?;
                let v = v.ok_or("bad operand")?;
                let v = if rt != t { self.convert(v, rt, t) } else { v };
                let n = self.arith(op, cur, v, t)?;
                self.b.def_var(var, n);
                Ok((None, Ty::Unit))
            }
            Expr::Cast(x, ty) => {
                let to = self.parse_ty(ty).or(self.peek(e)).ok_or("cannot infer the cast type")?;
                let (v, from) = self.expr(x, None)?;
                let v = v.ok_or("bad cast operand")?;
                Ok((Some(self.convert(v, from, to)), to))
            }
            Expr::Block(_, _, b) => self.block(b, expected),
            Expr::If(c, then, els) => {
                let (cv, _) = self.expr(c, Some(Ty::Bool))?;
                let cv = cv.ok_or("bad condition")?;
                let tb = self.b.create_block();
                let eb = self.b.create_block();
                let done = self.b.create_block();
                self.b.brif(cv, tb, &[], eb, &[]);
                self.b.switch_to_block(tb);
                let (tv, tt) = self.block(then, expected)?;
                let rt = if tt == Ty::Never { None } else { Some(tt) };
                let param = match (els, rt.and_then(clif_ty)) {
                    (Some(_), Some(ct)) => Some(self.b.append_block_param(done, ct)),
                    _ => None,
                };
                if tt != Ty::Never {
                    let args: Vec<Value> = if param.is_some() { tv.into_iter().collect() } else { vec![] };
                    self.b.jump(done, &args);
                }
                self.b.switch_to_block(eb);
                let mut et = Ty::Unit;
                if let Some(x) = els {
                    let (ev, t2) = self.expr(x, rt.or(expected))?;
                    et = t2;
                    if t2 != Ty::Never {
                        let args: Vec<Value> = if param.is_some() { ev.into_iter().collect() } else { vec![] };
                        self.b.jump(done, &args);
                    }
                } else {
                    self.b.jump(done, &[]);
                }
                self.b.switch_to_block(done);
                let ty = if tt == Ty::Never { et } else { tt };
                if tt == Ty::Never && et == Ty::Never {
                    return Ok((None, Ty::Never));
                }
                Ok((param, if els.is_some() { ty } else { Ty::Unit }))
            }
            Expr::While(_, c, body) => {
                let head = self.b.create_block();
                let bb = self.b.create_block();
                let exit = self.b.create_block();
                self.b.jump(head, &[]);
                self.b.switch_to_block(head);
                let (cv, _) = self.expr(c, Some(Ty::Bool))?;
                let cv = cv.ok_or("bad condition")?;
                self.b.brif(cv, bb, &[], exit, &[]);
                self.b.switch_to_block(bb);
                self.loops.push(Loop { cont: head, brk: exit });
                let (_, t) = self.block(body, Some(Ty::Unit))?;
                self.loops.pop();
                if t != Ty::Never {
                    self.b.jump(head, &[]);
                }
                self.b.switch_to_block(exit);
                Ok((None, Ty::Unit))
            }
            Expr::Loop(_, body) => {
                let head = self.b.create_block();
                let exit = self.b.create_block();
                self.b.jump(head, &[]);
                self.b.switch_to_block(head);
                self.loops.push(Loop { cont: head, brk: exit });
                let (_, t) = self.block(body, Some(Ty::Unit))?;
                self.loops.pop();
                if t != Ty::Never {
                    self.b.jump(head, &[]);
                }
                self.b.switch_to_block(exit);
                Ok((None, Ty::Unit))
            }
            Expr::Break(None, None) => {
                let l = self.loops.last().ok_or("`break` outside of a loop")?;
                let brk = l.brk;
                self.b.jump(brk, &[]);
                Ok((None, Ty::Never))
            }
            Expr::Continue(None) => {
                let l = self.loops.last().ok_or("`continue` outside of a loop")?;
                let c = l.cont;
                self.b.jump(c, &[]);
                Ok((None, Ty::Never))
            }
            Expr::Return(x) => {
                let ret = self.ret;
                let v = match x {
                    Some(x) => self.expr(x, Some(ret))?.0,
                    None => None,
                };
                match v {
                    Some(v) => self.b.return_(&[v]),
                    None => self.b.return_(&[]),
                }
                Ok((None, Ty::Never))
            }
            Expr::Call(f, args) => self.call(f, args),
            Expr::Tuple(xs) if xs.is_empty() => Ok((None, Ty::Unit)),
            _ => self.err(format!("unsupported expression: {}", short(e))),
        }
    }
}

fn short(e: &Expr) -> String {
    let s = format!("{e:?}");
    s.chars().take(60).collect()
}

/// 1 関数の機械語 (名前, 機械語, 再配置, 外から見えるか)
pub fn compile_fn(prog: &Program, id: u32, psig: &crate::parse::FnSig, body: &Block) -> R<(String, Vec<u8>, Vec<crate::minclif::Reloc>, bool)> {
    let sig: &FnSig = &prog.sigs[id as usize];
    let env = TyEnv { self_sid: sig.impl_of, params: vec![], self_ty: sig.self_ty.filter(|t| !prog.has_param(*t)) };
    let lets = crate::ast_infer::infer(prog, psig.recv.is_some(), &psig.params, body, sig, &env);
    let env = env.clone();
    let ps: Vec<Type> = sig.params.iter().filter_map(|&t| clif_ty(t)).collect();
    let rets: Vec<Type> = clif_ty(sig.ret).into_iter().collect();
    let b = FunctionBuilder::new(ps, rets);
    let mut cx = Cx { b, prog, lets, scopes: vec![vec![]], loops: vec![], ret: sig.ret, env };
    let pvals = cx.b.func_params();
    let mut k = 0;
    for ((p, _), &t) in psig.params.iter().zip(&sig.params) {
        let v = if clif_ty(t).is_some() {
            k += 1;
            Some(pvals[k - 1])
        } else {
            None
        };
        match p {
            Pat::Ident(_, _, n, None) => cx.bind(n, t, v)?,
            Pat::Wild => {}
            _ => return Err("unsupported parameter pattern".to_string()),
        }
    }
    let (v, t) = cx.block(body, Some(sig.ret))?;
    if t != Ty::Never {
        match (v, clif_ty(sig.ret)) {
            (Some(v), Some(_)) => cx.b.return_(&[v]),
            _ => cx.b.return_(&[]),
        }
    }
    let (code, relocs) = cx.b.finish();
    let global = sig.is_main || sig.export;
    let name = if sig.is_main { "main".to_string() } else { sig.symbol.clone() };
    Ok((name, code, relocs, global))
}

/// ソースから ELF の .o を作る (syn・Cranelift を使わない経路)。いまは prelude を付けない no_std のプログラムだけ
pub fn build_object(src0: &str) -> R<Vec<u8>> {
    let (src, items, user) = with_prelude(src0)?;
    let mut sums: Vec<Summary> = vec![Summary::default(); items.len()];
    for pass in 0..2 {
        for (i, it) in items.iter().enumerate() {
            let header = matches!(it.kind, LexKind::ImplHeader | LexKind::TraitHeader);
            if header != (pass == 0) {
                continue;
            }
            let ow = if pass == 1 { crate::ast_summary::owner_of(it, &sums) } else { None };
            sums[i] = crate::ast_summary::parse_and_summarize(&src, it, ow.as_ref())?;
        }
    }
    let prog = Program::build(&items, &sums)?;
    let mut funcs = vec![];
    for id in 0..prog.names.len() {
        let sig = &prog.sigs[id];
        let Some(ii) = prog.item_of[id] else { continue };
        // (いまはユーザの関数だけ: prelude の関数はまだ扱えない)
        if ii as usize >= user {
            continue;
        }
        if sig.is_generic() || !sig.has_body || sig.is_extern || prog.closures.contains_key(&(id as u32)) {
            continue;
        }
        let it = &items[ii as usize];
        let f = crate::parse::parse_file(&src[it.start..it.end])?;
        let Some(ItemKind::Fn(psig, Some(body))) = f.items.first().map(|i| &i.kind) else { continue };
        let r = compile_fn(&prog, id as u32, psig, body).map_err(|e| format!("in `{}`: {e}", prog.names[id]))?;
        funcs.push(r);
    }
    Ok(crate::minclif::write_object(&funcs))
}

/// driver.rs の with_prelude と同じ手順 (マクロの展開はまだしない): ユーザのアイテムの後ろに prelude のアイテムを付ける
pub fn with_prelude(src0: &str) -> R<(String, Vec<SrcItem>, usize)> {
    let s1 = crate::lexer::flatten_modules(src0).unwrap_or(src0.to_string());
    let s2 = crate::lexer::hoist_nested_items(&s1).unwrap_or(s1);
    let src = crate::ctors::rewrite_ctor_values(&s2).unwrap_or(s2);
    let mut items = crate::lexer::split(&src)?;
    let user = items.len();
    let extra = crate::prelude::select(&src, &items);
    if extra.is_empty() {
        return Ok((src, items, user));
    }
    let base = src.len() + 1;
    let lines = src.bytes().filter(|&c| c == b'\n').count() + 1;
    let n0 = items.len() as u32;
    for mut it in crate::lexer::split(&extra)? {
        it.start += base;
        it.end += base;
        it.line += lines;
        for r in &mut it.idents {
            r.0 += base as u32;
            r.1 += base as u32;
        }
        it.kind = match it.kind {
            LexKind::ImplMember(h) => LexKind::ImplMember(h + n0),
            LexKind::TraitMember(h) => LexKind::TraitMember(h + n0),
            k => k,
        };
        items.push(it);
    }
    Ok((format!("{src}\n{extra}"), items, user))
}
