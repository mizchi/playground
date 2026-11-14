//! 関数 1 個分の syn AST → Cranelift IR → 機械語。
//!
//! 型検査は lowering と同時に一発で行う (single pass)。中間 IR を持たないので
//! メモリ確保が少なく、関数ごとに独立しているのでスレッド並列にできる。
//!
//! 値の表現:
//! - 整数 / bool / 参照: SSA 値 (参照はポインタ)
//! - struct: 常にメモリ上にあり、「値」はそのアドレス。let / 代入 / 引数 / return の
//!   ところで必ずコピーする。引数は呼び出し側が一時領域にコピーしてそのアドレスを渡し、
//!   戻り値は呼び出し側が確保した領域のアドレスを隠れ第 1 引数 (sret) で渡す。
//! - ローカル変数: アドレスを取られない (`&x` が現れない) スカラーは Cranelift の
//!   Variable (レジスタ)、それ以外はスタックスロット。
//!
//! 出力は `Blob` (機械語 + シンボリックなリロケーション)。呼び出し先は関数 ID で
//! 参照するだけなので、呼び出し先の「本体」が変わっても Blob は再利用できる
//! (blaze の template と同じ発想。キャッシュキーにはシグネチャしか入れない)。

use std::collections::{HashMap, HashSet};

use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    self, AbiParam, Block, ExtFuncData, ExternalName, FuncRef, InstBuilder, MemFlagsData, Signature,
    StackSlot, StackSlotData, StackSlotKind, TrapCode, UserExternalName, UserFuncName, Value, types,
};
use cranelift_codegen::isa::{CallConv, TargetIsa};
use cranelift_codegen::{Context, RelocTarget, binemit::Reloc};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{BinOp, Expr, Lit, Pat, Stmt, UnOp};

use std::sync::Mutex;

use crate::front::{FnSig, Insts, Program, R, Ty, TyEnv, err, intern};

/// ポインタ型 (x86-64 / AArch64 前提)
const PTR: ir::Type = types::I64;

fn mf() -> MemFlagsData {
    MemFlagsData::trusted()
}

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

fn clif_ty(t: Ty) -> Option<ir::Type> {
    Some(match t {
        Ty::Ref(..) | Ty::Struct(_) => PTR,
        t => match t.bits() {
            0 => return None,
            8 => types::I8,
            16 => types::I16,
            32 => types::I32,
            _ => types::I64,
        },
    })
}

fn abi_param(t: Ty) -> AbiParam {
    let p = AbiParam::new(clif_ty(t).unwrap());
    // C ABI 互換のため小さい整数は拡張して渡す
    match t.bits() {
        8 | 16 if t.signed() => p.sext(),
        8 | 16 => p.uext(),
        _ => p,
    }
}

fn is_sret(s: &FnSig) -> bool {
    matches!(s.ret, Ty::Struct(_)) && !s.is_main
}

pub fn clif_sig(s: &FnSig, cc: CallConv) -> Signature {
    let mut sig = Signature::new(cc);
    if is_sret(s) {
        sig.params.push(AbiParam::new(PTR));
    }
    for &p in &s.params {
        sig.params.push(abi_param(p));
    }
    if s.is_main {
        // C の int main() として出す
        sig.returns.push(AbiParam::new(types::I32));
    } else if !is_sret(s) && clif_ty(s.ret).is_some() {
        sig.returns.push(abi_param(s.ret));
    }
    sig
}

// ---------------------------------------------------------------------------
// 関数のコンパイル
// ---------------------------------------------------------------------------

/// スレッドごとに使い回す作業領域。毎回 alloc しないことでメモリのピークと
/// アロケータ負荷を抑える。
pub struct Worker {
    ctx: Context,
    fbc: FunctionBuilderContext,
    /// 直前に lowering した関数が要求したインスタンス
    requested: Vec<u32>,
}

impl Worker {
    pub fn new() -> Self {
        Worker {
            ctx: Context::new(),
            fbc: FunctionBuilderContext::new(),
            requested: vec![],
        }
    }

    /// 型検査だけ (Cranelift IR まで作り、機械語は作らない)。LSP の check 用
    /// id は関数かインスタンス。インスタンスなら f は元のジェネリック関数の本体。
    /// 本体の中で要求されたジェネリック関数のインスタンスは insts に積まれる
    /// 返り値はこの関数が要求したインスタンス (検査済みの印と一緒に保存し、次回も要求し直すため)
    pub fn check(&mut self, prog: &Program, insts: &Mutex<Insts>, id: u32, f: &syn::ItemFn, isa: &dyn TargetIsa) -> R<Vec<u32>> {
        self.lower_or_reset(prog, insts, id, f, isa)?;
        Ok(std::mem::take(&mut self.requested))
    }

    pub fn compile(&mut self, prog: &Program, insts: &Mutex<Insts>, id: u32, f: &syn::ItemFn, isa: &dyn TargetIsa) -> R<Blob> {
        self.lower_or_reset(prog, insts, id, f, isa)?;
        self.emit(prog, id, isa)
    }

    /// エラーで lowering を途中で抜けると FunctionBuilderContext が中途半端な状態で残るので、
    /// 作り直してから次の関数に使う (check は 1 つのエラーで止まらず全関数を見るため)
    fn lower_or_reset(&mut self, prog: &Program, insts: &Mutex<Insts>, id: u32, f: &syn::ItemFn, isa: &dyn TargetIsa) -> R<()> {
        let r = self.lower(prog, insts, id, f, isa);
        if r.is_err() {
            self.fbc = FunctionBuilderContext::new();
        }
        r
    }

    fn lower(&mut self, prog: &Program, insts: &Mutex<Insts>, id: u32, f: &syn::ItemFn, isa: &dyn TargetIsa) -> R<()> {
        let (sig, env) = {
            let g = insts.lock().unwrap();
            (g.sig(prog, id).clone(), g.env(prog, id))
        };
        let sig = &sig;
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
                self_sid: env.self_sid,
                sret: None,
                frefs: HashMap::new(),
                lets: crate::infer::infer(prog, f, sig, &env),
                addr_taken: addr_taken(f),
                env,
                insts,
                requested: vec![],
            };
            let r = cx.body(f, sig);
            self.requested = std::mem::take(&mut cx.requested);
            r?;
            cx.b.seal_all_blocks();
            cx.b.finalize(isa.frontend_config());
        }
        Ok(())
    }

    fn emit(&mut self, prog: &Program, id: u32, isa: &dyn TargetIsa) -> R<Blob> {

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

/// `&x` / `&mut x.f` などでアドレスを取られるローカル変数名 (shadowing は無視した過大近似)。
fn addr_taken(f: &syn::ItemFn) -> HashSet<String> {
    struct V(HashSet<String>);
    fn root(e: &Expr) -> Option<String> {
        match e {
            Expr::Path(p) => p.path.get_ident().map(|i| i.to_string()),
            Expr::Field(f) => root(&f.base),
            Expr::Paren(p) => root(&p.expr),
            Expr::Group(g) => root(&g.expr),
            _ => None,
        }
    }
    impl<'ast> Visit<'ast> for V {
        fn visit_expr_reference(&mut self, r: &'ast syn::ExprReference) {
            if let Some(n) = root(&r.expr) {
                self.0.insert(n);
            }
            syn::visit::visit_expr_reference(self, r);
        }
    }
    let mut v = V(HashSet::new());
    v.visit_block(&f.block);
    v.0
}

/// 式の評価結果。Unit / Never のとき値は None。struct のときはアドレス。
type V = (Option<Value>, Ty);

#[derive(Clone, Copy)]
enum Loc {
    /// レジスタ (Cranelift Variable)
    Var(Variable),
    /// スタックスロット
    Slot(StackSlot),
    /// 呼び出し側が用意したメモリ (struct の引数)
    Ptr(Value),
    /// サイズ 0 の型
    Zst,
}

#[derive(Clone, Copy)]
enum Place {
    Var(Variable, Ty),
    /// アドレス + オフセット
    Mem(Value, i32, Ty),
    Zst(Ty),
}

impl Place {
    fn ty(&self) -> Ty {
        match *self {
            Place::Var(_, t) | Place::Mem(_, _, t) | Place::Zst(t) => t,
        }
    }
}

struct Loop {
    cont: Block,
    brk: Block,
    breaks: usize,
}

struct Cx<'a, 'p> {
    b: FunctionBuilder<'a>,
    prog: &'p Program,
    cc: CallConv,
    scopes: Vec<Vec<(String, Loc, Ty)>>,
    loops: Vec<Loop>,
    ret: Ty,
    is_main: bool,
    /// impl の中なら self 型
    self_sid: Option<u32>,
    /// struct を返す関数の、戻り値の書き込み先
    sret: Option<Value>,
    frefs: HashMap<u32, FuncRef>,
    lets: crate::infer::LetTypes,
    addr_taken: HashSet<String>,
    /// `Self` と型パラメータ (インスタンスなら具体型)
    env: TyEnv,
    insts: &'p Mutex<Insts>,
    requested: Vec<u32>,
}

fn is_place_expr(e: &Expr) -> bool {
    match e {
        Expr::Path(_) | Expr::Field(_) => true,
        Expr::Unary(u) => matches!(u.op, UnOp::Deref(_)),
        Expr::Paren(p) => is_place_expr(&p.expr),
        Expr::Group(g) => is_place_expr(&g.expr),
        _ => false,
    }
}

impl Cx<'_, '_> {
    fn same(&self, span: proc_macro2::Span, got: Ty, want: Ty) -> R<()> {
        if got == want || got == Ty::Never {
            Ok(())
        } else {
            err(
                span,
                format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(want), self.prog.tn(got)),
            )
        }
    }

    // --- メモリ操作 ------------------------------------------------------

    fn slot(&mut self, t: Ty) -> StackSlot {
        let (size, align) = self.prog.size_align(t);
        self.b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            size.max(1),
            align.trailing_zeros() as u8,
        ))
    }

    /// 一時領域を確保してそのアドレスを返す
    fn temp(&mut self, t: Ty) -> Value {
        let ss = self.slot(t);
        self.b.ins().stack_addr(PTR, ss, 0)
    }

    fn addr_plus(&mut self, a: Value, off: i32) -> Value {
        if off == 0 { a } else { self.b.ins().iadd_imm_s(a, off as i64) }
    }

    /// struct のコピー (サイズは静的に分かるので展開する)
    fn copy(&mut self, dst: Value, src: Value, t: Ty) {
        let (size, align) = self.prog.size_align(t);
        let chunk = align.min(8);
        let ty = match chunk {
            1 => types::I8,
            2 => types::I16,
            4 => types::I32,
            _ => types::I64,
        };
        let mut off = 0;
        while off < size {
            let v = self.b.ins().load(ty, mf(), src, off as i32);
            self.b.ins().store(mf(), v, dst, off as i32);
            off += chunk;
        }
    }

    fn loc_addr(&mut self, l: Loc) -> Option<Value> {
        match l {
            Loc::Slot(ss) => Some(self.b.ins().stack_addr(PTR, ss, 0)),
            Loc::Ptr(v) => Some(v),
            Loc::Var(_) | Loc::Zst => None,
        }
    }

    fn read(&mut self, p: Place) -> V {
        match p {
            Place::Var(v, t) => (Some(self.b.use_var(v)), t),
            Place::Zst(t) => (None, t),
            Place::Mem(a, o, t @ Ty::Struct(_)) => (Some(self.addr_plus(a, o)), t),
            Place::Mem(a, o, t) => match clif_ty(t) {
                Some(ct) => (Some(self.b.ins().load(ct, mf(), a, o)), t),
                None => (None, t),
            },
        }
    }

    fn write(&mut self, p: Place, v: Option<Value>) {
        let Some(v) = v else { return };
        match p {
            Place::Var(var, _) => self.b.def_var(var, v),
            Place::Zst(_) => {}
            Place::Mem(a, o, t @ Ty::Struct(_)) => {
                let dst = self.addr_plus(a, o);
                self.copy(dst, v, t);
            }
            Place::Mem(a, o, _) => {
                self.b.ins().store(mf(), v, a, o);
            }
        }
    }

    // --- 変数 -----------------------------------------------------------

    /// ローカル変数を作る。v はスカラー値か、struct ならコピー元のアドレス。
    fn bind(&mut self, name: String, ty: Ty, v: Option<Value>) {
        let loc = match ty {
            Ty::Struct(_) => {
                let ss = self.slot(ty);
                let dst = self.b.ins().stack_addr(PTR, ss, 0);
                if let Some(v) = v {
                    self.copy(dst, v, ty);
                }
                Loc::Slot(ss)
            }
            _ => match clif_ty(ty) {
                None => Loc::Zst,
                Some(_) if self.addr_taken.contains(&name) => {
                    let ss = self.slot(ty);
                    if let Some(v) = v {
                        self.b.ins().stack_store(PTR, v, ss, 0);
                    }
                    Loc::Slot(ss)
                }
                Some(ct) => {
                    let var = self.b.declare_var(ct);
                    if let Some(v) = v {
                        self.b.def_var(var, v);
                    }
                    Loc::Var(var)
                }
            },
        };
        self.scopes.last_mut().unwrap().push((name, loc, ty));
    }

    fn lookup(&self, name: &str) -> Option<(Loc, Ty)> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|s| s.iter().rev())
            .find(|(n, _, _)| n == name)
            .map(|&(_, l, t)| (l, t))
    }

    fn body(&mut self, f: &syn::ItemFn, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let mut params = self.b.block_params(entry).to_vec();
        if is_sret(sig) {
            self.sret = Some(params.remove(0));
        }
        for ((arg, v), &ty) in f.sig.inputs.iter().zip(params).zip(&sig.params) {
            let name = match arg {
                syn::FnArg::Receiver(_) => "self".to_string(),
                syn::FnArg::Typed(pt) => pat_name(&pt.pat)?,
            };
            if let Ty::Struct(_) = ty {
                // 呼び出し側がコピー済みなので、そのメモリをそのまま使う
                self.scopes.last_mut().unwrap().push((name, Loc::Ptr(v), ty));
            } else {
                self.bind(name, ty, Some(v));
            }
        }
        let (v, t) = self.block(&f.block, Some(self.ret))?;
        if t == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(());
        }
        // エラー位置は末尾式 (なければブロック) にする
        let span = match f.block.stmts.last() {
            Some(Stmt::Expr(e, None)) => e.span(),
            _ => f.block.span(),
        };
        self.same(span, t, self.ret)?;
        self.emit_return(v);
        Ok(())
    }

    fn emit_return(&mut self, v: Option<Value>) {
        if self.is_main {
            let z = self.b.ins().iconst(types::I32, 0);
            self.b.ins().return_(&[z]);
        } else if let Some(dst) = self.sret {
            self.copy(dst, v.unwrap(), self.ret);
            self.b.ins().return_(&[]);
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

    fn block(&mut self, blk: &syn::Block, expected: Option<Ty>) -> R<V> {
        self.scopes.push(vec![]);
        let mut diverged = false;
        let mut result: V = (None, Ty::Unit);
        let n = blk.stmts.len();
        for (i, st) in blk.stmts.iter().enumerate() {
            let last = i + 1 == n;
            match st {
                Stmt::Local(l) if !is_simple_let(&l.pat) => {
                    // 分解する let (`let S { x, y } = s;`): 反駁不能パターンとして束縛だけ行う
                    let Some(init) = &l.init else {
                        return err(l.span(), "`let` without initializer is not supported");
                    };
                    if init.diverge.is_some() {
                        return err(l.span(), "`let ... else` is not supported");
                    }
                    let Some(place) = self.scrutinee(&init.expr)? else {
                        diverged = true;
                        continue;
                    };
                    self.pat_bind(&l.pat, place, BindMode::Move)?;
                }
                Stmt::Local(l) => {
                    let (name, ann) = match &l.pat {
                        Pat::Type(pt) => (pat_name(&pt.pat)?, Some(self.prog.parse_ty(&pt.ty, &self.env)?)),
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
                    self.same(init.expr.span(), t, ty)?;
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
                Stmt::Macro(m) => {
                    let r = self.macro_(&m.mac, None)?;
                    if r.1 == Ty::Never {
                        diverged = true;
                    }
                    if last && m.semi_token.is_none() {
                        result = r;
                    }
                }
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
                Lit::Char(_) => Some(Ty::Char),
                _ => None,
            },
            Expr::Path(p) if p.path.segments.len() == 2 => {
                self.prog.variant_by_path(&p.path, self.self_sid).map(|v| Ty::Struct(v.0))
            }
            Expr::Call(c) if matches!(&*c.func, Expr::Path(p) if p.path.segments.len() == 2) => {
                let Expr::Path(p) = &*c.func else { unreachable!() };
                match self.prog.variant_by_path(&p.path, self.self_sid) {
                    Some((sid, _)) => Some(Ty::Struct(sid)),
                    None => self.callee(&c.func).map(|(_, s)| s.ret),
                }
            }
            Expr::Path(p) => p
                .path
                .get_ident()
                .and_then(|id| self.lookup(&id.to_string()))
                .map(|(_, t)| t),
            Expr::Call(c) => self.callee(&c.func).map(|(_, s)| s.ret).filter(|&t| !self.prog.has_param(t)),
            Expr::Cast(c) => self.prog.parse_ty(&c.ty, &self.env).ok(),
            Expr::MethodCall(mc) => {
                let mut t = self.peek(&mc.receiver)?;
                while let Ty::Ref(_, i) = t {
                    t = *i;
                }
                let Ty::Struct(sid) = t else { return None };
                let name = mc.method.to_string();
                let id = self.prog.method(sid, &name).or_else(|| self.prog.default_method(sid, &name))?;
                Some(self.prog.subst(self.prog.sigs[id as usize].ret, &[Ty::Struct(sid)])).filter(|&t| !self.prog.has_param(t))
            }
            Expr::Paren(p) => self.peek(&p.expr),
            Expr::Group(g) => self.peek(&g.expr),
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => match self.peek(&u.expr)? {
                Ty::Ref(_, i) => Some(*i),
                _ => None,
            },
            Expr::Unary(u) => self.peek(&u.expr),
            Expr::Reference(r) => Some(Ty::Ref(r.mutability.is_some(), intern(self.peek(&r.expr)?))),
            Expr::Field(f) => {
                let mut t = self.peek(&f.base)?;
                while let Ty::Ref(_, i) = t {
                    t = *i;
                }
                let Ty::Struct(sid) = t else { return None };
                self.prog.field(sid, &member_name(&f.member)).map(|(t, _)| t)
            }
            Expr::Struct(s) => self
                .prog
                .struct_by_path(&s.path, self.self_sid)
                .or_else(|| self.prog.variant_by_path(&s.path, self.self_sid).map(|v| v.0))
                .map(Ty::Struct),
            Expr::Binary(b) if is_cmp(&b.op) || matches!(b.op, BinOp::And(_) | BinOp::Or(_)) => {
                Some(Ty::Bool)
            }
            Expr::Binary(b) if matches!(b.op, BinOp::Shl(_) | BinOp::Shr(_)) => self.peek(&b.left),
            Expr::Binary(b) => self.peek(&b.left).or_else(|| self.peek(&b.right)),
            _ => None,
        }
    }

    /// `f` / `S::f` / `Self::f` 形式の呼び出し先
    fn callee(&self, f: &Expr) -> Option<(u32, &FnSig)> {
        let Expr::Path(p) = f else { return None };
        // `<T as Trait>::m` / `<T>::m`
        if let Some(q) = &p.qself {
            let Ok(Ty::Struct(sid)) = self.prog.parse_ty(&q.ty, &self.env) else { return None };
            let m = p.path.segments.last()?.ident.to_string();
            let i = self.prog.method(sid, &m).or_else(|| self.prog.default_method(sid, &m))?;
            return Some((i, &self.prog.sigs[i as usize]));
        }
        if let Some(id) = p.path.get_ident() {
            if self.lookup(&id.to_string()).is_some() {
                return None; // ローカル変数 (関数ポインタは未対応)
            }
        }
        let i = self.prog.fn_by_path(&p.path, self.self_sid)?;
        Some((i, &self.prog.sigs[i as usize]))
    }

    fn fref(&mut self, id: u32, sig: &FnSig) -> FuncRef {
        if let Some(&f) = self.frefs.get(&id) {
            return f;
        }
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

    // --- place (左辺値) ---------------------------------------------------

    fn place(&mut self, e: &Expr) -> R<Place> {
        match e {
            Expr::Paren(p) => self.place(&p.expr),
            Expr::Group(g) => self.place(&g.expr),
            Expr::Path(p) => {
                let Some(id) = p.path.get_ident() else {
                    return err(p.span(), "unsupported path");
                };
                match self.lookup(&id.to_string()) {
                    Some((Loc::Var(v), t)) => Ok(Place::Var(v, t)),
                    Some((Loc::Zst, t)) => Ok(Place::Zst(t)),
                    Some((l, t)) => {
                        let a = self.loc_addr(l).unwrap();
                        Ok(Place::Mem(a, 0, t))
                    }
                    None => err(p.span(), format!("cannot find value `{id}`")),
                }
            }
            Expr::Field(f) => {
                let fname = member_name(&f.member);
                let (a, o, sid) = self.struct_base(&f.base)?;
                match self.prog.field(sid, &fname) {
                    Some((ft, foff)) => Ok(Place::Mem(a, o + foff as i32, ft)),
                    None => err(
                        f.member.span(),
                        format!("no field `{fname}` on type `{}`", self.prog.structs[sid as usize].name),
                    ),
                }
            }
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => {
                let (v, t) = self.expr(&u.expr, None)?;
                match (v, t) {
                    (Some(p), Ty::Ref(_, inner)) => Ok(Place::Mem(p, 0, *inner)),
                    _ => err(u.span(), format!("type `{}` cannot be dereferenced", self.prog.tn(t))),
                }
            }
            _ => err(e.span(), "expression is not assignable"),
        }
    }

    /// フィールドアクセスの土台: 自動 deref しながら struct のアドレスを求める
    fn struct_base(&mut self, base: &Expr) -> R<(Value, i32, u32)> {
        let mut p = if is_place_expr(base) {
            self.place(base)?
        } else {
            match self.expr(base, None)? {
                (Some(a), t @ Ty::Struct(_)) => Place::Mem(a, 0, t),
                (Some(p), Ty::Ref(_, i)) => Place::Mem(p, 0, *i),
                (_, t) => return err(base.span(), format!("type `{}` has no fields", self.prog.tn(t))),
            }
        };
        loop {
            p = match p {
                Place::Mem(a, o, Ty::Struct(sid)) => return Ok((a, o, sid)),
                Place::Mem(a, o, Ty::Ref(_, i)) => {
                    let ptr = self.b.ins().load(PTR, mf(), a, o);
                    Place::Mem(ptr, 0, *i)
                }
                Place::Var(v, Ty::Ref(_, i)) => {
                    let ptr = self.b.use_var(v);
                    Place::Mem(ptr, 0, *i)
                }
                p => {
                    return err(base.span(), format!("type `{}` has no fields", self.prog.tn(p.ty())));
                }
            }
        }
    }

    // --- 式 -------------------------------------------------------------

    fn expr(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        // 期待型がなければ、型推論が後ろの使用から決めた型を使う
        let expected = match expected {
            Some(t) => Some(t),
            None if matches!(e, Expr::Lit(_) | Expr::Struct(_) | Expr::Call(_) | Expr::Path(_)) => {
                self.lets.get(&(e as *const Expr as usize)).copied()
            }
            None => None,
        };
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
            Expr::Path(p) if self.is_variant_path(&p.path) || p.path.segments.len() >= 2 => {
                let Some((sid, vi)) = self.prog.variant_by_path(&p.path, self.self_sid) else {
                    return self.path_value(p, expected);
                };
                if self.prog.structs[sid as usize].variants[vi].kind != crate::front::VariantKind::Unit {
                    return err(p.span(), "this variant has fields: use `E::V(..)` or `E::V { .. }`");
                }
                self.variant_value(sid, vi, &[], expected)
            }
            Expr::Match(m) => self.match_(m, expected),
            Expr::Path(_) | Expr::Field(_) => {
                let p = self.place(e)?;
                Ok(self.read(p))
            }
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => {
                let p = self.place(e)?;
                Ok(self.read(p))
            }
            Expr::Unary(u) => self.unary(u, expected),
            Expr::Reference(r) => self.reference(r, expected),
            Expr::Struct(s) => self.struct_lit(s, expected),
            Expr::Binary(b) => self.binary(b, expected),
            Expr::Assign(a) => {
                let p = self.place(&a.left)?;
                let t = p.ty();
                let (v, rt) = self.expr(&a.right, Some(t))?;
                if rt == Ty::Never {
                    return Ok((None, Ty::Never));
                }
                self.same(a.right.span(), rt, t)?;
                self.write(p, v);
                Ok((None, Ty::Unit))
            }
            Expr::If(i) => self.if_(i, expected),
            Expr::While(w) if matches!(&*w.cond, Expr::Let(_)) => self.while_let(w),
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
                self.same(w.cond.span(), ct, Ty::Bool)?;
                match c {
                    Some(c) => {
                        self.b.ins().brif(c, body, &[], exit, &[]);
                    }
                    None => {
                        self.b.ins().trap(TrapCode::unwrap_user(1));
                    }
                }
                self.b.switch_to_block(body);
                self.loops.push(Loop { cont: header, brk: exit, breaks: 0 });
                let (_, bt) = self.block(&w.body, Some(Ty::Unit))?;
                self.loops.pop();
                if bt != Ty::Never {
                    self.same(w.body.span(), bt, Ty::Unit)?;
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
                self.loops.push(Loop { cont: body, brk: exit, breaks: 0 });
                let (_, bt) = self.block(&l.body, Some(Ty::Unit))?;
                let lp = self.loops.pop().unwrap();
                if bt != Ty::Never {
                    self.same(l.body.span(), bt, Ty::Unit)?;
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
                        self.same(e.span(), t, ret)?;
                        v
                    }
                    None => {
                        self.same(r.span(), Ty::Unit, ret)?;
                        None
                    }
                };
                self.emit_return(v);
                Ok(self.dead())
            }
            Expr::Call(c) => self.call(c, expected),
            Expr::MethodCall(mc) => self.method_call(mc),
            Expr::Cast(c) => {
                let to = self.prog.parse_ty(&c.ty, &self.env)?;
                let hint = self.peek(&c.expr).or(Some(Ty::I32));
                let (v, from) = self.expr(&c.expr, hint)?;
                if from == Ty::Never {
                    return Ok((None, Ty::Never));
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
                // 整数 ⇔ char / bool → 整数 (char には u8 からだけ)
                let ok = (to.is_int() && (from.is_int() || matches!(from, Ty::Bool | Ty::Char)))
                    || (to == Ty::Char && matches!(from, Ty::U8 | Ty::Char));
                if !ok {
                    return err(
                        c.span(),
                        format!("unsupported cast `{}` as `{}`", self.prog.tn(from), self.prog.tn(to)),
                    );
                }
                Ok((Some(self.convert(v.unwrap(), from, to)), to))
            }
            Expr::Macro(m) => self.macro_(&m.mac, expected),
            Expr::Try(t) => self.try_(t),
            _ => err(e.span(), "unsupported expression"),
        }
    }

    fn reference(&mut self, r: &syn::ExprReference, expected: Option<Ty>) -> R<V> {
        let m = r.mutability.is_some();
        if is_place_expr(&r.expr) {
            return match self.place(&r.expr)? {
                Place::Mem(a, o, t) => Ok((Some(self.addr_plus(a, o)), Ty::Ref(m, intern(t)))),
                Place::Zst(t) => {
                    let a = self.temp(t);
                    Ok((Some(a), Ty::Ref(m, intern(t))))
                }
                Place::Var(..) => err(r.span(), "internal: borrowed local is not in memory"),
            };
        }
        // 一時値への参照: 一時領域に置いてそのアドレスを取る
        let inner_exp = match expected {
            Some(Ty::Ref(_, i)) => Some(*i),
            _ => None,
        };
        let (v, t) = self.expr(&r.expr, inner_exp)?;
        if t == Ty::Never {
            return Ok((None, Ty::Never));
        }
        let a = match (v, t) {
            (Some(a), Ty::Struct(_)) => a,
            (v, t) => {
                let a = self.temp(t);
                if let Some(v) = v {
                    self.b.ins().store(mf(), v, a, 0);
                }
                a
            }
        };
        Ok((Some(a), Ty::Ref(m, intern(t))))
    }

    fn struct_lit(&mut self, s: &syn::ExprStruct, expected: Option<Ty>) -> R<V> {
        if let Some((sid, vi)) = self.prog.variant_by_path(&s.path, self.self_sid) {
            let v = &self.prog.structs[sid as usize].variants[vi];
            if v.kind != crate::front::VariantKind::Struct {
                return err(s.path.span(), "this variant is not a struct-like variant");
            }
            let mut args = vec![];
            for fv in &s.fields {
                let syn::Member::Named(fid) = &fv.member else {
                    return err(fv.member.span(), "expected a field name");
                };
                let Some(fi) = v.fields.iter().position(|f| f.0 == fid.to_string()) else {
                    return err(fid.span(), format!("variant `{}` has no field named `{fid}`", v.name));
                };
                if args.iter().any(|&(i, _)| i == fi) {
                    return err(fid.span(), format!("field `{fid}` specified more than once"));
                }
                args.push((fi, &fv.expr));
            }
            if args.len() != v.fields.len() || s.rest.is_some() {
                return err(s.span(), format!("missing fields in initializer of `{}`", v.name));
            }
            return self.variant_value(sid, vi, &args, expected);
        }
        let Some(sid) = self.prog.struct_by_path(&s.path, self.self_sid) else {
            return err(s.path.span(), "cannot find struct");
        };
        if s.rest.is_some() || s.dot2_token.is_some() {
            return err(s.span(), "functional update syntax `..` is not supported");
        }
        let fields = self.prog.structs[sid as usize].fields.clone();
        let mut args = vec![];
        for fv in &s.fields {
            let name = match &fv.member {
                syn::Member::Named(fid) => fid.to_string(),
                syn::Member::Unnamed(i) => i.index.to_string(),
            };
            let Some(fi) = fields.iter().position(|f| f.0 == name) else {
                return err(fv.member.span(), format!("struct `{}` has no field named `{name}`", self.prog.structs[sid as usize].name));
            };
            if args.iter().any(|&(i, _)| i == fi) {
                return err(fv.member.span(), format!("field `{name}` specified more than once"));
            }
            args.push((fi, &fv.expr));
        }
        if args.len() != fields.len() {
            return err(s.span(), format!("missing fields in initializer of `{}`", self.prog.structs[sid as usize].name));
        }
        self.adt_value(sid, None, &args, expected, s.span())
    }

    fn call(&mut self, c: &syn::ExprCall, expected: Option<Ty>) -> R<V> {
        if let Expr::Path(p) = &*c.func {
            if let Some((sid, vi)) = self.prog.variant_by_path(&p.path, self.self_sid) {
                let v = &self.prog.structs[sid as usize].variants[vi];
                if v.kind != crate::front::VariantKind::Tuple || v.fields.len() != c.args.len() {
                    return err(c.span(), format!("variant `{}` takes {} fields", v.name, v.fields.len()));
                }
                let args: Vec<(usize, &Expr)> = c.args.iter().enumerate().collect();
                return self.variant_value(sid, vi, &args, expected);
            }
            // タプル構造体の構築 `P(1, 2)`
            if let Some(sid) = self.prog.struct_by_path(&p.path, self.self_sid) {
                if self.lookup(&p.path.segments[0].ident.to_string()).is_none() && self.prog.fn_by_path(&p.path, self.self_sid).is_none() {
                    let n = self.prog.structs[sid as usize].fields.len();
                    if n != c.args.len() {
                        return err(c.span(), format!("this struct takes {n} fields"));
                    }
                    let args: Vec<(usize, &Expr)> = c.args.iter().enumerate().collect();
                    return self.adt_value(sid, None, &args, expected, c.span());
                }
            }
        }
        let Some((id, _)) = self.callee(&c.func) else {
            return err(c.func.span(), "only direct calls to named functions are supported");
        };
        let args: Vec<&Expr> = c.args.iter().collect();
        let tf = match &*c.func {
            Expr::Path(p) => match &p.path.segments.last().unwrap().arguments {
                syn::PathArguments::AngleBracketed(a) => Some(a),
                _ => None,
            },
            _ => None,
        };
        self.call_fn(id, None, &args, tf, c.span())
    }

    /// `recv.m(args)`: レシーバを自動 deref して struct を見つけ、
    /// メソッドの self 型に合わせて自動参照 (`&self` / `&mut self`) かコピー (`self`) で渡す
    fn method_call(&mut self, mc: &syn::ExprMethodCall) -> R<V> {
        let mut p = if is_place_expr(&mc.receiver) {
            self.place(&mc.receiver)?
        } else {
            match self.expr(&mc.receiver, None)? {
                (_, Ty::Never) => return Ok((None, Ty::Never)),
                (Some(a), t @ Ty::Struct(_)) => Place::Mem(a, 0, t),
                (Some(v), Ty::Ref(_, i)) => Place::Mem(v, 0, *i),
                (_, t) => {
                    return err(mc.method.span(), format!("no method `{}` on type `{}`", mc.method, self.prog.tn(t)));
                }
            }
        };
        let (a, o, sid) = loop {
            p = match p {
                Place::Mem(a, o, Ty::Struct(sid)) => break (a, o, sid),
                Place::Mem(a, o, Ty::Ref(_, i)) => {
                    let ptr = self.b.ins().load(PTR, mf(), a, o);
                    Place::Mem(ptr, 0, *i)
                }
                Place::Var(v, Ty::Ref(_, i)) => {
                    let ptr = self.b.use_var(v);
                    Place::Mem(ptr, 0, *i)
                }
                p => {
                    let t = self.prog.tn(p.ty());
                    return err(mc.method.span(), format!("no method `{}` on type `{t}`", mc.method));
                }
            }
        };
        let name = mc.method.to_string();
        let sname = self.prog.structs[sid as usize].name.clone();
        // 固有メソッド / trait 実装のメソッド → なければ実装している trait のデフォルトメソッド
        let Some(id) = self.prog.method(sid, &name).or_else(|| self.prog.default_method(sid, &name)) else {
            return err(mc.method.span(), format!("no method named `{name}` found for `{sname}`"));
        };
        if !self.prog.sigs[id as usize].has_self {
            return err(
                mc.method.span(),
                format!("`{sname}::{name}` is an associated function, not a method"),
            );
        }
        // struct のアドレスを渡す: `&self` ならそのまま、`self` なら call_fn がコピーする
        let recv = self.addr_plus(a, o);
        let st = Ty::Struct(sid);
        let recv_ty = match self.prog.sigs[id as usize].params[0] {
            Ty::Ref(m, _) => Ty::Ref(m, intern(st)),
            _ => st,
        };
        let args: Vec<&Expr> = mc.args.iter().collect();
        self.call_fn(id, Some((recv, recv_ty)), &args, mc.turbofish.as_ref(), mc.span())
    }

    /// 型引数を解いて、呼ぶべき関数 (インスタンス / trait 実装) とそのシグネチャを決める
    /// turbofish (`f::<A, B>`) で決まる型パラメータ
    fn turbofish_sol(&self, sig: &FnSig, turbofish: Option<&syn::AngleBracketedGenericArguments>) -> R<Vec<Option<Ty>>> {
        let mut sol: Vec<Option<Ty>> = vec![None; sig.generics.len()];
        // trait メソッドは先頭が Self なので、turbofish はその後ろの型パラメータに当たる
        let first = sig.trait_of.is_some() as usize;
        if let Some(tf) = turbofish {
            for (i, a) in tf.args.iter().enumerate() {
                let syn::GenericArgument::Type(t) = a else {
                    return err(a.span(), "only type arguments are supported");
                };
                if let Some(slot) = sol.get_mut(first + i) {
                    *slot = Some(self.prog.parse_ty(t, &self.env)?);
                }
            }
        }
        Ok(sol)
    }

    fn resolve_call(&mut self, id: u32, arg_tys: &[Ty], mut sol: Vec<Option<Ty>>, span: proc_macro2::Span) -> R<(u32, FnSig)> {
        let sig = self.prog.sigs[id as usize].clone();
        if !sig.is_generic() {
            return Ok((id, sig));
        }
        for (&pt, &at) in sig.params.iter().zip(arg_tys) {
            if !self.prog.match_ty(pt, at, &mut sol) {
                return err(span, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(pt), self.prog.tn(at)));
            }
        }
        let Some(args) = sol.iter().copied().collect::<Option<Vec<Ty>>>() else {
            let i = sol.iter().position(|s| s.is_none()).unwrap();
            return err(span, format!("cannot infer type parameter `{}`", sig.generics[i].0));
        };
        // trait の関数を `Tr::m(&x)` で呼んだら、実装があればそちらへ静的ディスパッチ
        if sig.trait_of.is_some() {
            if let Ty::Struct(sid) = args[0] {
                let m = self.prog.names[id as usize].rsplit("::").next().unwrap().to_string();
                if let Some(imp) = self.prog.method(sid, &m) {
                    return Ok((imp, self.prog.sigs[imp as usize].clone()));
                }
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

    fn call_fn(
        &mut self,
        id: u32,
        recv: Option<(Value, Ty)>,
        args: &[&Expr],
        turbofish: Option<&syn::AngleBracketedGenericArguments>,
        span: proc_macro2::Span,
    ) -> R<V> {
        let sig0 = self.prog.sigs[id as usize].clone();
        let skip = recv.is_some() as usize;
        if sig0.params.len() != args.len() + skip {
            return err(
                span,
                format!("expected {} arguments, found {}", sig0.params.len() - skip, args.len()),
            );
        }
        // 実引数を先に評価する (ジェネリックなら型を見てから呼び先を決める)。
        // 型パラメータは turbofish → 前の引数の順に分かった分だけ、後の引数の期待型に使う
        let mut sol = self.turbofish_sol(&sig0, turbofish)?;
        let mut vals: Vec<(Value, Ty, proc_macro2::Span)> = vec![];
        if let Some((r, rt)) = recv {
            self.prog.match_ty(sig0.params[0], rt, &mut sol);
            vals.push((r, rt, span));
        }
        for (a, &pt) in args.iter().zip(&sig0.params[skip..]) {
            let exp = Some(self.prog.partial_subst(pt, &sol)).filter(|&t| !self.prog.has_param(t));
            let (v, t) = self.expr(a, exp)?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            self.prog.match_ty(pt, t, &mut sol);
            vals.push((v.unwrap(), t, a.span()));
        }
        let arg_tys: Vec<Ty> = vals.iter().map(|v| v.1).collect();
        let sol0 = self.turbofish_sol(&sig0, turbofish)?;
        let (target, sig) = self.resolve_call(id, &arg_tys, sol0, span)?;
        let mut cv = vec![];
        let sret = if is_sret(&sig) {
            let a = self.temp(sig.ret);
            cv.push(a);
            Some(a)
        } else {
            None
        };
        for ((v, t, sp), &pt) in vals.into_iter().zip(&sig.params) {
            self.same(sp, t, pt)?;
            self.push_arg(&mut cv, v, pt);
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

    fn push_arg(&mut self, vals: &mut Vec<Value>, v: Value, pt: Ty) {
        if let Ty::Struct(_) = pt {
            // 値渡し: 呼び出し側でコピーしてから渡す
            let tmp = self.temp(pt);
            self.copy(tmp, v, pt);
            vals.push(tmp);
        } else {
            vals.push(v);
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
                // 負のリテラル (パターンの `-1` など) も来るので i128 で読む
                let n: i128 = i.base10_parse().map_err(|e| e.to_string())?;
                let v = self.b.ins().iconst(clif_ty(ty).unwrap(), n as u64 as i64);
                Ok((Some(v), ty))
            }
            Lit::Bool(b) => Ok((Some(self.b.ins().iconst(types::I8, b.value as i64)), Ty::Bool)),
            Lit::Byte(b) => Ok((Some(self.b.ins().iconst(types::I8, b.value() as i64)), Ty::U8)),
            Lit::Char(c) => Ok((Some(self.b.ins().iconst(types::I32, c.value() as i64)), Ty::Char)),
            _ => err(l.span(), "unsupported literal"),
        }
    }

    fn unary(&mut self, u: &syn::ExprUnary, expected: Option<Ty>) -> R<V> {
        let (v, t) = self.expr(&u.expr, expected)?;
        let Some(v) = v else { return Ok((None, t)) };
        match u.op {
            UnOp::Neg(_) if t.is_int() && t.signed() => Ok((Some(self.b.ins().ineg(v)), t)),
            UnOp::Not(_) if t == Ty::Bool => Ok((Some(self.b.ins().bxor_imm_u(v, 1)), t)),
            UnOp::Not(_) if t.is_int() => Ok((Some(self.b.ins().bnot(v)), t)),
            _ => err(u.span(), format!("unsupported unary operator for `{}`", self.prog.tn(t))),
        }
    }

    fn binary(&mut self, b: &syn::ExprBinary, expected: Option<Ty>) -> R<V> {
        use BinOp::*;
        // 短絡評価
        if let And(_) | Or(_) = b.op {
            let (l, lt) = self.expr(&b.left, Some(Ty::Bool))?;
            self.same(b.left.span(), lt, Ty::Bool)?;
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
            self.same(b.right.span(), rt, Ty::Bool)?;
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
            let p = self.place(&b.left)?;
            let t = p.ty();
            let (l, _) = self.read(p);
            let rhint = if matches!(op, Shl(_) | Shr(_)) { self.peek(&b.right) } else { Some(t) };
            let (r, rt) = self.expr(&b.right, rhint.or(Some(t)))?;
            let Some(r) = r else { return Ok((None, Ty::Never)) };
            let Some(l) = l else { return err(b.span(), "unsupported operand") };
            let v = self.arith(b, &op, l, t, r, rt)?;
            self.write(p, Some(v));
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
            self.same(b.right.span(), rt, lt)?;
            if !(lt.is_int() || matches!(lt, Ty::Bool | Ty::Char)) {
                return err(b.span(), format!("comparison of `{}` is not supported", self.prog.tn(lt)));
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
        self.same(b.right.span(), rt, lt)?;
        let bool_ok = matches!(op, BitAnd(_) | BitOr(_) | BitXor(_));
        if !(lt.is_int() || (lt == Ty::Bool && bool_ok)) {
            return err(b.span(), format!("unsupported operator for `{}`", self.prog.tn(lt)));
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
        if let Expr::Let(l) = &*i.cond {
            return self.if_let(i, l, expected);
        }
        let (c, ct) = self.expr(&i.cond, Some(Ty::Bool))?;
        self.same(i.cond.span(), ct, Ty::Bool)?;
        let Some(c) = c else { return Ok((None, Ty::Never)) };
        let then_blk = self.b.create_block();
        let else_blk = self.b.create_block();
        let merge = self.b.create_block();
        self.b.ins().brif(c, then_blk, &[], else_blk, &[]);

        let mut result_ty: Option<Ty> = None;
        let mut param: Option<Value> = None;

        self.b.switch_to_block(then_blk);
        let (tv, tt) = self.block(&i.then_branch, expected)?;
        self.finish_arm(tv, tt, merge, &mut result_ty, &mut param, i.then_branch.span())?;

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
            Some(rt) => self.same(span, t, rt)?,
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

#[derive(Clone, Copy, PartialEq)]
enum BindMode {
    /// 値として束縛 (struct / enum はコピー)
    Move,
    /// 参照を match したときのデフォルト束縛モード: フィールドへの参照として束縛
    Ref(bool),
}

impl Cx<'_, '_> {
    // --- enum の値 ------------------------------------------------------

    /// 一時領域にタグと各フィールドを書いて enum の値を作る。args は (フィールド番号, 式)
    fn variant_value(&mut self, sid: u32, vi: usize, args: &[(usize, &Expr)], expected: Option<Ty>) -> R<V> {
        self.adt_value(sid, Some(vi), args, expected, proc_macro2::Span::call_site())
    }

    /// struct (vi = None) / enum のバリアント (vi = Some) の値を一時領域に作る。args は (フィールド番号, 式)。
    /// ジェネリックな定義なら、期待型 → 実引数の型 の順にインスタンスを決める
    fn adt_value(&mut self, base: u32, vi: Option<usize>, args: &[(usize, &Expr)], expected: Option<Ty>, span: proc_macro2::Span) -> R<V> {
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
            vals.push((fi, v, vt, e.span()));
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
            self.same(sp, vt, fty)?;
            self.write(Place::Mem(a, off as i32, fty), v);
        }
        Ok((Some(a), t))
    }

    // --- panic / マクロ / `?` ---------------------------------------------

    /// panic する: `#[panic_handler]` があればそれを呼び (戻らない)、なければ trap する
    fn panic_call(&mut self) -> V {
        if let Some(h) = self.prog.panic_handler {
            let sig = self.prog.sigs[h as usize].clone();
            let info_ty = match sig.params.first() {
                Some(Ty::Ref(_, t)) => **t,
                _ => Ty::U8,
            };
            let info = self.temp(info_ty);
            let f = self.fref(h, &sig);
            self.b.ins().call(f, &[info]);
        }
        self.b.ins().trap(TrapCode::unwrap_user(3));
        self.dead()
    }

    fn macro_args(&self, mac: &syn::Macro) -> R<Vec<Expr>> {
        use syn::parse::Parser;
        let p = syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated;
        p.parse2(mac.tokens.clone()).map(|v| v.into_iter().collect()).or_else(|e| err(mac.span(), e))
    }

    /// 対応しているマクロ: panic! / unreachable! / todo! / unimplemented! / assert! / assert_eq! /
    /// assert_ne! (と debug_ 版) / matches!。メッセージの書式化はしない
    fn macro_(&mut self, mac: &syn::Macro, expected: Option<Ty>) -> R<V> {
        let name = mac.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
        match name.as_str() {
            "panic" | "unreachable" | "todo" | "unimplemented" => Ok(self.panic_call()),
            "assert" | "debug_assert" => {
                let args = self.macro_args(mac)?;
                let Some(c) = args.first() else { return err(mac.span(), "assert! needs a condition") };
                let (cv, ct) = self.expr(c, Some(Ty::Bool))?;
                self.same(c.span(), ct, Ty::Bool)?;
                if let Some(cv) = cv {
                    self.panic_unless(cv);
                }
                Ok((None, Ty::Unit))
            }
            "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne" => {
                let args = self.macro_args(mac)?;
                if args.len() < 2 {
                    return err(mac.span(), format!("{name}! needs two arguments"));
                }
                let hint = self.peek(&args[0]).or_else(|| self.peek(&args[1]));
                let (l, lt) = self.expr(&args[0], hint)?;
                let (r, rt) = self.expr(&args[1], Some(lt))?;
                self.same(args[1].span(), rt, lt)?;
                let (Some(l), Some(r)) = (l, r) else { return Ok((None, Ty::Unit)) };
                if !(lt.is_int() || matches!(lt, Ty::Bool | Ty::Char)) {
                    return err(mac.span(), format!("{name}! is only supported for integers, bool and char"));
                }
                let cc = if name.ends_with("_eq") { IntCC::Equal } else { IntCC::NotEqual };
                let c = self.b.ins().icmp(cc, l, r);
                self.panic_unless(c);
                Ok((None, Ty::Unit))
            }
            "matches" => {
                use syn::parse::Parser;
                let parser = |input: syn::parse::ParseStream| -> syn::Result<(Expr, Pat, Option<Expr>)> {
                    let e: Expr = input.parse()?;
                    input.parse::<syn::Token![,]>()?;
                    let p = Pat::parse_multi_with_leading_vert(input)?;
                    let g = if input.peek(syn::Token![if]) {
                        input.parse::<syn::Token![if]>()?;
                        Some(input.parse()?)
                    } else {
                        None
                    };
                    let _ = input.parse::<Option<syn::Token![,]>>();
                    Ok((e, p, g))
                };
                let (e, pat, guard) = parser.parse2(mac.tokens.clone()).or_else(|er| err(mac.span(), er))?;
                let Some(place) = self.scrutinee(&e)? else { return Ok((None, Ty::Never)) };
                let no = self.b.create_block();
                let merge = self.b.create_block();
                let res = self.b.append_block_param(merge, types::I8);
                self.pat_test(&pat, place, no)?;
                self.scopes.push(vec![]);
                self.pat_bind(&pat, place, BindMode::Move)?;
                if let Some(g) = &guard {
                    let (c, _) = self.expr(g, Some(Ty::Bool))?;
                    if let Some(c) = c {
                        self.require(c, no);
                    }
                }
                self.scopes.pop();
                let one = self.b.ins().iconst(types::I8, 1);
                self.b.ins().jump(merge, &[one.into()]);
                self.b.switch_to_block(no);
                let zero = self.b.ins().iconst(types::I8, 0);
                self.b.ins().jump(merge, &[zero.into()]);
                self.b.switch_to_block(merge);
                let _ = expected;
                Ok((Some(res), Ty::Bool))
            }
            _ => err(mac.span(), format!("macro `{name}!` is not supported")),
        }
    }

    /// cond が偽なら panic する
    fn panic_unless(&mut self, cond: Value) {
        let ok = self.b.create_block();
        let bad = self.b.create_block();
        self.b.ins().brif(cond, ok, &[], bad, &[]);
        self.b.switch_to_block(bad);
        self.panic_call();
        self.b.ins().trap(TrapCode::unwrap_user(3));
        self.b.switch_to_block(ok);
    }

    fn store_tag(&mut self, a: Value, inst: u32, vi: usize) {
        let sd = &self.prog.structs[inst as usize];
        let (tag, discr) = (sd.tag, sd.variants[vi].discr);
        let tv = self.b.ins().iconst(clif_ty(tag).unwrap(), discr);
        self.b.ins().store(mf(), tv, a, 0);
    }

    /// `e?`: Option なら None、Result なら Err(e) をそのまま return する
    fn try_(&mut self, t: &syn::ExprTry) -> R<V> {
        let Some(place) = self.scrutinee(&t.expr)? else { return Ok((None, Ty::Never)) };
        let (place, _) = self.pat_deref(place, BindMode::Move);
        let Ty::Struct(inst) = place.ty() else {
            return err(t.span(), format!("the `?` operator cannot be applied to type `{}`", self.prog.tn(place.ty())));
        };
        let family = self.prog.structs[self.prog.base_of(inst) as usize].name.clone();
        let (okn, errn) = match family.as_str() {
            "Option" => ("Some", "None"),
            "Result" => ("Ok", "Err"),
            _ => return err(t.span(), format!("the `?` operator cannot be applied to type `{}`", self.prog.tn(place.ty()))),
        };
        let vpos = |this: &Self, sid: u32, n: &str| this.prog.structs[sid as usize].variants.iter().position(|v| v.name == n).unwrap();
        let ok_vi = vpos(self, inst, okn);
        let err_vi = vpos(self, inst, errn);
        let fail = self.b.create_block();
        self.test_variant(place, inst, ok_vi, t.span(), fail)?;
        let (_, oty, ooff) = self.prog.structs[inst as usize].variants[ok_vi].fields[0];
        let (okv, _) = self.read(Self::field_place(place, oty, ooff));
        let cont = self.b.create_block();
        let param = clif_ty(oty).map(|ct| self.b.append_block_param(cont, ct));
        match okv {
            Some(v) => self.b.ins().jump(cont, &[v.into()]),
            None => self.b.ins().jump(cont, &[]),
        };
        // 失敗側: 関数の戻り値型の None / Err を作って return
        self.b.switch_to_block(fail);
        let ret = self.ret;
        let rinst = match ret {
            Ty::Struct(r) if self.prog.structs[self.prog.base_of(r) as usize].name == family => r,
            _ => {
                return err(
                    t.span(),
                    format!("the `?` operator can only be used in a function that returns `{family}` (returns `{}`)", self.prog.tn(ret)),
                );
            }
        };
        let a = self.temp(ret);
        let r_err = vpos(self, rinst, errn);
        self.store_tag(a, rinst, r_err);
        if family == "Result" {
            let (_, ety, eoff) = self.prog.structs[inst as usize].variants[err_vi].fields[0];
            let (_, rety, reoff) = self.prog.structs[rinst as usize].variants[r_err].fields[0];
            self.same(t.span(), ety, rety)?;
            let (ev, _) = self.read(Self::field_place(place, ety, eoff));
            self.write(Place::Mem(a, reoff as i32, rety), ev);
        }
        self.emit_return(Some(a));
        self.b.switch_to_block(cont);
        Ok((param, oty))
    }

    /// 1 セグメントのパスがローカル変数でなく enum のバリアント (`None` など) か
    fn is_variant_path(&self, p: &syn::Path) -> bool {
        if let Some(id) = p.get_ident() {
            if self.lookup(&id.to_string()).is_some() {
                return false;
            }
        }
        self.prog.variant_by_path(p, self.self_sid).is_some()
    }

    /// パターンのパスのバリアントを、match 対象の型 t (インスタンス) に合わせて引く
    fn variant_on(&self, p: &syn::Path, t: Ty) -> Option<(u32, usize)> {
        let (sid, vi) = self.prog.variant_by_path(p, self.self_sid)?;
        match t {
            Ty::Struct(x) if self.prog.base_of(x) == self.prog.base_of(sid) => Some((x, vi)),
            _ => Some((sid, vi)),
        }
    }

    /// 変数でもバリアントでもないパス (定数など)。後で const / 関連定数を足す
    fn path_value(&mut self, p: &syn::ExprPath, _expected: Option<Ty>) -> R<V> {
        err(p.span(), "cannot find value (unsupported path)")
    }

    // --- match / if let / while let --------------------------------------

    /// match 対象を place にする。値なら一時変数に置く。None は発散
    fn scrutinee(&mut self, e: &Expr) -> R<Option<Place>> {
        if is_place_expr(e) {
            return Ok(Some(self.place(e)?));
        }
        let (v, t) = self.expr(e, None)?;
        Ok(match (v, t) {
            (_, Ty::Never) => None,
            (Some(a), Ty::Struct(_)) => Some(Place::Mem(a, 0, t)),
            (Some(v), t) => {
                let var = self.b.declare_var(clif_ty(t).unwrap());
                self.b.def_var(var, v);
                Some(Place::Var(var, t))
            }
            (None, t) => Some(Place::Zst(t)),
        })
    }

    fn match_(&mut self, m: &syn::ExprMatch, expected: Option<Ty>) -> R<V> {
        let Some(place) = self.scrutinee(&m.expr)? else { return Ok((None, Ty::Never)) };
        let merge = self.b.create_block();
        let mut result_ty: Option<Ty> = None;
        let mut param: Option<Value> = None;
        // 各 arm を上から順に試す (判定 → 束縛 → ガード → 本体)
        for arm in &m.arms {
            let next = self.b.create_block();
            self.pat_test(&arm.pat, place, next)?;
            self.scopes.push(vec![]);
            self.pat_bind(&arm.pat, place, BindMode::Move)?;
            if let Some((_, g)) = &arm.guard {
                let (c, ct) = self.expr(g, Some(Ty::Bool))?;
                self.same(g.span(), ct, Ty::Bool)?;
                if let Some(c) = c {
                    self.require(c, next);
                }
            }
            let (v, t) = self.expr(&arm.body, result_ty.or(expected))?;
            self.scopes.pop();
            self.finish_arm(v, t, merge, &mut result_ty, &mut param, arm.body.span())?;
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

    fn if_let(&mut self, i: &syn::ExprIf, l: &syn::ExprLet, expected: Option<Ty>) -> R<V> {
        let Some(place) = self.scrutinee(&l.expr)? else { return Ok((None, Ty::Never)) };
        let else_blk = self.b.create_block();
        let merge = self.b.create_block();
        let mut result_ty: Option<Ty> = None;
        let mut param: Option<Value> = None;
        self.pat_test(&l.pat, place, else_blk)?;
        self.scopes.push(vec![]);
        self.pat_bind(&l.pat, place, BindMode::Move)?;
        let (tv, tt) = self.block(&i.then_branch, expected)?;
        self.scopes.pop();
        self.finish_arm(tv, tt, merge, &mut result_ty, &mut param, i.then_branch.span())?;
        self.b.switch_to_block(else_blk);
        let (ev, et) = match &i.else_branch {
            Some((_, e)) => self.expr(e, result_ty.or(expected))?,
            None => (None, Ty::Unit),
        };
        self.finish_arm(ev, et, merge, &mut result_ty, &mut param, i.span())?;
        self.b.switch_to_block(merge);
        Ok(match result_ty {
            None => (None, Ty::Never),
            Some(t) => (param, t),
        })
    }

    fn while_let(&mut self, w: &syn::ExprWhile) -> R<V> {
        if w.label.is_some() {
            return err(w.span(), "loop labels are not supported");
        }
        let Expr::Let(l) = &*w.cond else { unreachable!() };
        let header = self.b.create_block();
        let exit = self.b.create_block();
        self.b.ins().jump(header, &[]);
        self.b.switch_to_block(header);
        // 毎回 match 対象を評価し直す
        let Some(place) = self.scrutinee(&l.expr)? else { return Ok((None, Ty::Never)) };
        self.pat_test(&l.pat, place, exit)?;
        self.scopes.push(vec![]);
        self.pat_bind(&l.pat, place, BindMode::Move)?;
        self.loops.push(Loop { cont: header, brk: exit, breaks: 0 });
        let (_, bt) = self.block(&w.body, Some(Ty::Unit))?;
        self.loops.pop();
        self.scopes.pop();
        if bt != Ty::Never {
            self.same(w.body.span(), bt, Ty::Unit)?;
            self.b.ins().jump(header, &[]);
        } else {
            self.b.ins().trap(TrapCode::unwrap_user(1));
        }
        self.b.switch_to_block(exit);
        Ok((None, Ty::Unit))
    }

    fn no_variant_msg(&self, p: &syn::Path) -> String {
        let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
        match segs.as_slice() {
            [t, v] => format!("no variant named `{v}` in `{t}`"),
            _ => "unsupported path pattern (only enum variants)".into(),
        }
    }

    /// cond が偽なら fail へ。真なら新しいブロックで続ける
    fn require(&mut self, cond: Value, fail: Block) {
        let next = self.b.create_block();
        self.b.ins().brif(cond, next, &[], fail, &[]);
        self.b.switch_to_block(next);
    }

    /// 参照に対して参照でないパターンを当てるときは自動で deref し、束縛モードを Ref にする
    fn pat_deref(&mut self, mut p: Place, mut mode: BindMode) -> (Place, BindMode) {
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

    fn field_place(p: Place, fty: Ty, off: u32) -> Place {
        match p {
            Place::Mem(a, o, _) => Place::Mem(a, o + off as i32, fty),
            _ => unreachable!("aggregates always live in memory"),
        }
    }

    /// パターンの判定値 (リテラル / 範囲の端) を place の型で作る
    fn pat_const(&mut self, e: &Expr, t: Ty) -> R<Value> {
        let Expr::Lit(l) = e else {
            return err(e.span(), "only literals are supported in range patterns");
        };
        let (v, vt) = self.lit(&l.lit, Some(t))?;
        self.same(l.span(), vt, t)?;
        Ok(v.unwrap())
    }

    /// enum のタグが vi のバリアントか調べる
    fn test_variant(&mut self, p: Place, sid: u32, vi: usize, span: proc_macro2::Span, fail: Block) -> R<()> {
        if p.ty() != Ty::Struct(sid) {
            let got = self.prog.tn(p.ty());
            return err(span, format!("mismatched types: expected `{got}`, found `{}`", self.prog.tn(Ty::Struct(sid))));
        }
        let sd = &self.prog.structs[sid as usize];
        if sd.variants.len() == 1 {
            return Ok(()); // バリアントが 1 つなら判定不要
        }
        let (tag, discr) = (sd.tag, sd.variants[vi].discr);
        let Place::Mem(a, o, _) = p else { unreachable!() };
        let tv = self.b.ins().load(clif_ty(tag).unwrap(), mf(), a, o);
        let d = self.b.ins().iconst(clif_ty(tag).unwrap(), discr);
        let c = self.b.ins().icmp(IntCC::Equal, tv, d);
        self.require(c, fail);
        Ok(())
    }

    /// パターン列 (`V(a, .., z)`) とフィールド列の対応 (`..` は途中を飛ばす)
    fn tuple_fields<'e>(
        elems: impl Iterator<Item = &'e Pat>,
        nfields: usize,
        span: proc_macro2::Span,
    ) -> R<Vec<(usize, &'e Pat)>> {
        let elems: Vec<&Pat> = elems.collect();
        let rest = elems.iter().position(|p| matches!(p, Pat::Rest(_)));
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
    fn pat_test(&mut self, pat: &Pat, p: Place, fail: Block) -> R<()> {
        match pat {
            Pat::Wild(_) | Pat::Rest(_) => Ok(()),
            // `None` のように unit バリアントに解決される識別子は、束縛ではなくパスのパターン
            Pat::Ident(pi) if unit_variant_ident(self.prog, pi).is_some() => {
                let path = unit_variant_ident(self.prog, pi).unwrap();
                let (p, _) = self.pat_deref(p, BindMode::Move);
                let Some((sid, vi)) = self.variant_on(&path, p.ty()) else { unreachable!() };
                self.test_variant(p, sid, vi, pi.span(), fail)
            }
            Pat::Ident(pi) => match &pi.subpat {
                Some((_, sub)) => self.pat_test(sub, p, fail),
                None => Ok(()),
            },
            Pat::Paren(pp) => self.pat_test(&pp.pat, p, fail),
            Pat::Type(pt) => self.pat_test(&pt.pat, p, fail),
            Pat::Reference(r) => {
                let Ty::Ref(_, _) = p.ty() else {
                    return err(r.span(), format!("mismatched types: `{}` is not a reference", self.prog.tn(p.ty())));
                };
                let (q, _) = self.pat_deref_once(p);
                self.pat_test(&r.pat, q, fail)
            }
            Pat::Or(o) => {
                let ok = self.b.create_block();
                let n = o.cases.len();
                for (i, case) in o.cases.iter().enumerate() {
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
            _ => {
                let (p, _) = self.pat_deref(p, BindMode::Move);
                let t = p.ty();
                match pat {
                    Pat::Lit(l) => {
                        let (v, _) = self.read(p);
                        let (c, ct) = self.lit(&l.lit, Some(t))?;
                        self.same(l.span(), ct, t)?;
                        if !(t.is_int() || t == Ty::Bool) {
                            return err(l.span(), "literal patterns need an integer or bool");
                        }
                        let cond = self.b.ins().icmp(IntCC::Equal, v.unwrap(), c.unwrap());
                        self.require(cond, fail);
                        Ok(())
                    }
                    Pat::Range(r) => {
                        if !t.is_int() {
                            return err(r.span(), "range patterns need an integer");
                        }
                        let (v, _) = self.read(p);
                        let v = v.unwrap();
                        let s = t.signed();
                        if let Some(lo) = &r.start {
                            let lo = self.pat_const(lo, t)?;
                            let cc = if s { IntCC::SignedGreaterThanOrEqual } else { IntCC::UnsignedGreaterThanOrEqual };
                            let c = self.b.ins().icmp(cc, v, lo);
                            self.require(c, fail);
                        }
                        if let Some(hi) = &r.end {
                            let hi = self.pat_const(hi, t)?;
                            let cc = match (&r.limits, s) {
                                (syn::RangeLimits::Closed(_), true) => IntCC::SignedLessThanOrEqual,
                                (syn::RangeLimits::Closed(_), false) => IntCC::UnsignedLessThanOrEqual,
                                (_, true) => IntCC::SignedLessThan,
                                (_, false) => IntCC::UnsignedLessThan,
                            };
                            let c = self.b.ins().icmp(cc, v, hi);
                            self.require(c, fail);
                        }
                        Ok(())
                    }
                    Pat::Path(pp) => {
                        let Some((sid, vi)) = self.variant_on(&pp.path, t) else {
                            return err(pp.span(), self.no_variant_msg(&pp.path));
                        };
                        self.test_variant(p, sid, vi, pp.span(), fail)
                    }
                    Pat::TupleStruct(ts) => {
                        let fields = match self.variant_on(&ts.path, t) {
                            Some((sid, vi)) => {
                                self.test_variant(p, sid, vi, ts.span(), fail)?;
                                self.prog.structs[sid as usize].variants[vi].fields.clone()
                            }
                            None => self.tuple_struct_fields(&ts.path, t, ts.span())?,
                        };
                        for (fi, sub) in Self::tuple_fields(ts.elems.iter(), fields.len(), ts.span())? {
                            let (_, fty, off) = fields[fi];
                            self.pat_test(sub, Self::field_place(p, fty, off), fail)?;
                        }
                        Ok(())
                    }
                    Pat::Struct(ps) => {
                        let fields = self.struct_pat_fields(ps, p, Some(fail))?;
                        for (sub, fp) in fields {
                            self.pat_test(sub, fp, fail)?;
                        }
                        Ok(())
                    }
                    _ => err(pat.span(), "unsupported pattern"),
                }
            }
        }
    }

    /// struct パターン (`P(a, b)` / `S { .. }`) のフィールド。match 対象の型 t のインスタンスで引く
    fn tuple_struct_fields(&self, path: &syn::Path, t: Ty, span: proc_macro2::Span) -> R<Vec<(String, Ty, u32)>> {
        let Some(sid) = self.prog.struct_by_path(path, self.self_sid) else {
            return err(span, self.no_variant_msg(path));
        };
        match t {
            Ty::Struct(x) if self.prog.base_of(x) == self.prog.base_of(sid) => Ok(self.prog.structs[x as usize].fields.clone()),
            _ => err(span, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(t), self.prog.structs[sid as usize].name)),
        }
    }

    fn pat_deref_once(&mut self, p: Place) -> (Place, bool) {
        match p {
            Place::Var(v, Ty::Ref(m, i)) => (Place::Mem(self.b.use_var(v), 0, *i), m),
            Place::Mem(a, o, Ty::Ref(m, i)) => (Place::Mem(self.b.ins().load(PTR, mf(), a, o), 0, *i), m),
            p => (p, false),
        }
    }

    /// `S { a, b: p, .. }` / `E::V { a, .. }` の各フィールドの (サブパターン, place)。
    /// fail があればバリアントの判定もする
    fn struct_pat_fields<'e>(
        &mut self,
        ps: &'e syn::PatStruct,
        p: Place,
        fail: Option<Block>,
    ) -> R<Vec<(&'e Pat, Place)>> {
        let fields: Vec<(String, Ty, u32)> = if let Some((sid, vi)) = self.variant_on(&ps.path, p.ty()) {
            if let Some(fail) = fail {
                self.test_variant(p, sid, vi, ps.span(), fail)?;
            }
            self.prog.structs[sid as usize].variants[vi].fields.clone()
        } else if self.prog.struct_by_path(&ps.path, self.self_sid).is_some() {
            self.tuple_struct_fields(&ps.path, p.ty(), ps.span())?
        } else {
            return err(ps.path.span(), "cannot find struct or variant");
        };
        let mut out = vec![];
        for fp in &ps.fields {
            let syn::Member::Named(fid) = &fp.member else {
                return err(fp.member.span(), "expected a field name");
            };
            let Some(&(_, fty, off)) = fields.iter().find(|f| f.0 == fid.to_string()) else {
                return err(fid.span(), format!("no field `{fid}`"));
            };
            out.push((&*fp.pat, Self::field_place(p, fty, off)));
        }
        if ps.rest.is_none() && out.len() != fields.len() {
            return err(ps.span(), "pattern does not mention all fields (add `..`)");
        }
        Ok(out)
    }

    /// 判定に通った後で、パターン中の名前を束縛する
    fn pat_bind(&mut self, pat: &Pat, p: Place, mode: BindMode) -> R<()> {
        match pat {
            Pat::Wild(_) | Pat::Rest(_) | Pat::Lit(_) | Pat::Range(_) | Pat::Path(_) => Ok(()),
            Pat::Ident(pi) if unit_variant_ident(self.prog, pi).is_some() => Ok(()),
            Pat::Ident(pi) => {
                if let Some((_, sub)) = &pi.subpat {
                    self.pat_bind(sub, p, mode)?;
                }
                let name = pi.ident.to_string();
                let mode = match pi.by_ref {
                    Some(_) => BindMode::Ref(pi.mutability.is_some()),
                    None => mode,
                };
                match mode {
                    BindMode::Move => {
                        let (v, t) = self.read(p);
                        self.bind(name, t, v);
                    }
                    BindMode::Ref(m) => {
                        let Place::Mem(a, o, t) = p else {
                            return err(pi.span(), "cannot bind by reference here");
                        };
                        let addr = self.addr_plus(a, o);
                        self.bind(name, Ty::Ref(m, intern(t)), Some(addr));
                    }
                }
                Ok(())
            }
            Pat::Paren(pp) => self.pat_bind(&pp.pat, p, mode),
            Pat::Type(pt) => self.pat_bind(&pt.pat, p, mode),
            Pat::Reference(r) => {
                let (q, _) = self.pat_deref_once(p);
                self.pat_bind(&r.pat, q, BindMode::Move)
            }
            Pat::Or(o) => {
                if o.cases.iter().any(has_bindings) {
                    return err(o.span(), "bindings inside `|` patterns are not supported yet");
                }
                Ok(())
            }
            Pat::TupleStruct(ts) => {
                let (p, mode) = self.pat_deref(p, mode);
                let fields = match self.variant_on(&ts.path, p.ty()) {
                    Some((sid, vi)) => self.prog.structs[sid as usize].variants[vi].fields.clone(),
                    None => self.tuple_struct_fields(&ts.path, p.ty(), ts.span())?,
                };
                for (fi, sub) in Self::tuple_fields(ts.elems.iter(), fields.len(), ts.span())? {
                    let (_, fty, off) = fields[fi];
                    self.pat_bind(sub, Self::field_place(p, fty, off), mode)?;
                }
                Ok(())
            }
            Pat::Struct(ps) => {
                let (p, mode) = self.pat_deref(p, mode);
                for (sub, fp) in self.struct_pat_fields(ps, p, None)? {
                    self.pat_bind(sub, fp, mode)?;
                }
                Ok(())
            }
            _ => err(pat.span(), "unsupported pattern"),
        }
    }
}

/// `let x` / `let mut x` / `let x: T` / `let _` のような単純な let か
fn is_simple_let(p: &Pat) -> bool {
    match p {
        Pat::Ident(pi) => pi.subpat.is_none() && pi.by_ref.is_none(),
        Pat::Wild(_) => true,
        Pat::Type(pt) => is_simple_let(&pt.pat),
        _ => false,
    }
}

/// `x.f` / `x.0` のフィールド名
fn member_name(m: &syn::Member) -> String {
    match m {
        syn::Member::Named(id) => id.to_string(),
        syn::Member::Unnamed(i) => i.index.to_string(),
    }
}

/// 識別子のパターンが unit バリアント (`None` など) に解決されるなら、そのパス
pub(crate) fn unit_variant_ident(prog: &Program, pi: &syn::PatIdent) -> Option<syn::Path> {
    if pi.by_ref.is_some() || pi.mutability.is_some() || pi.subpat.is_some() {
        return None;
    }
    let path = syn::Path::from(pi.ident.clone());
    let (sid, vi) = prog.variant_by_path(&path, None)?;
    (prog.structs[sid as usize].variants[vi].kind == crate::front::VariantKind::Unit).then_some(path)
}

fn has_bindings(p: &Pat) -> bool {
    match p {
        Pat::Ident(pi) if pi.ident == "None" && pi.subpat.is_none() => false,
        Pat::Ident(_) => true,
        Pat::Paren(pp) => has_bindings(&pp.pat),
        Pat::Reference(r) => has_bindings(&r.pat),
        Pat::Or(o) => o.cases.iter().any(has_bindings),
        Pat::TupleStruct(ts) => ts.elems.iter().any(has_bindings),
        Pat::Struct(ps) => ps.fields.iter().any(|f| has_bindings(&f.pat)),
        _ => false,
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
