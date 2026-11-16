//! 自前の構文木 (src/parse.rs) から minclif (src/minclif.rs) で機械語を作るコード生成。
//! syn と Cranelift に依存しないコンパイラの経路 (parse → ast_summary → program → ast_infer → ここ → minclif → ELF) の最後の段。
//! codegen.rs (syn + Cranelift) を置き換えるために、扱える言語の範囲を少しずつ広げていく。
//!
//! 値の表し方: 整数・bool・char・浮動小数点・ポインタは機械の値、struct / enum / 配列 / タプル / fat pointer (`&[T]` / `&str`)
//! はメモリに置いてアドレスで扱う。ローカル変数はすべてスタックスロットに置く (参照を取れるように)。
//! 関数の ABI: 集約型の引数はアドレスで渡し (呼ぶ側が複製を作る)、集約型の戻り値は隠れた第 1 引数の場所に書く。
//!
//! 扱えるもの: 整数・bool・char・キャスト、let / 代入 / 複合代入、算術・ビット演算・比較・`&&` / `||`、
//! if / if let / while / while let / loop / break / continue / return / match、struct / タプル / 配列 / enum の値とパターン、
//! 参照・`*p`・フィールド・添字 (範囲の検査つき)・`&a[i..j]`、文字列とバイト列のリテラル、`len()` / `as_bytes()`、
//! 範囲・配列・スライスの for、ユーザの関数・メソッド (ジェネリックでないもの) と extern "C" の関数の呼び出し。
//! 外部クレートに依存しないので crustc 自身でもコンパイルできる。tests/ast_codegen.rs が rustc と出力を比べる。

use crate::ast_infer::LetTypes;
use crate::lexer::{ItemKind as LexKind, SrcItem};
use crate::minclif::{Block as CBlock, DataDef, FunctionBuilder, IntCC, Reloc, StackSlot, Type, Value};
use crate::parse::{Arm, Block, Expr, ItemKind, Pat, Path, Stmt};
use crate::program::{FnSig, PPath, Program, TyEnv};
use crate::types::{Summary, Ty, VariantKind, intern};

pub type R<T> = Result<T, String>;

/// Ty → 機械の型 (値を持たない型・集約型は None)
pub fn clif_ty(t: Ty) -> Option<Type> {
    if t.is_agg() {
        return None;
    }
    Some(match t {
        Ty::I8 | Ty::U8 | Ty::Bool => Type::I8,
        Ty::I16 | Ty::U16 => Type::I16,
        Ty::I32 | Ty::U32 | Ty::Char => Type::I32,
        Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize | Ty::Ptr(..) | Ty::Ref(..) | Ty::Fn(_) | Ty::Box(_) => Type::I64,
        Ty::I128 | Ty::U128 => Type::I128,
        Ty::F32 => Type::F32,
        Ty::F64 => Type::F64,
        _ => return None,
    })
}

/// 引数・戻り値の機械の型 (集約型はアドレス)
fn abi_ty(t: Ty) -> Option<Type> {
    if t.is_agg() { Some(Type::I64) } else { clif_ty(t) }
}

/// 式の値と型 (集約型ならアドレス)
type V = (Option<Value>, Ty);

#[derive(Clone, Copy)]
struct Place {
    addr: Value,
    off: i32,
    ty: Ty,
}

struct Loop {
    cont: CBlock,
    brk: CBlock,
}

struct Cx<'a> {
    b: FunctionBuilder,
    prog: &'a Program,
    lets: LetTypes,
    scopes: Vec<Vec<(String, StackSlot, Ty)>>,
    loops: Vec<Loop>,
    ret: Ty,
    env: TyEnv,
    /// 集約型の戻り値を書く場所
    sret: Option<Value>,
    /// この関数が作ったデータ (文字列リテラルなど)
    datas: Vec<DataDef>,
    name: String,
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
        if t.starts_with("b'") {
            return bytes.first().map(|&b| b as i64);
        }
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

fn short(e: &Expr) -> String {
    let s = format!("{e:?}");
    s.chars().take(80).collect()
}

impl Cx<'_> {
    fn err<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(msg.into())
    }

    /// 書かれた型
    fn parse_ty(&self, t: &crate::parse::Ty) -> Option<Ty> {
        let ts = crate::ast_types::tysyn(t, Some(&crate::types::TySyn::Named("Self".into()))).ok()?;
        self.prog.resolve_env(&ts, &self.env).ok()
    }

    /// 推論が記録した式の型
    fn peek(&self, e: &Expr) -> Option<Ty> {
        self.lets.get(&(e as *const Expr as usize)).copied()
    }

    fn size_of(&self, t: Ty) -> u32 {
        self.prog.size_align(t).0
    }

    fn lookup(&self, n: &str) -> Option<(StackSlot, Ty)> {
        self.scopes.iter().rev().flat_map(|s| s.iter().rev()).find(|x| x.0 == n).map(|x| (x.1, x.2))
    }

    fn temp(&mut self, t: Ty) -> Value {
        let (s, a) = self.prog.size_align(t);
        let ss = self.b.create_stack_slot(s.max(1), a.max(1));
        self.b.stack_addr(ss, 0)
    }

    fn addr_plus(&mut self, a: Value, off: i32) -> Value {
        if off == 0 { a } else { self.b.iadd_imm(a, off as i64) }
    }

    /// size byte を src から dst へ
    fn memcpy(&mut self, dst: Value, src: Value, size: u32) {
        let mut o = 0u32;
        while o < size {
            let (t, n) = if size - o >= 8 {
                (Type::I64, 8)
            } else if size - o >= 4 {
                (Type::I32, 4)
            } else if size - o >= 2 {
                (Type::I16, 2)
            } else {
                (Type::I8, 1)
            };
            let v = self.b.load(t, src, o as i32);
            self.b.store(v, dst, o as i32);
            o += n;
        }
    }

    /// 場所を読む (集約型はアドレス)
    fn read(&mut self, p: Place) -> V {
        if p.ty.is_agg() {
            return (Some(self.addr_plus(p.addr, p.off)), p.ty);
        }
        match clif_ty(p.ty) {
            Some(ct) => (Some(self.b.load(ct, p.addr, p.off)), p.ty),
            None => (None, p.ty),
        }
    }

    /// 場所に書く (集約型は複製)
    fn write(&mut self, p: Place, v: Option<Value>) {
        let Some(v) = v else { return };
        if p.ty.is_agg() {
            let dst = self.addr_plus(p.addr, p.off);
            let n = self.size_of(p.ty);
            self.memcpy(dst, v, n);
        } else if clif_ty(p.ty).is_some() {
            self.b.store(v, p.addr, p.off);
        }
    }

    /// 新しいローカル変数 (値を複製して入れる)
    fn bind(&mut self, n: &str, t: Ty, v: Option<Value>) {
        let (s, a) = self.prog.size_align(t);
        let ss = self.b.create_stack_slot(s.max(1), a.max(1));
        let addr = self.b.stack_addr(ss, 0);
        self.write(Place { addr, off: 0, ty: t }, v);
        self.scopes.last_mut().unwrap().push((n.to_string(), ss, t));
    }

    /// 発散した後のコード用のブロック (どこからも飛ばない)
    fn dead_block(&mut self) {
        let d = self.b.create_block();
        self.b.switch_to_block(d);
    }

    /// 範囲外の添字など: いまは止めるだけ
    fn trap_unless(&mut self, ok: Value) {
        self.b.trapz(ok);
    }

    // --- ブロック ------------------------------------------------------

    fn block(&mut self, blk: &Block, expected: Option<Ty>) -> R<V> {
        self.scopes.push(vec![]);
        let n = blk.stmts.len();
        let mut res: V = (None, Ty::Unit);
        for (i, st) in blk.stmts.iter().enumerate() {
            match st {
                Stmt::Let(pat, ann, init, els) => {
                    let t = match ann.as_ref().and_then(|a| self.parse_ty(a)) {
                        Some(t) => t,
                        None => match self.lets.get(&crate::ast_infer::stmt_key(st)).copied() {
                            Some(t) => t,
                            None => return self.err(format!("cannot infer the type of `let {}`", short_pat(pat))),
                        },
                    };
                    let Some(e) = init else {
                        // (`let x;`: 後で代入する)
                        if let Pat::Ident(_, _, name, None) = pat {
                            self.bind(name, t, None);
                            continue;
                        }
                        return self.err("`let` without an initializer must bind a name");
                    };
                    let (v, et) = self.expr(e, Some(t))?;
                    if et == Ty::Never {
                        self.dead_block();
                        continue;
                    }
                    let v = self.coerce(v, et, t);
                    match (pat, els) {
                        (Pat::Ident(_, _, name, None), None) => self.bind(name, t, v),
                        (p, els) => {
                            // パターンの let (`let (a, b) = ..` / `let Some(x) = .. else { .. }`)
                            let tmp = self.temp(t);
                            self.write(Place { addr: tmp, off: 0, ty: t }, v);
                            let place = Place { addr: tmp, off: 0, ty: t };
                            let fail = self.b.create_block();
                            let ok = self.b.create_block();
                            self.pat_test(p, place, fail)?;
                            self.b.jump(ok, &[]);
                            self.b.switch_to_block(fail);
                            match els {
                                Some(eb) => {
                                    let (_, et) = self.block(eb, Some(Ty::Never))?;
                                    if et != Ty::Never {
                                        return self.err("the `else` of `let ... else` must diverge");
                                    }
                                }
                                None => self.b.trap(),
                            }
                            self.b.switch_to_block(ok);
                            self.pat_bind(p, place, false)?;
                        }
                    }
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

    /// `&[T; N]` → `&[T]` などの強制変換
    fn coerce(&mut self, v: Option<Value>, from: Ty, to: Ty) -> Option<Value> {
        match (from, to) {
            (Ty::Ref(_, Ty::Array(_, n)), Ty::Ref(_, Ty::Slice(_))) => {
                let p = v?;
                Some(self.make_fat(p, *n as i64))
            }
            _ => v,
        }
    }

    /// (ポインタ, 長さ) の fat pointer を一時領域に作る
    fn make_fat(&mut self, p: Value, len: i64) -> Value {
        let l = self.b.iconst(Type::I64, len);
        self.make_fat_v(p, l)
    }

    fn make_fat_v(&mut self, p: Value, len: Value) -> Value {
        let ss = self.b.create_stack_slot(16, 8);
        let a = self.b.stack_addr(ss, 0);
        self.b.store(p, a, 0);
        self.b.store(len, a, 8);
        a
    }

    // --- 演算 ----------------------------------------------------------

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

    fn arith(&mut self, op: &str, l: Value, r: Value, t: Ty) -> R<Value> {
        let s = t.signed();
        Ok(match op {
            "+" => self.b.iadd(l, r),
            "-" => self.b.isub(l, r),
            "*" => self.b.imul(l, r),
            "/" | "%" => {
                // (0 除算は止める)
                let z = self.b.icmp_imm(IntCC::NotEqual, r, 0);
                self.trap_unless(z);
                match (op, s) {
                    ("/", true) => self.b.sdiv(l, r),
                    ("/", false) => self.b.udiv(l, r),
                    (_, true) => self.b.srem(l, r),
                    _ => self.b.urem(l, r),
                }
            }
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
        let (l, mut lt) = self.expr(le, hint)?;
        let rhint = if matches!(op, "<<" | ">>") { None } else { Some(lt) };
        let (r, mut rt) = self.expr(re, rhint)?;
        let (Some(mut l), Some(mut r)) = (l, r) else { return self.err("bad operand") };
        // (`&i64 + i64` / `*x == y` の参照は値として計算する)
        while let Ty::Ref(_, i) = lt {
            if i.is_agg() || lt.is_fat() {
                break;
            }
            l = self.b.load(clif_ty(*i).ok_or("bad operand")?, l, 0);
            lt = *i;
        }
        while let Ty::Ref(_, i) = rt {
            if i.is_agg() || rt.is_fat() {
                break;
            }
            r = self.b.load(clif_ty(*i).ok_or("bad operand")?, r, 0);
            rt = *i;
        }
        if cmp {
            if lt.is_agg() {
                return self.agg_eq(op, l, r, lt);
            }
            let cc = Self::int_cc(op, lt.signed()).ok_or("bad comparison")?;
            return Ok((Some(self.b.icmp(cc, l, r)), Ty::Bool));
        }
        let r = if rt != lt { self.convert(r, rt, lt) } else { r };
        Ok((Some(self.arith(op, l, r, lt)?), lt))
    }

    /// `&str` / `&[u8]` などの == (長さと中身)
    fn agg_eq(&mut self, op: &str, l: Value, r: Value, t: Ty) -> R<V> {
        let elem = match t {
            Ty::Ref(_, Ty::Str) => Ty::U8,
            Ty::Ref(_, Ty::Slice(e)) if clif_ty(**e).is_some() => **e,
            _ => return self.err(format!("unsupported comparison of {t:?}")),
        };
        let es = self.size_of(elem) as i64;
        let ct = clif_ty(elem).unwrap();
        let (pa, la) = (self.b.load(Type::I64, l, 0), self.b.load(Type::I64, l, 8));
        let (pb, lb) = (self.b.load(Type::I64, r, 0), self.b.load(Type::I64, r, 8));
        let head = self.b.create_block();
        let body = self.b.create_block();
        let done = self.b.create_block();
        let res = self.b.append_block_param(done, Type::I8);
        let i = self.b.append_block_param(head, Type::I64);
        let same = self.b.icmp(IntCC::Equal, la, lb);
        let z = self.b.iconst(Type::I64, 0);
        let f = self.b.iconst(Type::I8, 0);
        self.b.brif(same, head, &[z], done, &[f]);
        self.b.switch_to_block(head);
        let more = self.b.icmp(IntCC::UnsignedLessThan, i, la);
        let t1 = self.b.iconst(Type::I8, 1);
        self.b.brif(more, body, &[], done, &[t1]);
        self.b.switch_to_block(body);
        let k = self.b.iconst(Type::I64, es);
        let off = self.b.imul(i, k);
        let xa = self.b.iadd(pa, off);
        let xb = self.b.iadd(pb, off);
        let x = self.b.load(ct, xa, 0);
        let y = self.b.load(ct, xb, 0);
        let eq = self.b.icmp(IntCC::Equal, x, y);
        let i2 = self.b.iadd_imm(i, 1);
        let f2 = self.b.iconst(Type::I8, 0);
        self.b.brif(eq, head, &[i2], done, &[f2]);
        self.b.switch_to_block(done);
        let r = if op == "!=" {
            let one = self.b.iconst(Type::I8, 1);
            self.b.bxor(res, one)
        } else {
            res
        };
        Ok((Some(r), Ty::Bool))
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

    // --- 呼び出し --------------------------------------------------------

    /// 関数 id を呼ぶ (recv はメソッドの受け手の値)
    fn call_id(&mut self, id: u32, recv: Option<V>, args: &[Expr]) -> R<V> {
        let sig = self.prog.sigs[id as usize].clone();
        if sig.is_generic() {
            return self.err(format!("generic functions are not supported yet (`{}`)", self.prog.names[id as usize]));
        }
        let mut vals = vec![];
        // (集約型の戻り値の場所)
        let sret = if sig.ret.is_agg() { Some(self.temp(sig.ret)) } else { None };
        if let Some(s) = sret {
            vals.push(s);
        }
        let mut params = sig.params.iter().copied();
        if let Some((rv, rt)) = recv {
            let pt = params.next().ok_or("method without self")?;
            vals.extend(self.pass(rv, rt, pt));
        }
        for (a, pt) in args.iter().zip(params) {
            let (v, t) = self.expr(a, Some(pt))?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            let v = self.coerce(v, t, pt);
            vals.extend(self.pass(v, t, pt));
        }
        let fr = self.b.import_function(&sig.symbol);
        let rets: Vec<Type> = if sret.is_some() { vec![] } else { clif_ty(sig.ret).into_iter().collect() };
        let r = self.b.call(fr, &vals, &rets);
        if sig.ret == Ty::Never {
            self.b.trap();
            return Ok((None, Ty::Never));
        }
        if let Some(s) = sret {
            return Ok((Some(s), sig.ret));
        }
        Ok((r.first().copied(), sig.ret))
    }

    /// 引数の値 (集約型は複製のアドレス)
    fn pass(&mut self, v: Option<Value>, t: Ty, pt: Ty) -> Option<Value> {
        let v = v?;
        if pt.is_agg() {
            let tmp = self.temp(pt);
            let n = self.size_of(pt);
            self.memcpy(tmp, v, n);
            let _ = t;
            return Some(tmp);
        }
        Some(v)
    }

    fn call(&mut self, f: &Expr, args: &[Expr], expected: Option<Ty>) -> R<V> {
        let Expr::Path(None, p) = f else { return self.err("unsupported call") };
        // バリアント / タプル struct の値
        if let Some((sid, vi)) = self.prog.variant_by_p(&PPath::of(p), None) {
            return self.variant_value(sid, vi, args, expected);
        }
        let Some(id) = self.prog.fn_by_p(&PPath::of(p), None) else {
            if let Some(sid) = self.prog.struct_by_p(&PPath::of(p), None) {
                return self.tuple_struct(sid, args);
            }
            return self.err(format!("cannot find function `{}`", p.segs.last().map(|s| s.name.as_str()).unwrap_or("")));
        };
        self.call_id(id, None, args)
    }

    fn method_call(&mut self, recv: &Expr, name: &str, args: &[Expr]) -> R<V> {
        let rt = self.peek(recv).or_else(|| self.static_ty(recv)).ok_or_else(|| format!("cannot infer the receiver of `.{name}()`"))?;
        let mut base = rt;
        while let Ty::Ref(_, i) = base {
            base = *i;
        }
        // 組み込み: 配列 / スライス / &str の len、&str の as_bytes
        match (name, base, rt) {
            ("len", Ty::Array(_, n), _) if args.is_empty() => {
                return Ok((Some(self.b.iconst(Type::I64, n as i64)), Ty::Usize));
            }
            ("len", _, t) | ("as_bytes", _, t) if args.is_empty() && t.is_fat() => {
                let (v, _) = self.expr(recv, None)?;
                let v = v.ok_or("bad receiver")?;
                if name == "len" {
                    return Ok((Some(self.b.load(Type::I64, v, 8)), Ty::Usize));
                }
                return Ok((Some(v), Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8))))));
            }
            ("as_ptr", _, t) | ("as_mut_ptr", _, t) if args.is_empty() && t.is_fat() => {
                let (v, _) = self.expr(recv, None)?;
                let p = self.b.load(Type::I64, v.ok_or("bad receiver")?, 0);
                let el = match base {
                    Ty::Slice(e) => *e,
                    _ => Ty::U8,
                };
                return Ok((Some(p), Ty::Ptr(name == "as_mut_ptr", intern(el))));
            }
            ("as_ptr", Ty::Array(e, _), _) | ("as_mut_ptr", Ty::Array(e, _), _) if args.is_empty() => {
                let p = self.place_or_temp(recv)?;
                let p = self.deref_place(p);
                let a = self.addr_plus(p.addr, p.off);
                return Ok((Some(a), Ty::Ptr(name == "as_mut_ptr", e)));
            }
            ("is_empty", _, t) if args.is_empty() && t.is_fat() => {
                let (v, _) = self.expr(recv, None)?;
                let l = self.b.load(Type::I64, v.ok_or("bad receiver")?, 8);
                return Ok((Some(self.b.icmp_imm(IntCC::Equal, l, 0)), Ty::Bool));
            }
            _ => {}
        }
        let Ty::Struct(sid) = base else { return self.err(format!("unsupported method `.{name}()` on {base:?}")) };
        let Some(id) = self.prog.method(sid, name) else { return self.err(format!("no method `{name}`")) };
        let pt = self.prog.sigs[id as usize].params.first().copied().ok_or("not a method")?;
        // 受け手: `&self` なら場所のアドレス、`self` なら値
        let rv: V = match pt {
            Ty::Ref(..) if !matches!(rt, Ty::Ref(..)) => {
                let p = self.place(recv)?;
                (Some(self.addr_plus(p.addr, p.off)), Ty::Ref(false, intern(base)))
            }
            Ty::Ref(..) => {
                let mut v = self.expr(recv, None)?;
                // (`&&S` → `&S`)
                while let Ty::Ref(_, Ty::Ref(_, i)) = v.1 {
                    let x = self.b.load(Type::I64, v.0.ok_or("bad receiver")?, 0);
                    v = (Some(x), Ty::Ref(false, i));
                }
                v
            }
            _ => {
                let p = self.place_or_temp(recv)?;
                let mut p = p;
                while let Ty::Ref(_, i) = p.ty {
                    let a = self.b.load(Type::I64, p.addr, p.off);
                    p = Place { addr: a, off: 0, ty: *i };
                }
                self.read(p)
            }
        };
        self.call_id(id, Some(rv), args)
    }

    // --- 構築 ----------------------------------------------------------

    fn variant_value(&mut self, sid: u32, vi: usize, args: &[Expr], expected: Option<Ty>) -> R<V> {
        let sid = match expected {
            Some(Ty::Struct(e)) if self.prog.base_of(e) == self.prog.base_of(sid) => e,
            _ => sid,
        };
        let sd = &self.prog.structs[sid as usize];
        if !sd.generics.is_empty() && sd.base.is_none() {
            return self.err("generic enums are not supported yet");
        }
        let (tag, discr) = (sd.tag, sd.variants[vi].discr);
        let fields = sd.variants[vi].fields.clone();
        let t = Ty::Struct(sid);
        let a = self.temp(t);
        let tv = self.b.iconst(clif_ty(tag).unwrap_or(Type::I8), discr);
        self.b.store(tv, a, 0);
        for ((_, ft, off), x) in fields.iter().zip(args) {
            let (v, vt) = self.expr(x, Some(*ft))?;
            let v = self.coerce(v, vt, *ft);
            self.write(Place { addr: a, off: *off as i32, ty: *ft }, v);
        }
        Ok((Some(a), t))
    }

    fn tuple_struct(&mut self, sid: u32, args: &[Expr]) -> R<V> {
        let fields = self.prog.structs[sid as usize].fields.clone();
        let t = Ty::Struct(sid);
        let a = self.temp(t);
        for ((_, ft, off), x) in fields.iter().zip(args) {
            let (v, vt) = self.expr(x, Some(*ft))?;
            let v = self.coerce(v, vt, *ft);
            self.write(Place { addr: a, off: *off as i32, ty: *ft }, v);
        }
        Ok((Some(a), t))
    }

    fn struct_lit(&mut self, p: &Path, fs: &[(String, Expr)], e: &Expr) -> R<V> {
        if let Some((sid, vi)) = self.prog.variant_by_p(&PPath::of(p), None) {
            // (`E::V { a, b }`)
            let fields = self.prog.structs[sid as usize].variants[vi].fields.clone();
            let (tag, discr) = (self.prog.structs[sid as usize].tag, self.prog.structs[sid as usize].variants[vi].discr);
            let t = Ty::Struct(sid);
            let a = self.temp(t);
            let tv = self.b.iconst(clif_ty(tag).unwrap_or(Type::I8), discr);
            self.b.store(tv, a, 0);
            for (n, x) in fs {
                let (_, ft, off) = fields.iter().find(|f| &f.0 == n).cloned().ok_or_else(|| format!("no field `{n}`"))?;
                let (v, vt) = self.expr(x, Some(ft))?;
                let v = self.coerce(v, vt, ft);
                self.write(Place { addr: a, off: off as i32, ty: ft }, v);
            }
            return Ok((Some(a), t));
        }
        let t = self.peek(e).or_else(|| self.prog.struct_by_p(&PPath::of(p), None).map(Ty::Struct)).ok_or("cannot find struct")?;
        let Ty::Struct(sid) = t else { return self.err("not a struct") };
        let fields = self.prog.structs[sid as usize].fields.clone();
        let a = self.temp(t);
        for (n, x) in fs {
            let (_, ft, off) = fields.iter().find(|f| &f.0 == n).cloned().ok_or_else(|| format!("no field `{n}`"))?;
            let (v, vt) = self.expr(x, Some(ft))?;
            let v = self.coerce(v, vt, ft);
            self.write(Place { addr: a, off: off as i32, ty: ft }, v);
        }
        Ok((Some(a), t))
    }

    // --- 場所 ----------------------------------------------------------

    /// 推論の記録がないときの、変数・フィールドの型
    fn static_ty(&self, e: &Expr) -> Option<Ty> {
        match e {
            Expr::Path(None, p) => self.lookup(ident(p)?).map(|x| x.1),
            Expr::Paren(x) => self.static_ty(x),
            Expr::Field(b, n) => {
                let mut t = self.static_ty(b)?;
                while let Ty::Ref(_, i) = t {
                    t = *i;
                }
                let Ty::Struct(sid) = t else { return None };
                self.prog.field(sid, n).map(|f| f.0)
            }
            _ => None,
        }
    }

    fn is_place(e: &Expr) -> bool {
        match e {
            Expr::Path(None, p) => ident(p).is_some(),
            Expr::Field(..) => true,
            Expr::Index(_, i) => !matches!(&**i, Expr::Range(..)),
            Expr::Unary(op, _) => op == "*",
            Expr::Paren(x) => Self::is_place(x),
            _ => false,
        }
    }

    /// 場所の式なら場所、でなければ値を一時領域に置いた場所
    fn place_or_temp(&mut self, e: &Expr) -> R<Place> {
        if Self::is_place(e) && !matches!(e, Expr::Path(None, p) if self.lookup(ident(p).unwrap_or("")).is_none()) {
            return self.place(e);
        }
        let (v, t) = self.expr(e, None)?;
        if t.is_agg() {
            return Ok(Place { addr: v.ok_or("no value")?, off: 0, ty: t });
        }
        let a = self.temp(t);
        let p = Place { addr: a, off: 0, ty: t };
        self.write(p, v);
        Ok(p)
    }

    /// 参照・ポインタを辿った場所
    fn deref_place(&mut self, mut p: Place) -> Place {
        while let Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) = p.ty {
            if p.ty.is_fat() {
                break;
            }
            let a = self.b.load(Type::I64, p.addr, p.off);
            p = Place { addr: a, off: 0, ty: *i };
        }
        p
    }

    fn place(&mut self, e: &Expr) -> R<Place> {
        match e {
            Expr::Paren(x) => self.place(x),
            Expr::Path(None, p) => {
                let n = ident(p).ok_or("unsupported place")?;
                let (ss, t) = self.lookup(n).ok_or_else(|| format!("cannot find `{n}`"))?;
                let a = self.b.stack_addr(ss, 0);
                Ok(Place { addr: a, off: 0, ty: t })
            }
            Expr::Field(base, name) => {
                let p = self.place_or_temp(base)?;
                let p = self.deref_place(p);
                let Ty::Struct(sid) = p.ty else { return self.err(format!("no field `{name}` on {:?}", p.ty)) };
                let (ft, off) = self.prog.field(sid, name).ok_or_else(|| format!("no field `{name}`"))?;
                Ok(Place { addr: p.addr, off: p.off + off as i32, ty: ft })
            }
            Expr::Index(base, ix) => {
                let p = self.place_or_temp(base)?;
                let p = self.deref_place(p);
                let (ptr, len, elem) = match p.ty {
                    Ty::Array(e, n) => {
                        let a = self.addr_plus(p.addr, p.off);
                        let l = self.b.iconst(Type::I64, n as i64);
                        (a, l, *e)
                    }
                    Ty::Ref(_, Ty::Slice(e)) => {
                        let a = self.b.load(Type::I64, p.addr, p.off);
                        let l = self.b.load(Type::I64, p.addr, p.off + 8);
                        (a, l, **e)
                    }
                    t => return self.err(format!("cannot index into {t:?}")),
                };
                let (i, it) = self.expr(ix, Some(Ty::Usize))?;
                let i = i.ok_or("bad index")?;
                let i = self.convert(i, it, Ty::Usize);
                let ok = self.b.icmp(IntCC::UnsignedLessThan, i, len);
                self.trap_unless(ok);
                let es = self.b.iconst(Type::I64, self.size_of(elem) as i64);
                let off = self.b.imul(i, es);
                let a = self.b.iadd(ptr, off);
                Ok(Place { addr: a, off: 0, ty: elem })
            }
            Expr::Unary(op, x) if op == "*" => {
                let (v, t) = self.expr(x, None)?;
                let v = v.ok_or("bad deref")?;
                match t {
                    Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) if !t.is_fat() => Ok(Place { addr: v, off: 0, ty: *i }),
                    _ => self.err(format!("cannot dereference {t:?}")),
                }
            }
            _ => {
                let (v, t) = self.expr(e, None)?;
                if t.is_agg() {
                    return Ok(Place { addr: v.ok_or("no value")?, off: 0, ty: t });
                }
                let a = self.temp(t);
                let p = Place { addr: a, off: 0, ty: t };
                self.write(p, v);
                Ok(p)
            }
        }
    }

    /// 文字列リテラルのデータ (`&str` の fat pointer)
    fn str_data(&mut self, bytes: Vec<u8>) -> Value {
        let n = bytes.len() as i64;
        let name = format!("{}.s{}", self.name, self.datas.len());
        self.datas.push((name.clone(), bytes, vec![], false));
        let p = self.b.symbol_addr(&name);
        self.make_fat(p, n)
    }

    // --- 式 ------------------------------------------------------------

    fn expr(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        self.expr_(e, expected).map_err(|m| if m.contains(" (at ") { m } else { format!("{m} (at {})", short(e)) })
    }

    fn expr_(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        match e {
            Expr::Lit(t) if t.starts_with('"') || t.starts_with("r\"") || t.starts_with("r#") => {
                let (_, bytes) = crate::ast_consteval::str_lit(t).ok_or("bad string literal")?;
                let v = self.str_data(bytes);
                Ok((Some(v), Ty::Ref(false, intern(Ty::Str))))
            }
            Expr::Lit(t) if t.starts_with("b\"") || t.starts_with("br") => {
                let (_, bytes) = crate::ast_consteval::str_lit(t).ok_or("bad byte string literal")?;
                let n = bytes.len() as u32;
                let fat = self.str_data(bytes);
                let p = self.b.load(Type::I64, fat, 0);
                Ok((Some(p), Ty::Ref(false, intern(Ty::Array(intern(Ty::U8), n)))))
            }
            Expr::Lit(t) => {
                let ty = self.peek(e).or(expected).filter(|t| clif_ty(*t).is_some()).unwrap_or(if t == "true" || t == "false" {
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
            Expr::Path(None, p) if ident(p).is_some_and(|n| self.lookup(n).is_some()) => {
                let pl = self.place(e)?;
                Ok(self.read(pl))
            }
            Expr::Path(None, p) => {
                // unit バリアント / unit struct / const
                if let Some((sid, vi)) = self.prog.variant_by_p(&PPath::of(p), None) {
                    return self.variant_value(sid, vi, &[], expected);
                }
                let pp = PPath::of(p);
                let n = p.segs.last().map(|s| s.name.clone()).unwrap_or_default();
                let key = if p.segs.len() == 2 { format!("{}::{}", p.segs[0].name, p.segs[1].name) } else { n.clone() };
                if let Some(&ci) = self.prog.const_index.get(&key).or_else(|| self.prog.const_index.get(&n)) {
                    let c = &self.prog.consts[ci as usize];
                    if c.stat.is_none() && clif_ty(c.ty).is_some() {
                        let t = c.ty;
                        let x = self.prog.const_int(&key, 0).or_else(|_| self.prog.const_int(&n, 0))?;
                        return Ok((Some(self.b.iconst(clif_ty(t).unwrap(), x as i64)), t));
                    }
                }
                // (`u8::MAX` など)
                if p.segs.len() == 2 {
                    if let Some(t) = Ty::prim(&p.segs[0].name) {
                        if let Some(x) = crate::program::prim_assoc_const(t, &p.segs[1].name) {
                            let t = if p.segs[1].name == "BITS" { Ty::U32 } else { t };
                            return Ok((Some(self.b.iconst(clif_ty(t).unwrap(), x as i64)), t));
                        }
                    }
                }
                if let Some(sid) = self.prog.struct_by_p(&pp, None) {
                    let t = Ty::Struct(sid);
                    return Ok((Some(self.temp(t)), t));
                }
                self.err(format!("cannot find value `{n}`"))
            }
            Expr::Unary(op, x) if op == "*" => {
                let pl = self.place(e)?;
                let _ = x;
                Ok(self.read(pl))
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
            Expr::Ref(m, x) => {
                // `&a[i..j]` / `&a[..]`
                if let Expr::Index(base, ix) = &**x {
                    if let Expr::Range(lo, hi, incl) = &**ix {
                        return self.slice_range(base, lo.as_deref(), hi.as_deref(), *incl, *m);
                    }
                }
                // `&"lit"` などの値は一時領域へ
                let p = self.place_or_temp(x)?;
                let a = self.addr_plus(p.addr, p.off);
                let t = Ty::Ref(*m, intern(p.ty));
                // (`&[T; N]` を `&[T]` の位置に)
                if let (Some(Ty::Ref(_, Ty::Slice(_))), Ty::Array(_, n)) = (expected, p.ty) {
                    let fat = self.make_fat(a, n as i64);
                    return Ok((Some(fat), expected.unwrap()));
                }
                Ok((Some(a), t))
            }
            Expr::Binary(op, l, r) => self.binary(op, l, r, expected),
            Expr::Assign(l, r) => {
                let p = self.place(l)?;
                let (v, vt) = self.expr(r, Some(p.ty))?;
                let v = self.coerce(v, vt, p.ty);
                self.write(p, v);
                Ok((None, Ty::Unit))
            }
            Expr::AssignOp(op, l, r) => {
                let p = self.place(l)?;
                let (cur, t) = self.read(p);
                let cur = cur.ok_or("bad operand")?;
                let rh = if matches!(op.as_str(), "<<" | ">>") { None } else { Some(t) };
                let (v, rt) = self.expr(r, rh)?;
                let mut v = v.ok_or("bad operand")?;
                let mut rt = rt;
                while let Ty::Ref(_, i) = rt {
                    v = self.b.load(clif_ty(*i).ok_or("bad operand")?, v, 0);
                    rt = *i;
                }
                let v = if rt != t { self.convert(v, rt, t) } else { v };
                let n = self.arith(op, cur, v, t)?;
                self.write(p, Some(n));
                Ok((None, Ty::Unit))
            }
            Expr::Cast(x, ty) => {
                let to = self.parse_ty(ty).or(self.peek(e)).ok_or("cannot infer the cast type")?;
                let (v, from) = self.expr(x, None)?;
                let v = v.ok_or("bad cast operand")?;
                Ok((Some(self.convert(v, from, to)), to))
            }
            Expr::Block(_, _, b) => self.block(b, expected),
            Expr::If(c, then, els) => self.if_(c, then, els.as_deref(), expected),
            Expr::While(_, c, body) => {
                let head = self.b.create_block();
                let bb = self.b.create_block();
                let exit = self.b.create_block();
                self.b.jump(head, &[]);
                self.b.switch_to_block(head);
                self.scopes.push(vec![]);
                self.cond(c, bb, exit)?;
                self.b.switch_to_block(bb);
                self.loops.push(Loop { cont: head, brk: exit });
                let (_, t) = self.block(body, Some(Ty::Unit))?;
                self.loops.pop();
                self.scopes.pop();
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
            Expr::For(_, pat, it, body) => self.for_(pat, it, body),
            Expr::Break(None, None) => {
                let brk = self.loops.last().ok_or("`break` outside of a loop")?.brk;
                self.b.jump(brk, &[]);
                Ok((None, Ty::Never))
            }
            Expr::Continue(None) => {
                let c = self.loops.last().ok_or("`continue` outside of a loop")?.cont;
                self.b.jump(c, &[]);
                Ok((None, Ty::Never))
            }
            Expr::Return(x) => {
                let ret = self.ret;
                let v = match x {
                    Some(x) => {
                        let (v, t) = self.expr(x, Some(ret))?;
                        self.coerce(v, t, ret)
                    }
                    None => None,
                };
                self.ret_value(v);
                Ok((None, Ty::Never))
            }
            Expr::Call(f, args) => self.call(f, args, expected),
            Expr::MethodCall(recv, name, _, args) => self.method_call(recv, name, args),
            Expr::Struct(p, fs, None) => self.struct_lit(p, fs, e),
            Expr::Tuple(xs) if xs.is_empty() => Ok((None, Ty::Unit)),
            Expr::Tuple(xs) => {
                let t = self.peek(e).or(expected).ok_or("cannot infer the tuple type")?;
                let Ty::Struct(sid) = t else { return self.err("bad tuple type") };
                self.tuple_struct(sid, xs)
            }
            Expr::Array(xs) => {
                let t = self.peek(e).or(expected).filter(|t| matches!(t, Ty::Array(..))).ok_or("cannot infer the array type")?;
                let Ty::Array(el, _) = t else { unreachable!() };
                let es = self.size_of(*el) as i32;
                let a = self.temp(t);
                for (i, x) in xs.iter().enumerate() {
                    let (v, vt) = self.expr(x, Some(*el))?;
                    let v = self.coerce(v, vt, *el);
                    self.write(Place { addr: a, off: i as i32 * es, ty: *el }, v);
                }
                Ok((Some(a), t))
            }
            Expr::Repeat(x, _) => {
                let t = self.peek(e).or(expected).filter(|t| matches!(t, Ty::Array(..))).ok_or("cannot infer the array type")?;
                let Ty::Array(el, n) = t else { unreachable!() };
                let es = self.size_of(*el) as i32;
                let a = self.temp(t);
                let (v, _) = self.expr(x, Some(*el))?;
                for i in 0..n {
                    self.write(Place { addr: a, off: i as i32 * es, ty: *el }, v);
                }
                Ok((Some(a), t))
            }
            Expr::Field(..) | Expr::Index(..) => {
                let p = self.place(e)?;
                Ok(self.read(p))
            }
            Expr::Match(scrut, arms) => self.match_(scrut, arms, expected),
            Expr::Macro(p, _) if p.segs.last().is_some_and(|s| s.name == "unreachable" || s.name == "panic") => {
                self.b.trap();
                Ok((None, Ty::Never))
            }
            _ => self.err(format!("unsupported expression: {}", short(e))),
        }
    }

    fn ret_value(&mut self, v: Option<Value>) {
        match (self.sret, v) {
            (Some(s), Some(v)) => {
                let n = self.size_of(self.ret);
                self.memcpy(s, v, n);
                self.b.return_(&[]);
            }
            (None, Some(v)) if clif_ty(self.ret).is_some() => self.b.return_(&[v]),
            _ => self.b.return_(&[]),
        }
    }

    /// `&a[lo..hi]`
    fn slice_range(&mut self, base: &Expr, lo: Option<&Expr>, hi: Option<&Expr>, incl: bool, _m: bool) -> R<V> {
        let p = self.place_or_temp(base)?;
        let p = self.deref_place(p);
        let (ptr, len, elem, is_str) = match p.ty {
            Ty::Array(e, n) => {
                let a = self.addr_plus(p.addr, p.off);
                let l = self.b.iconst(Type::I64, n as i64);
                (a, l, *e, false)
            }
            Ty::Ref(_, Ty::Slice(e)) => (self.b.load(Type::I64, p.addr, p.off), self.b.load(Type::I64, p.addr, p.off + 8), **e, false),
            Ty::Ref(_, Ty::Str) => (self.b.load(Type::I64, p.addr, p.off), self.b.load(Type::I64, p.addr, p.off + 8), Ty::U8, true),
            t => return self.err(format!("cannot slice {t:?}")),
        };
        let a = match lo {
            Some(x) => {
                let (v, t) = self.expr(x, Some(Ty::Usize))?;
                let v = v.ok_or("bad index")?;
                self.convert(v, t, Ty::Usize)
            }
            None => self.b.iconst(Type::I64, 0),
        };
        let mut bnd = match hi {
            Some(x) => {
                let (v, t) = self.expr(x, Some(Ty::Usize))?;
                let v = v.ok_or("bad index")?;
                self.convert(v, t, Ty::Usize)
            }
            None => len,
        };
        if incl {
            bnd = self.b.iadd_imm(bnd, 1);
        }
        let ok1 = self.b.icmp(IntCC::UnsignedLessThanOrEqual, a, bnd);
        self.trap_unless(ok1);
        let ok2 = self.b.icmp(IntCC::UnsignedLessThanOrEqual, bnd, len);
        self.trap_unless(ok2);
        let es = self.b.iconst(Type::I64, self.size_of(elem) as i64);
        let off = self.b.imul(a, es);
        let np = self.b.iadd(ptr, off);
        let nl = self.b.isub(bnd, a);
        let fat = self.make_fat_v(np, nl);
        let t = if is_str { Ty::Ref(false, intern(Ty::Str)) } else { Ty::Ref(false, intern(Ty::Slice(intern(elem)))) };
        Ok((Some(fat), t))
    }

    /// 条件 (`if let` / `while let` のパターンもここで束縛する): 真なら yes へ、偽なら no へ
    fn cond(&mut self, c: &Expr, yes: CBlock, no: CBlock) -> R<()> {
        if let Expr::Let(pat, x) = c {
            let p = self.place_or_temp(x)?;
            self.pat_test(pat, p, no)?;
            self.pat_bind(pat, p, false)?;
            self.b.jump(yes, &[]);
            return Ok(());
        }
        // `a && let ..` は扱わない
        let (cv, _) = self.expr(c, Some(Ty::Bool))?;
        let cv = cv.ok_or("bad condition")?;
        self.b.brif(cv, yes, &[], no, &[]);
        Ok(())
    }

    fn if_(&mut self, c: &Expr, then: &Block, els: Option<&Expr>, expected: Option<Ty>) -> R<V> {
        let tb = self.b.create_block();
        let eb = self.b.create_block();
        let done = self.b.create_block();
        self.scopes.push(vec![]);
        self.cond(c, tb, eb)?;
        self.b.switch_to_block(tb);
        let (tv, tt) = self.block(then, expected)?;
        self.scopes.pop();
        let rty = if tt == Ty::Never { expected } else { Some(tt) };
        let param = match (els, rty.and_then(abi_ty)) {
            (Some(_), Some(ct)) if rty != Some(Ty::Unit) => Some(self.b.append_block_param(done, ct)),
            _ => None,
        };
        if tt != Ty::Never {
            let args: Vec<Value> = if param.is_some() { tv.into_iter().collect() } else { vec![] };
            self.b.jump(done, &args);
        }
        self.b.switch_to_block(eb);
        let mut et = Ty::Unit;
        if let Some(x) = els {
            let (ev, t2) = self.expr(x, rty.or(expected))?;
            et = t2;
            if t2 != Ty::Never {
                let ev = match (rty, ev) {
                    (Some(rt), ev) => self.coerce(ev, t2, rt),
                    (None, ev) => ev,
                };
                let args: Vec<Value> = if param.is_some() { ev.into_iter().collect() } else { vec![] };
                self.b.jump(done, &args);
            }
        } else {
            self.b.jump(done, &[]);
        }
        self.b.switch_to_block(done);
        if tt == Ty::Never && et == Ty::Never {
            return Ok((None, Ty::Never));
        }
        let ty = if tt == Ty::Never { et } else { tt };
        Ok((param, if els.is_some() { ty } else { Ty::Unit }))
    }

    fn match_(&mut self, scrut: &Expr, arms: &[Arm], expected: Option<Ty>) -> R<V> {
        let p = self.place_or_temp(scrut)?;
        let done = self.b.create_block();
        let mut param: Option<Value> = None;
        let mut rty: Option<Ty> = expected.filter(|t| *t != Ty::Never);
        let mut all_never = true;
        for arm in arms {
            let next = self.b.create_block();
            self.scopes.push(vec![]);
            self.pat_test(&arm.pat, p, next)?;
            self.pat_bind(&arm.pat, p, false)?;
            if let Some(g) = &arm.guard {
                let ok = self.b.create_block();
                let (gv, _) = self.expr(g, Some(Ty::Bool))?;
                self.b.brif(gv.ok_or("bad guard")?, ok, &[], next, &[]);
                self.b.switch_to_block(ok);
            }
            let (v, t) = self.expr(&arm.body, rty)?;
            self.scopes.pop();
            if t != Ty::Never {
                all_never = false;
                if rty.is_none() {
                    rty = Some(t);
                }
                let rt = rty.unwrap();
                if param.is_none() && rt != Ty::Unit {
                    if let Some(ct) = abi_ty(rt) {
                        param = Some(self.b.append_block_param(done, ct));
                    }
                }
                let v = self.coerce(v, t, rt);
                let args: Vec<Value> = if param.is_some() { v.into_iter().collect() } else { vec![] };
                self.b.jump(done, &args);
            }
            self.b.switch_to_block(next);
        }
        // (どの腕にも合わない: 網羅されているので来ない)
        self.b.trap();
        self.b.switch_to_block(done);
        if all_never {
            return Ok((None, Ty::Never));
        }
        Ok((param, rty.unwrap_or(Ty::Unit)))
    }

    fn for_(&mut self, pat: &Pat, it: &Expr, body: &Block) -> R<V> {
        let head = self.b.create_block();
        let bb = self.b.create_block();
        let step = self.b.create_block();
        let exit = self.b.create_block();
        // 範囲 `a..b` / `a..=b`
        if let Expr::Range(Some(lo), Some(hi), incl) = it {
            let t = self.peek(lo).or(self.peek(hi)).unwrap_or(Ty::I32);
            let (a, _) = self.expr(lo, Some(t))?;
            let (z, _) = self.expr(hi, Some(t))?;
            let (a, z) = (a.ok_or("bad range")?, z.ok_or("bad range")?);
            let ct = clif_ty(t).ok_or("bad range type")?;
            let i = self.b.append_block_param(head, ct);
            self.b.jump(head, &[a]);
            self.b.switch_to_block(head);
            let cc = match (*incl, t.signed()) {
                (false, true) => IntCC::SignedLessThan,
                (false, false) => IntCC::UnsignedLessThan,
                (true, true) => IntCC::SignedLessThanOrEqual,
                (true, false) => IntCC::UnsignedLessThanOrEqual,
            };
            let more = self.b.icmp(cc, i, z);
            self.b.brif(more, bb, &[], exit, &[]);
            self.b.switch_to_block(bb);
            self.scopes.push(vec![]);
            let ip = self.temp(t);
            self.b.store(i, ip, 0);
            self.pat_bind(pat, Place { addr: ip, off: 0, ty: t }, false)?;
            self.loops.push(Loop { cont: step, brk: exit });
            let (_, bt) = self.block(body, Some(Ty::Unit))?;
            self.loops.pop();
            self.scopes.pop();
            if bt != Ty::Never {
                self.b.jump(step, &[]);
            }
            self.b.switch_to_block(step);
            // (`a..=b` で b が型の最大なら止める)
            if *incl {
                let last = self.b.icmp(IntCC::Equal, i, z);
                let cont = self.b.create_block();
                self.b.brif(last, exit, &[], cont, &[]);
                self.b.switch_to_block(cont);
            }
            let i2 = self.b.iadd_imm(i, 1);
            self.b.jump(head, &[i2]);
            self.b.switch_to_block(exit);
            return Ok((None, Ty::Unit));
        }
        // 配列・スライス (`&a` / `a.iter()` / `&a[1..]`): 要素への参照
        let (src, by_ref) = match it {
            Expr::MethodCall(r, n, _, a) if (n == "iter" || n == "iter_mut") && a.is_empty() => (&**r, true),
            e => (e, false),
        };
        let p = self.place_or_temp(src)?;
        // (`for x in [1, 2, 3]`: 配列の値なら要素の値)
        let by_value = !by_ref && matches!(p.ty, Ty::Array(..));
        let p = self.deref_place(p);
        let (ptr, len, elem) = match p.ty {
            Ty::Array(e, n) => {
                let a = self.addr_plus(p.addr, p.off);
                (a, self.b.iconst(Type::I64, n as i64), *e)
            }
            Ty::Ref(_, Ty::Slice(e)) => (self.b.load(Type::I64, p.addr, p.off), self.b.load(Type::I64, p.addr, p.off + 8), **e),
            t => return self.err(format!("cannot iterate over {t:?}")),
        };
        let es = self.size_of(elem) as i64;
        let i = self.b.append_block_param(head, Type::I64);
        let z = self.b.iconst(Type::I64, 0);
        self.b.jump(head, &[z]);
        self.b.switch_to_block(head);
        let more = self.b.icmp(IntCC::UnsignedLessThan, i, len);
        self.b.brif(more, bb, &[], exit, &[]);
        self.b.switch_to_block(bb);
        self.scopes.push(vec![]);
        let k = self.b.iconst(Type::I64, es);
        let off = self.b.imul(i, k);
        let ea = self.b.iadd(ptr, off);
        if by_value {
            self.pat_bind(pat, Place { addr: ea, off: 0, ty: elem }, false)?;
        } else {
            let rt = Ty::Ref(false, intern(elem));
            let rp = self.temp(rt);
            self.b.store(ea, rp, 0);
            self.pat_bind(pat, Place { addr: rp, off: 0, ty: rt }, false)?;
        }
        self.loops.push(Loop { cont: step, brk: exit });
        let (_, bt) = self.block(body, Some(Ty::Unit))?;
        self.loops.pop();
        self.scopes.pop();
        if bt != Ty::Never {
            self.b.jump(step, &[]);
        }
        self.b.switch_to_block(step);
        let i2 = self.b.iadd_imm(i, 1);
        self.b.jump(head, &[i2]);
        self.b.switch_to_block(exit);
        Ok((None, Ty::Unit))
    }

    // --- パターン --------------------------------------------------------

    /// 名前のパターンが unit バリアント (`None`) なら、そのバリアント
    fn unit_variant(&self, n: &str) -> Option<(u32, usize)> {
        let (sid, vi) = self.prog.variant_by_p(&PPath::of(&Path { global: false, segs: vec![crate::parse::Seg { name: n.to_string(), args: vec![], fn_args: None, text: n.to_string() }] }), None)?;
        (self.prog.structs[sid as usize].variants[vi].kind == VariantKind::Unit).then_some((sid, vi))
    }

    /// enum の判別値を確かめる (合わなければ fail へ)
    fn test_tag(&mut self, p: Place, vi: usize, fail: CBlock) -> R<()> {
        let Ty::Struct(sid) = p.ty else { return self.err("not an enum") };
        let sd = &self.prog.structs[sid as usize];
        let (tag, discr) = (sd.tag, sd.variants[vi].discr);
        let tv = self.b.load(clif_ty(tag).unwrap_or(Type::I8), p.addr, p.off);
        let ok = self.b.icmp_imm(IntCC::Equal, tv, discr);
        let cont = self.b.create_block();
        self.b.brif(ok, cont, &[], fail, &[]);
        self.b.switch_to_block(cont);
        Ok(())
    }

    /// パターンが合うか (合わなければ fail へ飛ぶ。合えば今のブロックに続く)
    fn pat_test(&mut self, pat: &Pat, p: Place, fail: CBlock) -> R<()> {
        // (参照の値に対するパターンは中身で見る: default binding modes)
        let p = match pat {
            Pat::Ident(_, _, n, None) if self.unit_variant(n).is_none() => return Ok(()),
            Pat::Wild | Pat::Rest => return Ok(()),
            Pat::Ref(_, inner) => {
                let q = self.deref_one(p)?;
                return self.pat_test(inner, q, fail);
            }
            _ => self.deref_place(p),
        };
        match pat {
            Pat::Ident(_, _, n, None) => {
                let (_, vi) = self.unit_variant(n).unwrap();
                self.test_tag(p, vi, fail)
            }
            Pat::Ident(_, _, _, Some(sub)) => self.pat_test(sub, p, fail),
            Pat::Path(path) => {
                let (_, vi) = self.prog.variant_by_p(&PPath::of(path), None).ok_or("unknown variant")?;
                self.test_tag(p, vi, fail)
            }
            Pat::Lit(x) => {
                let (lv, _) = self.pat_const(x, p.ty)?;
                let lv = lv.ok_or("bad literal pattern")?;
                if p.ty.is_fat() {
                    let pa = self.addr_plus(p.addr, p.off);
                    let (r, _) = self.agg_eq("==", pa, lv, p.ty)?;
                    let cont = self.b.create_block();
                    self.b.brif(r.unwrap(), cont, &[], fail, &[]);
                    self.b.switch_to_block(cont);
                    return Ok(());
                }
                let (v, _) = self.read(p);
                let ok = self.b.icmp(IntCC::Equal, v.ok_or("bad scrutinee")?, lv);
                let cont = self.b.create_block();
                self.b.brif(ok, cont, &[], fail, &[]);
                self.b.switch_to_block(cont);
                Ok(())
            }
            Pat::Range(lo, hi, incl) => {
                let (v, _) = self.read(p);
                let v = v.ok_or("bad scrutinee")?;
                let s = p.ty.signed();
                if let Some(lo) = lo {
                    let (l, _) = self.pat_const(lo, p.ty)?;
                    let cc = if s { IntCC::SignedGreaterThanOrEqual } else { IntCC::UnsignedGreaterThanOrEqual };
                    let ok = self.b.icmp(cc, v, l.ok_or("bad range")?);
                    let cont = self.b.create_block();
                    self.b.brif(ok, cont, &[], fail, &[]);
                    self.b.switch_to_block(cont);
                }
                if let Some(hi) = hi {
                    let (h, _) = self.pat_const(hi, p.ty)?;
                    let cc = match (*incl, s) {
                        (true, true) => IntCC::SignedLessThanOrEqual,
                        (true, false) => IntCC::UnsignedLessThanOrEqual,
                        (false, true) => IntCC::SignedLessThan,
                        (false, false) => IntCC::UnsignedLessThan,
                    };
                    let ok = self.b.icmp(cc, v, h.ok_or("bad range")?);
                    let cont = self.b.create_block();
                    self.b.brif(ok, cont, &[], fail, &[]);
                    self.b.switch_to_block(cont);
                }
                Ok(())
            }
            Pat::Or(cs) => {
                // どれかが合えば ok へ
                let ok = self.b.create_block();
                for c in cs {
                    let next = self.b.create_block();
                    self.pat_test(c, p, next)?;
                    self.b.jump(ok, &[]);
                    self.b.switch_to_block(next);
                }
                self.b.jump(fail, &[]);
                self.b.switch_to_block(ok);
                Ok(())
            }
            Pat::Tuple(ps) => {
                let Ty::Struct(sid) = p.ty else { return self.err("tuple pattern on a non-tuple") };
                let fields = self.prog.structs[sid as usize].fields.clone();
                for (sp, (_, ft, off)) in ps.iter().zip(fields) {
                    self.pat_test(sp, Place { addr: p.addr, off: p.off + off as i32, ty: ft }, fail)?;
                }
                Ok(())
            }
            Pat::TupleStruct(path, ps) => {
                let fields = if let Some((_, vi)) = self.prog.variant_by_p(&PPath::of(path), None) {
                    self.test_tag(p, vi, fail)?;
                    let Ty::Struct(sid) = p.ty else { unreachable!() };
                    self.prog.structs[sid as usize].variants[vi].fields.clone()
                } else {
                    let Ty::Struct(sid) = p.ty else { return self.err("bad pattern") };
                    self.prog.structs[sid as usize].fields.clone()
                };
                for (sp, (_, ft, off)) in ps.iter().zip(fields) {
                    self.pat_test(sp, Place { addr: p.addr, off: p.off + off as i32, ty: ft }, fail)?;
                }
                Ok(())
            }
            Pat::Struct(path, fps, _) => {
                let fields = if let Some((_, vi)) = self.prog.variant_by_p(&PPath::of(path), None) {
                    self.test_tag(p, vi, fail)?;
                    let Ty::Struct(sid) = p.ty else { unreachable!() };
                    self.prog.structs[sid as usize].variants[vi].fields.clone()
                } else {
                    let Ty::Struct(sid) = p.ty else { return self.err("bad pattern") };
                    self.prog.structs[sid as usize].fields.clone()
                };
                for (n, sp) in fps {
                    let (_, ft, off) = fields.iter().find(|f| &f.0 == n).cloned().ok_or_else(|| format!("no field `{n}`"))?;
                    self.pat_test(sp, Place { addr: p.addr, off: p.off + off as i32, ty: ft }, fail)?;
                }
                Ok(())
            }
            _ => self.err(format!("unsupported pattern: {}", short_pat(pat))),
        }
    }

    /// パターンの中の定数 (整数リテラルは照合する値の型で作る)
    fn pat_const(&mut self, x: &Expr, t: Ty) -> R<V> {
        let (neg, lit) = match x {
            Expr::Lit(l) => (false, l),
            Expr::Unary(op, y) if op == "-" => match &**y {
                Expr::Lit(l) => (true, l),
                _ => return self.expr(x, Some(t)),
            },
            _ => return self.expr(x, Some(t)),
        };
        match (clif_ty(t), lit_value(lit)) {
            (Some(ct), Some(v)) if !lit.starts_with('"') && !lit.starts_with("b\"") => {
                let v = if neg { v.wrapping_neg() } else { v };
                Ok((Some(self.b.iconst(ct, v)), t))
            }
            _ => self.expr(x, Some(t)),
        }
    }

    /// 参照を 1 段だけ辿る
    fn deref_one(&mut self, p: Place) -> R<Place> {
        match p.ty {
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) if !p.ty.is_fat() => {
                let a = self.b.load(Type::I64, p.addr, p.off);
                Ok(Place { addr: a, off: 0, ty: *i })
            }
            t => self.err(format!("`&` pattern on {t:?}")),
        }
    }

    /// パターンの名前を束縛する (by_ref: 参照越しに合わせたので参照で束縛する)
    fn pat_bind(&mut self, pat: &Pat, p: Place, by_ref: bool) -> R<()> {
        match pat {
            Pat::Wild | Pat::Rest | Pat::Lit(_) | Pat::Range(..) | Pat::Path(_) => Ok(()),
            Pat::Ident(_, _, n, None) if self.unit_variant(n).is_some() => Ok(()),
            Pat::Ident(is_ref, _, n, sub) => {
                if let Some(sub) = sub {
                    self.pat_bind(sub, p, by_ref)?;
                }
                if by_ref || *is_ref {
                    let a = self.addr_plus(p.addr, p.off);
                    self.bind(n, Ty::Ref(false, intern(p.ty)), Some(a));
                } else {
                    let (v, t) = self.read(p);
                    self.bind(n, t, v);
                }
                Ok(())
            }
            Pat::Ref(_, inner) => {
                let q = self.deref_one(p)?;
                self.pat_bind(inner, q, false)
            }
            Pat::Or(cs) => match cs.first() {
                // (どの選択肢も同じ名前を束縛する: 最初のもので位置を決める。選択肢ごとに束縛の場所が違う場合は扱わない)
                Some(c) => self.pat_bind(c, p, by_ref),
                None => Ok(()),
            },
            _ => {
                // 構造のパターン: 参照越しなら参照で束縛する
                let (p, by_ref) = if matches!(p.ty, Ty::Ref(..)) && !p.ty.is_fat() { (self.deref_place(p), true) } else { (p, by_ref) };
                let Ty::Struct(sid) = p.ty else { return self.err("bad structure pattern") };
                let (fields, ps): (Vec<(String, Ty, u32)>, Vec<(Option<String>, &Pat)>) = match pat {
                    Pat::Tuple(ps) => (self.prog.structs[sid as usize].fields.clone(), ps.iter().map(|x| (None, x)).collect()),
                    Pat::TupleStruct(path, ps) => {
                        let f = match self.prog.variant_by_p(&PPath::of(path), None) {
                            Some((_, vi)) => self.prog.structs[sid as usize].variants[vi].fields.clone(),
                            None => self.prog.structs[sid as usize].fields.clone(),
                        };
                        (f, ps.iter().map(|x| (None, x)).collect())
                    }
                    Pat::Struct(path, fps, _) => {
                        let f = match self.prog.variant_by_p(&PPath::of(path), None) {
                            Some((_, vi)) => self.prog.structs[sid as usize].variants[vi].fields.clone(),
                            None => self.prog.structs[sid as usize].fields.clone(),
                        };
                        (f, fps.iter().map(|(n, x)| (Some(n.clone()), x)).collect())
                    }
                    _ => return self.err(format!("unsupported pattern: {}", short_pat(pat))),
                };
                for (i, (n, sp)) in ps.iter().enumerate() {
                    let f = match n {
                        Some(n) => fields.iter().find(|f| &f.0 == n).cloned(),
                        None => fields.get(i).cloned(),
                    };
                    let Some((_, ft, off)) = f else { continue };
                    self.pat_bind(sp, Place { addr: p.addr, off: p.off + off as i32, ty: ft }, by_ref)?;
                }
                Ok(())
            }
        }
    }
}

fn short_pat(p: &Pat) -> String {
    let s = format!("{p:?}");
    s.chars().take(60).collect()
}

/// 1 関数の機械語 (名前, 機械語, 再配置, 外から見えるか) と、作ったデータ
pub fn compile_fn(prog: &Program, id: u32, psig: &crate::parse::FnSig, body: &Block) -> R<((String, Vec<u8>, Vec<Reloc>, bool), Vec<DataDef>)> {
    let sig: &FnSig = &prog.sigs[id as usize];
    let env = TyEnv { self_sid: sig.impl_of, params: vec![], self_ty: sig.self_ty.filter(|t| !prog.has_param(*t)) };
    let lets = crate::ast_infer::infer(prog, psig.recv.is_some(), &psig.params, body, sig, &env);
    let mut ps: Vec<Type> = vec![];
    if sig.ret.is_agg() {
        ps.push(Type::I64);
    }
    ps.extend(sig.params.iter().filter_map(|&t| abi_ty(t)));
    let rets: Vec<Type> = if sig.ret.is_agg() { vec![] } else { clif_ty(sig.ret).into_iter().collect() };
    let b = FunctionBuilder::new(ps, rets);
    let name = if sig.is_main { "main".to_string() } else { sig.symbol.clone() };
    let mut cx = Cx { b, prog, lets, scopes: vec![vec![]], loops: vec![], ret: sig.ret, env, sret: None, datas: vec![], name: name.clone() };
    let pvals = cx.b.func_params();
    let mut k = 0;
    if sig.ret.is_agg() {
        cx.sret = Some(pvals[0]);
        k = 1;
    }
    let mut names: Vec<Option<&Pat>> = vec![];
    if psig.recv.is_some() {
        names.push(None);
    }
    for (p, _) in &psig.params {
        names.push(Some(p));
    }
    for (i, &t) in sig.params.iter().enumerate() {
        let v = if abi_ty(t).is_some() {
            k += 1;
            Some(pvals[k - 1])
        } else {
            None
        };
        match names.get(i).copied().flatten() {
            None => cx.bind("self", t, v),
            Some(Pat::Ident(_, _, n, None)) => cx.bind(n, t, v),
            Some(Pat::Wild) => {}
            Some(p) => {
                let a = cx.temp(t);
                let pl = Place { addr: a, off: 0, ty: t };
                cx.write(pl, v);
                cx.pat_bind(p, pl, false)?;
            }
        }
    }
    let (v, t) = cx.block(body, Some(sig.ret))?;
    if t != Ty::Never {
        let ret = cx.ret;
        let v = cx.coerce(v, t, ret);
        cx.ret_value(v);
    }
    let datas = std::mem::take(&mut cx.datas);
    let (code, relocs) = cx.b.finish();
    let global = sig.is_main || sig.export;
    Ok(((name, code, relocs, global), datas))
}

/// ソースから ELF の .o を作る (syn・Cranelift を使わない経路)。いまはユーザの (ジェネリックでない) 関数だけを機械語にする
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
    let mut datas = vec![];
    for id in 0..prog.names.len() {
        let sig = &prog.sigs[id];
        let Some(ii) = prog.item_of[id] else { continue };
        if ii as usize >= user {
            continue;
        }
        if sig.is_generic() || !sig.has_body || sig.is_extern || prog.closures.contains_key(&(id as u32)) {
            continue;
        }
        let it = &items[ii as usize];
        let f = crate::parse::parse_file(&src[it.start..it.end])?;
        let Some(ItemKind::Fn(psig, Some(body))) = f.items.first().map(|i| &i.kind) else { continue };
        let (r, ds) = compile_fn(&prog, id as u32, psig, body).map_err(|e| format!("in `{}`: {e}", prog.names[id]))?;
        funcs.push(r);
        datas.extend(ds);
    }
    Ok(crate::minclif::write_object_full(&funcs, &datas))
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
