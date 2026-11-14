//! 関数 1 個分の syn AST → Cranelift IR → 機械語。
//!
//! 型検査は lowering と同時に一発で行う (single pass)。中間 IR を持たないので
//! メモリ確保が少なく、関数ごとに独立しているのでスレッド並列にできる。
//!
//! 出力は `Blob` (機械語 + シンボリックなリロケーション)。呼び出し先は関数 ID で
//! 参照するだけなので、呼び出し先の「本体」が変わっても Blob は再利用できる
//! (blaze の template と同じ発想。キャッシュキーにはシグネチャしか入れない)。

use std::collections::HashMap;

use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    self, AbiParam, Block, ExtFuncData, ExternalName, FuncRef, InstBuilder, Signature, TrapCode,
    UserExternalName, UserFuncName, Value, types,
};
use cranelift_codegen::isa::{CallConv, TargetIsa};
use cranelift_codegen::{Context, RelocTarget, binemit::Reloc};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use syn::spanned::Spanned;
use syn::{BinOp, Expr, Lit, Pat, Stmt, UnOp};

use crate::front::{FnSig, Program, R, Ty, err};

// ---------------------------------------------------------------------------
// 出力形式
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Blob {
    pub align: u32,
    pub code: Vec<u8>,
    pub relocs: Vec<BlobReloc>,
}

#[derive(Clone, Debug)]
pub struct BlobReloc {
    pub offset: u32,
    pub kind: u8,
    pub addend: i64,
    pub target: Target,
}

#[derive(Clone, Debug)]
pub enum Target {
    /// 関数 ID (Program::names の index)
    Func(u32),
    /// 自関数内のオフセット
    SelfOffset(u32),
}

/// キャッシュに書くため Reloc を 1 byte にする表。
pub const RELOC_KINDS: &[Reloc] = &[
    Reloc::Abs4,
    Reloc::Abs8,
    Reloc::X86PCRel4,
    Reloc::X86CallPCRel4,
    Reloc::X86CallPLTRel4,
    Reloc::X86GOTPCRel4,
    Reloc::Arm64Call,
    Reloc::Aarch64AdrGotPage21,
    Reloc::Aarch64Ld64GotLo12Nc,
    Reloc::Aarch64AdrPrelPgHi21,
    Reloc::Aarch64AddAbsLo12Nc,
];

fn reloc_code(r: Reloc) -> u8 {
    RELOC_KINDS
        .iter()
        .position(|&k| k == r)
        .unwrap_or_else(|| panic!("unsupported reloc kind {r:?}")) as u8
}

// ---------------------------------------------------------------------------
// シグネチャ
// ---------------------------------------------------------------------------

fn abi_param(t: Ty) -> AbiParam {
    let p = AbiParam::new(clif_ty(t).unwrap());
    // C ABI 互換のため小さい整数は拡張して渡す
    match t.bits() {
        8 | 16 if t.signed() => p.sext(),
        8 | 16 => p.uext(),
        _ => p,
    }
}

pub fn clif_sig(s: &FnSig, cc: CallConv) -> Signature {
    let mut sig = Signature::new(cc);
    for &p in &s.params {
        sig.params.push(abi_param(p));
    }
    if s.is_main {
        // C の int main() として出す
        sig.returns.push(AbiParam::new(types::I32));
    } else if clif_ty(s.ret).is_some() {
        sig.returns.push(abi_param(s.ret));
    }
    sig
}

fn clif_ty(t: Ty) -> Option<ir::Type> {
    Some(match t.bits() {
        0 => return None,
        8 => types::I8,
        16 => types::I16,
        32 => types::I32,
        _ => types::I64,
    })
}

// ---------------------------------------------------------------------------
// 関数のコンパイル
// ---------------------------------------------------------------------------

/// スレッドごとに使い回す作業領域。毎回 alloc しないことでメモリのピークと
/// アロケータ負荷を抑える。
pub struct Worker {
    ctx: Context,
    fbc: FunctionBuilderContext,
}

impl Worker {
    pub fn new() -> Self {
        Worker {
            ctx: Context::new(),
            fbc: FunctionBuilderContext::new(),
        }
    }

    pub fn compile(&mut self, prog: &Program, id: u32, f: &syn::ItemFn, isa: &dyn TargetIsa) -> R<Blob> {
        let sig = &prog.sigs[id as usize];
        let cc = isa.default_call_conv();
        self.ctx.clear();
        self.ctx.func.signature = clif_sig(sig, cc);
        self.ctx.func.name = UserFuncName::user(0, id);

        {
            let b = FunctionBuilder::new(&mut self.ctx.func, &mut self.fbc);
            let mut cx = Cx {
                b,
                prog,
                cc,
                scopes: vec![vec![]],
                loops: vec![],
                ret: sig.ret,
                is_main: sig.is_main,
                frefs: HashMap::new(),
                lets: crate::infer::infer(prog, f, sig.ret),
            };
            cx.body(f)?;
            cx.b.seal_all_blocks();
            cx.b.finalize(isa.frontend_config());
        }

        self.ctx
            .compile(isa, &mut ControlPlane::default())
            .map_err(|e| format!("{}: codegen error: {:?}", prog.names[id as usize], e.inner))?;
        let code = self.ctx.compiled_code().unwrap();
        let mut relocs = vec![];
        for r in code.buffer.relocs() {
            let target = match r.target {
                RelocTarget::ExternalName(ExternalName::User(reff)) => {
                    Target::Func(self.ctx.func.params.user_named_funcs()[reff].index)
                }
                RelocTarget::Label(l) => Target::SelfOffset(l.as_offset()),
                ref t => return Err(format!("unsupported reloc target {t:?}")),
            };
            relocs.push(BlobReloc {
                offset: r.offset,
                kind: reloc_code(r.kind),
                addend: r.addend,
                target,
            });
        }
        Ok(Blob {
            align: code.buffer.min_alignment.max(1),
            code: code.buffer.data().to_vec(),
            relocs,
        })
    }
}

/// 式の評価結果。Unit / Never のとき値は None。
type V = (Option<Value>, Ty);

struct Loop {
    cont: Block,
    brk: Block,
    breaks: usize,
}

struct Cx<'a, 'p> {
    b: FunctionBuilder<'a>,
    prog: &'p Program,
    cc: CallConv,
    scopes: Vec<Vec<(String, Variable, Ty)>>,
    loops: Vec<Loop>,
    ret: Ty,
    is_main: bool,
    frefs: HashMap<u32, FuncRef>,
    lets: crate::infer::LetTypes,
}

fn same(span: proc_macro2::Span, got: Ty, want: Ty) -> R<()> {
    if got == want || got == Ty::Never {
        Ok(())
    } else {
        err(span, format!("mismatched types: expected {want:?}, found {got:?}"))
    }
}

impl Cx<'_, '_> {
    fn body(&mut self, f: &syn::ItemFn) -> R<()> {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let params = self.b.block_params(entry).to_vec();
        for (arg, v) in f.sig.inputs.iter().zip(params) {
            let syn::FnArg::Typed(pt) = arg else { unreachable!() };
            let ty = Ty::parse(&pt.ty)?;
            let name = pat_name(&pt.pat)?;
            self.bind(name, ty, Some(v));
        }
        let (v, t) = self.block(&f.block, Some(self.ret))?;
        if t == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(());
        }
        same(f.block.span(), t, self.ret)?;
        self.emit_return(v);
        Ok(())
    }

    fn emit_return(&mut self, v: Option<Value>) {
        if self.is_main {
            let z = self.b.ins().iconst(types::I32, 0);
            self.b.ins().return_(&[z]);
        } else {
            match v {
                Some(v) => self.b.ins().return_(&[v]),
                None => self.b.ins().return_(&[]),
            };
        }
    }

    /// 終端命令の後に続くコード用の到達不能ブロックへ移る。
    fn dead(&mut self) -> V {
        let blk = self.b.create_block();
        self.b.switch_to_block(blk);
        (None, Ty::Never)
    }

    fn bind(&mut self, name: String, ty: Ty, v: Option<Value>) {
        if let Some(ct) = clif_ty(ty) {
            let var = self.b.declare_var(ct);
            if let Some(v) = v {
                self.b.def_var(var, v);
            }
            self.scopes.last_mut().unwrap().push((name, var, ty));
        }
    }

    fn lookup(&self, name: &str) -> Option<(Variable, Ty)> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|s| s.iter().rev())
            .find(|(n, _, _)| n == name)
            .map(|&(_, v, t)| (v, t))
    }

    fn block(&mut self, blk: &syn::Block, expected: Option<Ty>) -> R<V> {
        self.scopes.push(vec![]);
        let mut diverged = false;
        let mut result: V = (None, Ty::Unit);
        let n = blk.stmts.len();
        for (i, st) in blk.stmts.iter().enumerate() {
            let last = i + 1 == n;
            match st {
                Stmt::Local(l) => {
                    let (name, ann) = match &l.pat {
                        Pat::Type(pt) => (pat_name(&pt.pat)?, Some(Ty::parse(&pt.ty)?)),
                        p => (pat_name(p)?, self.lets.get(&crate::infer::let_key(l)).copied()),
                    };
                    let Some(init) = &l.init else {
                        return err(l.span(), "`let` without initializer is not supported");
                    };
                    if init.diverge.is_some() {
                        return err(l.span(), "`let ... else` is not supported");
                    }
                    let (v, t) = self.expr(&init.expr, ann)?;
                    if t == Ty::Never {
                        diverged = true;
                        continue;
                    }
                    let ty = ann.unwrap_or(t);
                    same(init.expr.span(), t, ty)?;
                    self.bind(name, ty, v);
                }
                Stmt::Expr(e, semi) => {
                    let exp = if last && semi.is_none() { expected } else { None };
                    let r = self.expr(e, exp)?;
                    if r.1 == Ty::Never {
                        diverged = true;
                    }
                    if last && semi.is_none() {
                        result = r;
                    } else if semi.is_none() && !matches!(r.1, Ty::Unit | Ty::Never) {
                        return err(e.span(), "expected `;` after non-unit expression");
                    }
                }
                Stmt::Item(it) => return err(it.span(), "nested items are not supported"),
                Stmt::Macro(m) => return err(m.span(), "macros are not supported yet"),
            }
        }
        self.scopes.pop();
        if diverged && result.1 == Ty::Unit {
            result = (None, Ty::Never);
        }
        Ok(result)
    }

    /// コード生成せずに型だけ推測する (整数リテラルの型決定用)。
    fn peek(&self, e: &Expr) -> Option<Ty> {
        match e {
            Expr::Lit(l) => match &l.lit {
                Lit::Int(i) => Ty::from_suffix(i.suffix()),
                Lit::Bool(_) => Some(Ty::Bool),
                Lit::Byte(_) => Some(Ty::U8),
                _ => None,
            },
            Expr::Path(p) => p
                .path
                .get_ident()
                .and_then(|id| self.lookup(&id.to_string()))
                .map(|(_, t)| t),
            Expr::Call(c) => self.callee(&c.func).map(|(_, s)| s.ret),
            Expr::Cast(c) => Ty::parse(&c.ty).ok(),
            Expr::Paren(p) => self.peek(&p.expr),
            Expr::Group(g) => self.peek(&g.expr),
            Expr::Unary(u) => self.peek(&u.expr),
            Expr::Binary(b) if is_cmp(&b.op) || matches!(b.op, BinOp::And(_) | BinOp::Or(_)) => {
                Some(Ty::Bool)
            }
            Expr::Binary(b) if matches!(b.op, BinOp::Shl(_) | BinOp::Shr(_)) => self.peek(&b.left),
            Expr::Binary(b) => self.peek(&b.left).or_else(|| self.peek(&b.right)),
            _ => None,
        }
    }

    fn callee(&self, f: &Expr) -> Option<(u32, &FnSig)> {
        let Expr::Path(p) = f else { return None };
        let id = p.path.get_ident()?.to_string();
        if self.lookup(&id).is_some() {
            return None;
        }
        let &i = self.prog.index.get(&id)?;
        Some((i, &self.prog.sigs[i as usize]))
    }

    fn fref(&mut self, id: u32) -> FuncRef {
        if let Some(&f) = self.frefs.get(&id) {
            return f;
        }
        let sig = &self.prog.sigs[id as usize];
        let sigref = self.b.import_signature(clif_sig(sig, self.cc));
        let name = self
            .b
            .func
            .declare_imported_user_function(UserExternalName::new(0, id));
        let f = self.b.import_function(ExtFuncData {
            name: ExternalName::user(name),
            signature: sigref,
            // 同じオブジェクト内に定義される関数は直接 call、extern は GOT 経由
            colocated: !sig.is_extern,
            patchable: false,
        });
        self.frefs.insert(id, f);
        f
    }

    fn expr(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        match e {
            Expr::Lit(l) => self.lit(&l.lit, expected),
            Expr::Paren(p) => self.expr(&p.expr, expected),
            Expr::Group(g) => self.expr(&g.expr, expected),
            Expr::Block(b) => {
                if b.label.is_some() {
                    return err(b.span(), "labeled blocks are not supported");
                }
                self.block(&b.block, expected)
            }
            Expr::Unsafe(u) => self.block(&u.block, expected),
            Expr::Path(p) => {
                let Some(id) = p.path.get_ident() else {
                    return err(p.span(), "unsupported path");
                };
                match self.lookup(&id.to_string()) {
                    Some((var, t)) => Ok((Some(self.b.use_var(var)), t)),
                    None => err(p.span(), format!("cannot find value `{id}`")),
                }
            }
            Expr::Unary(u) => self.unary(u, expected),
            Expr::Binary(b) => self.binary(b, expected),
            Expr::Assign(a) => {
                let (var, t) = self.place(&a.left)?;
                let (v, rt) = self.expr(&a.right, Some(t))?;
                if rt == Ty::Never {
                    return Ok((None, Ty::Never));
                }
                same(a.right.span(), rt, t)?;
                if let Some(v) = v {
                    self.b.def_var(var, v);
                }
                Ok((None, Ty::Unit))
            }
            Expr::If(i) => self.if_(i, expected),
            Expr::While(w) => {
                if w.label.is_some() {
                    return err(w.span(), "loop labels are not supported");
                }
                let header = self.b.create_block();
                let body = self.b.create_block();
                let exit = self.b.create_block();
                self.b.ins().jump(header, &[]);
                self.b.switch_to_block(header);
                let (c, ct) = self.expr(&w.cond, Some(Ty::Bool))?;
                same(w.cond.span(), ct, Ty::Bool)?;
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
                });
                let (_, bt) = self.block(&w.body, Some(Ty::Unit))?;
                self.loops.pop();
                if bt != Ty::Never {
                    same(w.body.span(), bt, Ty::Unit)?;
                    self.b.ins().jump(header, &[]);
                } else {
                    self.b.ins().trap(TrapCode::unwrap_user(1));
                }
                self.b.switch_to_block(exit);
                Ok((None, Ty::Unit))
            }
            Expr::Loop(l) => {
                if l.label.is_some() {
                    return err(l.span(), "loop labels are not supported");
                }
                let body = self.b.create_block();
                let exit = self.b.create_block();
                self.b.ins().jump(body, &[]);
                self.b.switch_to_block(body);
                self.loops.push(Loop {
                    cont: body,
                    brk: exit,
                    breaks: 0,
                });
                let (_, bt) = self.block(&l.body, Some(Ty::Unit))?;
                let lp = self.loops.pop().unwrap();
                if bt != Ty::Never {
                    same(l.body.span(), bt, Ty::Unit)?;
                    self.b.ins().jump(body, &[]);
                } else {
                    self.b.ins().trap(TrapCode::unwrap_user(1));
                }
                self.b.switch_to_block(exit);
                if lp.breaks == 0 {
                    // break のない loop は値を返さない (型は !)
                    return Ok((None, Ty::Never));
                }
                Ok((None, Ty::Unit))
            }
            Expr::Break(br) => {
                if br.label.is_some() || br.expr.is_some() {
                    return err(br.span(), "labeled break / break with value is not supported");
                }
                let Some(lp) = self.loops.last_mut() else {
                    return err(br.span(), "`break` outside of a loop");
                };
                lp.breaks += 1;
                let t = lp.brk;
                self.b.ins().jump(t, &[]);
                Ok(self.dead())
            }
            Expr::Continue(c) => {
                if c.label.is_some() {
                    return err(c.span(), "labeled continue is not supported");
                }
                let Some(lp) = self.loops.last() else {
                    return err(c.span(), "`continue` outside of a loop");
                };
                let t = lp.cont;
                self.b.ins().jump(t, &[]);
                Ok(self.dead())
            }
            Expr::Return(r) => {
                let ret = self.ret;
                let v = match &r.expr {
                    Some(e) => {
                        let (v, t) = self.expr(e, Some(ret))?;
                        if t == Ty::Never {
                            return Ok((None, Ty::Never));
                        }
                        same(e.span(), t, ret)?;
                        v
                    }
                    None => {
                        same(r.span(), Ty::Unit, ret)?;
                        None
                    }
                };
                self.emit_return(v);
                Ok(self.dead())
            }
            Expr::Call(c) => {
                let Some((id, sig)) = self.callee(&c.func) else {
                    return err(c.func.span(), "only direct calls to named functions are supported");
                };
                let sig = sig.clone();
                if sig.params.len() != c.args.len() {
                    return err(
                        c.span(),
                        format!("expected {} arguments, found {}", sig.params.len(), c.args.len()),
                    );
                }
                let mut args = vec![];
                for (a, &pt) in c.args.iter().zip(&sig.params) {
                    let (v, t) = self.expr(a, Some(pt))?;
                    if t == Ty::Never {
                        return Ok((None, Ty::Never));
                    }
                    same(a.span(), t, pt)?;
                    args.push(v.unwrap());
                }
                let f = self.fref(id);
                let call = self.b.ins().call(f, &args);
                if sig.ret == Ty::Never {
                    self.b.ins().trap(TrapCode::unwrap_user(1));
                    return Ok(self.dead());
                }
                let r = self.b.inst_results(call).first().copied();
                Ok((r, sig.ret))
            }
            Expr::Cast(c) => {
                let to = Ty::parse(&c.ty)?;
                let hint = self.peek(&c.expr).or(Some(Ty::I32));
                let (v, from) = self.expr(&c.expr, hint)?;
                if from == Ty::Never {
                    return Ok((None, Ty::Never));
                }
                if !to.is_int() || !(from.is_int() || from == Ty::Bool) {
                    return err(c.span(), format!("unsupported cast {from:?} as {to:?}"));
                }
                Ok((Some(self.convert(v.unwrap(), from, to)), to))
            }
            Expr::Macro(m) => err(m.span(), "macros are not supported yet"),
            _ => err(e.span(), "unsupported expression"),
        }
    }

    fn convert(&mut self, v: Value, from: Ty, to: Ty) -> Value {
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

    fn lit(&mut self, l: &Lit, expected: Option<Ty>) -> R<V> {
        match l {
            Lit::Int(i) => {
                let ty = Ty::from_suffix(i.suffix())
                    .or(expected.filter(|t| t.is_int()))
                    .unwrap_or(Ty::I32);
                let n: u128 = i.base10_parse().map_err(|e| e.to_string())?;
                let v = self.b.ins().iconst(clif_ty(ty).unwrap(), n as u64 as i64);
                Ok((Some(v), ty))
            }
            Lit::Bool(b) => Ok((Some(self.b.ins().iconst(types::I8, b.value as i64)), Ty::Bool)),
            Lit::Byte(b) => Ok((Some(self.b.ins().iconst(types::I8, b.value() as i64)), Ty::U8)),
            _ => err(l.span(), "unsupported literal"),
        }
    }

    fn place(&self, e: &Expr) -> R<(Variable, Ty)> {
        if let Expr::Path(p) = e {
            if let Some(id) = p.path.get_ident() {
                if let Some(x) = self.lookup(&id.to_string()) {
                    return Ok(x);
                }
            }
        }
        err(e.span(), "only local variables can be assigned")
    }

    fn unary(&mut self, u: &syn::ExprUnary, expected: Option<Ty>) -> R<V> {
        let (v, t) = self.expr(&u.expr, expected)?;
        let Some(v) = v else { return Ok((None, t)) };
        match u.op {
            UnOp::Neg(_) if t.is_int() && t.signed() => Ok((Some(self.b.ins().ineg(v)), t)),
            UnOp::Not(_) if t == Ty::Bool => Ok((Some(self.b.ins().bxor_imm_u(v, 1)), t)),
            UnOp::Not(_) if t.is_int() => Ok((Some(self.b.ins().bnot(v)), t)),
            _ => err(u.span(), format!("unsupported unary operator for {t:?}")),
        }
    }

    fn binary(&mut self, b: &syn::ExprBinary, expected: Option<Ty>) -> R<V> {
        use BinOp::*;
        // 短絡評価
        if let And(_) | Or(_) = b.op {
            let (l, lt) = self.expr(&b.left, Some(Ty::Bool))?;
            same(b.left.span(), lt, Ty::Bool)?;
            let Some(l) = l else { return Ok((None, Ty::Never)) };
            let rhs_blk = self.b.create_block();
            let merge = self.b.create_block();
            let res = self.b.append_block_param(merge, types::I8);
            if matches!(b.op, And(_)) {
                self.b.ins().brif(l, rhs_blk, &[], merge, &[l.into()]);
            } else {
                self.b.ins().brif(l, merge, &[l.into()], rhs_blk, &[]);
            }
            self.b.switch_to_block(rhs_blk);
            let (r, rt) = self.expr(&b.right, Some(Ty::Bool))?;
            same(b.right.span(), rt, Ty::Bool)?;
            match r {
                Some(r) => self.b.ins().jump(merge, &[r.into()]),
                None => self.b.ins().trap(TrapCode::unwrap_user(1)),
            };
            self.b.switch_to_block(merge);
            return Ok((Some(res), Ty::Bool));
        }

        // 複合代入 a op= b
        let compound = match b.op {
            AddAssign(_) => Some(Add(Default::default())),
            SubAssign(_) => Some(Sub(Default::default())),
            MulAssign(_) => Some(Mul(Default::default())),
            DivAssign(_) => Some(Div(Default::default())),
            RemAssign(_) => Some(Rem(Default::default())),
            BitXorAssign(_) => Some(BitXor(Default::default())),
            BitAndAssign(_) => Some(BitAnd(Default::default())),
            BitOrAssign(_) => Some(BitOr(Default::default())),
            ShlAssign(_) => Some(Shl(Default::default())),
            ShrAssign(_) => Some(Shr(Default::default())),
            _ => None,
        };
        if let Some(op) = compound {
            let (var, t) = self.place(&b.left)?;
            let l = self.b.use_var(var);
            let rhint = if matches!(op, Shl(_) | Shr(_)) { self.peek(&b.right) } else { Some(t) };
            let (r, rt) = self.expr(&b.right, rhint.or(Some(t)))?;
            let Some(r) = r else { return Ok((None, Ty::Never)) };
            let v = self.arith(b, &op, l, t, r, rt)?;
            self.b.def_var(var, v);
            return Ok((None, Ty::Unit));
        }

        let is_shift = matches!(b.op, Shl(_) | Shr(_));
        let hint = if is_cmp(&b.op) {
            self.peek(&b.left).or_else(|| self.peek(&b.right))
        } else if is_shift {
            self.peek(&b.left).or(expected)
        } else {
            self.peek(&b.left).or_else(|| self.peek(&b.right)).or(expected)
        };
        let (l, lt) = self.expr(&b.left, hint)?;
        let Some(l) = l else { return Ok((None, Ty::Never)) };
        let rhint = if is_shift { self.peek(&b.right) } else { Some(lt) };
        let (r, rt) = self.expr(&b.right, rhint.or(Some(lt)))?;
        let Some(r) = r else { return Ok((None, Ty::Never)) };

        if is_cmp(&b.op) {
            same(b.right.span(), rt, lt)?;
            if !(lt.is_int() || lt == Ty::Bool) {
                return err(b.span(), "comparison of unsupported type");
            }
            let s = lt.signed();
            let cc = match b.op {
                Eq(_) => IntCC::Equal,
                Ne(_) => IntCC::NotEqual,
                Lt(_) if s => IntCC::SignedLessThan,
                Lt(_) => IntCC::UnsignedLessThan,
                Le(_) if s => IntCC::SignedLessThanOrEqual,
                Le(_) => IntCC::UnsignedLessThanOrEqual,
                Gt(_) if s => IntCC::SignedGreaterThan,
                Gt(_) => IntCC::UnsignedGreaterThan,
                Ge(_) if s => IntCC::SignedGreaterThanOrEqual,
                Ge(_) => IntCC::UnsignedGreaterThanOrEqual,
                _ => unreachable!(),
            };
            return Ok((Some(self.b.ins().icmp(cc, l, r)), Ty::Bool));
        }
        let v = self.arith(b, &b.op, l, lt, r, rt)?;
        Ok((Some(v), lt))
    }

    fn arith(&mut self, b: &syn::ExprBinary, op: &BinOp, l: Value, lt: Ty, r: Value, rt: Ty) -> R<Value> {
        use BinOp::*;
        if matches!(op, Shl(_) | Shr(_)) {
            if !lt.is_int() || !rt.is_int() {
                return err(b.span(), "shift of non-integer");
            }
            let ins = self.b.ins();
            return Ok(match op {
                Shl(_) => ins.ishl(l, r),
                _ if lt.signed() => ins.sshr(l, r),
                _ => ins.ushr(l, r),
            });
        }
        same(b.right.span(), rt, lt)?;
        let bool_ok = matches!(op, BitAnd(_) | BitOr(_) | BitXor(_));
        if !(lt.is_int() || (lt == Ty::Bool && bool_ok)) {
            return err(b.span(), format!("unsupported operator for {lt:?}"));
        }
        let s = lt.signed();
        let ins = self.b.ins();
        Ok(match op {
            Add(_) => ins.iadd(l, r),
            Sub(_) => ins.isub(l, r),
            Mul(_) => ins.imul(l, r),
            Div(_) if s => ins.sdiv(l, r),
            Div(_) => ins.udiv(l, r),
            Rem(_) if s => ins.srem(l, r),
            Rem(_) => ins.urem(l, r),
            BitAnd(_) => ins.band(l, r),
            BitOr(_) => ins.bor(l, r),
            BitXor(_) => ins.bxor(l, r),
            _ => return err(b.span(), "unsupported binary operator"),
        })
    }

    fn if_(&mut self, i: &syn::ExprIf, expected: Option<Ty>) -> R<V> {
        let (c, ct) = self.expr(&i.cond, Some(Ty::Bool))?;
        same(i.cond.span(), ct, Ty::Bool)?;
        let Some(c) = c else { return Ok((None, Ty::Never)) };
        let then_blk = self.b.create_block();
        let else_blk = self.b.create_block();
        let merge = self.b.create_block();
        self.b.ins().brif(c, then_blk, &[], else_blk, &[]);

        let mut result_ty: Option<Ty> = None;
        let mut param: Option<Value> = None;

        // then
        self.b.switch_to_block(then_blk);
        let (tv, tt) = self.block(&i.then_branch, expected)?;
        self.finish_arm(tv, tt, merge, &mut result_ty, &mut param, i.then_branch.span())?;

        // else
        self.b.switch_to_block(else_blk);
        let (ev, et) = match &i.else_branch {
            Some((_, e)) => self.expr(e, result_ty.or(expected))?,
            None => (None, Ty::Unit),
        };
        self.finish_arm(ev, et, merge, &mut result_ty, &mut param, i.span())?;

        self.b.switch_to_block(merge);
        match result_ty {
            None => Ok((None, Ty::Never)),
            Some(t) => Ok((param, t)),
        }
    }

    fn finish_arm(
        &mut self,
        v: Option<Value>,
        t: Ty,
        merge: Block,
        result_ty: &mut Option<Ty>,
        param: &mut Option<Value>,
        span: proc_macro2::Span,
    ) -> R<()> {
        if t == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(());
        }
        match *result_ty {
            Some(rt) => same(span, t, rt)?,
            None => {
                *result_ty = Some(t);
                if let Some(ct) = clif_ty(t) {
                    *param = Some(self.b.append_block_param(merge, ct));
                }
            }
        }
        match v {
            Some(v) => self.b.ins().jump(merge, &[v.into()]),
            None => self.b.ins().jump(merge, &[]),
        };
        Ok(())
    }
}

fn is_cmp(op: &BinOp) -> bool {
    matches!(
        op,
        BinOp::Eq(_) | BinOp::Ne(_) | BinOp::Lt(_) | BinOp::Le(_) | BinOp::Gt(_) | BinOp::Ge(_)
    )
}

fn pat_name(p: &Pat) -> R<String> {
    match p {
        Pat::Ident(pi) if pi.subpat.is_none() && pi.by_ref.is_none() => Ok(pi.ident.to_string()),
        Pat::Wild(_) => Ok("_".into()),
        _ => err(p.span(), "unsupported pattern"),
    }
}
