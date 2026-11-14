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

use crate::front::{FnSig, Program, R, Ty, err, intern};

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
}

impl Worker {
    pub fn new() -> Self {
        Worker {
            ctx: Context::new(),
            fbc: FunctionBuilderContext::new(),
        }
    }

    /// 型検査だけ (Cranelift IR まで作り、機械語は作らない)。LSP の check 用
    pub fn check(&mut self, prog: &Program, id: u32, f: &syn::ItemFn, isa: &dyn TargetIsa) -> R<()> {
        self.lower(prog, id, f, isa)
    }

    pub fn compile(&mut self, prog: &Program, id: u32, f: &syn::ItemFn, isa: &dyn TargetIsa) -> R<Blob> {
        self.lower(prog, id, f, isa)?;
        self.emit(prog, id, isa)
    }

    fn lower(&mut self, prog: &Program, id: u32, f: &syn::ItemFn, isa: &dyn TargetIsa) -> R<()> {
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
                self_sid: sig.impl_of,
                sret: None,
                frefs: HashMap::new(),
                lets: crate::infer::infer(prog, f, id),
                addr_taken: addr_taken(f),
            };
            cx.body(f, sig)?;
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
                Stmt::Local(l) => {
                    let (name, ann) = match &l.pat {
                        Pat::Type(pt) => (pat_name(&pt.pat)?, Some(self.prog.parse_ty(&pt.ty, self.self_sid)?)),
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
            Expr::Cast(c) => self.prog.parse_ty(&c.ty, self.self_sid).ok(),
            Expr::MethodCall(mc) => {
                let mut t = self.peek(&mc.receiver)?;
                while let Ty::Ref(_, i) = t {
                    t = *i;
                }
                let Ty::Struct(sid) = t else { return None };
                let id = self.prog.method(sid, &mc.method.to_string())?;
                Some(self.prog.sigs[id as usize].ret)
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
                let (Ty::Struct(sid), syn::Member::Named(id)) = (t, &f.member) else { return None };
                self.prog.field(sid, &id.to_string()).map(|(t, _)| t)
            }
            Expr::Struct(s) => self.prog.struct_by_path(&s.path, self.self_sid).map(Ty::Struct),
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
        if let Some(id) = p.path.get_ident() {
            if self.lookup(&id.to_string()).is_some() {
                return None; // ローカル変数 (関数ポインタは未対応)
            }
        }
        let i = self.prog.fn_by_path(&p.path, self.self_sid)?;
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
                let syn::Member::Named(fid) = &f.member else {
                    return err(f.member.span(), "tuple fields are not supported");
                };
                let (a, o, sid) = self.struct_base(&f.base)?;
                match self.prog.field(sid, &fid.to_string()) {
                    Some((ft, foff)) => Ok(Place::Mem(a, o + foff as i32, ft)),
                    None => err(
                        fid.span(),
                        format!("no field `{fid}` on type `{}`", self.prog.structs[sid as usize].name),
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
            Expr::Struct(s) => self.struct_lit(s),
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
            Expr::Call(c) => self.call(c),
            Expr::MethodCall(mc) => self.method_call(mc),
            Expr::Cast(c) => {
                let to = self.prog.parse_ty(&c.ty, self.self_sid)?;
                let hint = self.peek(&c.expr).or(Some(Ty::I32));
                let (v, from) = self.expr(&c.expr, hint)?;
                if from == Ty::Never {
                    return Ok((None, Ty::Never));
                }
                if !to.is_int() || !(from.is_int() || from == Ty::Bool) {
                    return err(
                        c.span(),
                        format!("unsupported cast `{}` as `{}`", self.prog.tn(from), self.prog.tn(to)),
                    );
                }
                Ok((Some(self.convert(v.unwrap(), from, to)), to))
            }
            Expr::Macro(m) => err(m.span(), "macros are not supported yet"),
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

    fn struct_lit(&mut self, s: &syn::ExprStruct) -> R<V> {
        let Some(sid) = self.prog.struct_by_path(&s.path, self.self_sid) else {
            return err(s.path.span(), "cannot find struct");
        };
        if s.rest.is_some() || s.dot2_token.is_some() {
            return err(s.span(), "functional update syntax `..` is not supported");
        }
        let t = Ty::Struct(sid);
        let a = self.temp(t);
        let mut seen = HashSet::new();
        for fv in &s.fields {
            let syn::Member::Named(fid) = &fv.member else {
                return err(fv.member.span(), "tuple fields are not supported");
            };
            let name = fid.to_string();
            let Some((ft, off)) = self.prog.field(sid, &name) else {
                return err(fid.span(), format!("struct `{}` has no field named `{name}`", self.prog.tn(t)));
            };
            if !seen.insert(name.clone()) {
                return err(fid.span(), format!("field `{name}` specified more than once"));
            }
            let (v, vt) = self.expr(&fv.expr, Some(ft))?;
            if vt == Ty::Never {
                return Ok((None, Ty::Never));
            }
            self.same(fv.expr.span(), vt, ft)?;
            self.write(Place::Mem(a, off as i32, ft), v);
        }
        let n = self.prog.structs[sid as usize].fields.len();
        if seen.len() != n {
            return err(s.span(), format!("missing fields in initializer of `{}`", self.prog.tn(t)));
        }
        Ok((Some(a), t))
    }

    fn call(&mut self, c: &syn::ExprCall) -> R<V> {
        let Some((id, _)) = self.callee(&c.func) else {
            return err(c.func.span(), "only direct calls to named functions are supported");
        };
        let args: Vec<&Expr> = c.args.iter().collect();
        self.call_fn(id, None, &args, c.span())
    }

    /// `recv.m(args)`: レシーバを自動 deref して struct を見つけ、
    /// メソッドの self 型に合わせて自動参照 (`&self` / `&mut self`) かコピー (`self`) で渡す
    fn method_call(&mut self, mc: &syn::ExprMethodCall) -> R<V> {
        if mc.turbofish.is_some() {
            return err(mc.span(), "turbofish is not supported");
        }
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
        let Some(id) = self.prog.method(sid, &name) else {
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
        let args: Vec<&Expr> = mc.args.iter().collect();
        self.call_fn(id, Some(recv), &args, mc.span())
    }

    fn call_fn(&mut self, id: u32, recv: Option<Value>, args: &[&Expr], span: proc_macro2::Span) -> R<V> {
        let sig = self.prog.sigs[id as usize].clone();
        let skip = recv.is_some() as usize;
        if sig.params.len() != args.len() + skip {
            return err(
                span,
                format!("expected {} arguments, found {}", sig.params.len() - skip, args.len()),
            );
        }
        let mut vals = vec![];
        let sret = if is_sret(&sig) {
            let a = self.temp(sig.ret);
            vals.push(a);
            Some(a)
        } else {
            None
        };
        if let Some(r) = recv {
            self.push_arg(&mut vals, r, sig.params[0]);
        }
        for (a, &pt) in args.iter().zip(&sig.params[skip..]) {
            let (v, t) = self.expr(a, Some(pt))?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            self.same(a.span(), t, pt)?;
            self.push_arg(&mut vals, v.unwrap(), pt);
        }
        let f = self.fref(id);
        let call = self.b.ins().call(f, &vals);
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
                let n: u128 = i.base10_parse().map_err(|e| e.to_string())?;
                let v = self.b.ins().iconst(clif_ty(ty).unwrap(), n as u64 as i64);
                Ok((Some(v), ty))
            }
            Lit::Bool(b) => Ok((Some(self.b.ins().iconst(types::I8, b.value as i64)), Ty::Bool)),
            Lit::Byte(b) => Ok((Some(self.b.ins().iconst(types::I8, b.value() as i64)), Ty::U8)),
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
            if !(lt.is_int() || lt == Ty::Bool) {
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
