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
    self, AbiParam, Block, ExtFuncData, ExternalName, FuncRef, InstBuilder, MemFlagsData,
    Signature, StackSlot, StackSlotData, StackSlotKind, TrapCode, UserExternalName, UserFuncName,
    Value, types,
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
    /// この関数が参照する無名データ (文字列リテラルなど)。.o では内容で重複を除く
    pub datas: Vec<Vec<u8>>,
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
    /// Blob::datas の index
    Data(u32),
    /// static (Program::consts の index)
    Static(u32),
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
        // 集約型 (struct / enum / 配列 / fat pointer) は値としてはアドレス
        t if t.is_agg() => PTR,
        Ty::Ref(..) | Ty::Ptr(..) => PTR,
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
    s.ret.is_agg() && !s.is_main
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
    /// 直前に lowering した関数の無名データ
    datas: Vec<Vec<u8>>,
}

/// UserExternalName の名前空間: 0 = 関数, 1 = 無名データ, 2 = static
pub const NS_DATA: u32 = 1;
pub const NS_STATIC: u32 = 2;

impl Worker {
    pub fn new() -> Self {
        Worker {
            ctx: Context::new(),
            fbc: FunctionBuilderContext::new(),
            requested: vec![],
            datas: vec![],
        }
    }

    /// 型検査だけ (Cranelift IR まで作り、機械語は作らない)。LSP の check 用
    /// id は関数かインスタンス。インスタンスなら f は元のジェネリック関数の本体。
    /// 本体の中で要求されたジェネリック関数のインスタンスは insts に積まれる
    /// 返り値はこの関数が要求したインスタンス (検査済みの印と一緒に保存し、次回も要求し直すため)
    pub fn check(
        &mut self,
        prog: &Program,
        insts: &Mutex<Insts>,
        id: u32,
        f: &syn::ItemFn,
        isa: &dyn TargetIsa,
    ) -> R<Vec<u32>> {
        self.lower_or_reset(prog, insts, id, f, isa)?;
        Ok(std::mem::take(&mut self.requested))
    }

    pub fn compile(
        &mut self,
        prog: &Program,
        insts: &Mutex<Insts>,
        id: u32,
        f: &syn::ItemFn,
        isa: &dyn TargetIsa,
    ) -> R<Blob> {
        self.lower_or_reset(prog, insts, id, f, isa)?;
        self.emit(prog, id, isa)
    }

    /// エラーで lowering を途中で抜けると FunctionBuilderContext が中途半端な状態で残るので、
    /// 作り直してから次の関数に使う (check は 1 つのエラーで止まらず全関数を見るため)
    fn lower_or_reset(
        &mut self,
        prog: &Program,
        insts: &Mutex<Insts>,
        id: u32,
        f: &syn::ItemFn,
        isa: &dyn TargetIsa,
    ) -> R<()> {
        let r = self.lower(prog, insts, id, f, isa);
        if r.is_err() {
            self.fbc = FunctionBuilderContext::new();
        }
        r
    }

    fn lower(
        &mut self,
        prog: &Program,
        insts: &Mutex<Insts>,
        id: u32,
        f: &syn::ItemFn,
        isa: &dyn TargetIsa,
    ) -> R<()> {
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
                datas: vec![],
                gvs: HashMap::new(),
            };
            let r = cx.body(f, sig);
            self.requested = std::mem::take(&mut cx.requested);
            self.datas = std::mem::take(&mut cx.datas);
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
                    let n = &self.ctx.func.params.user_named_funcs()[reff];
                    match n.namespace {
                        NS_DATA => Target::Data(n.index),
                        NS_STATIC => Target::Static(n.index),
                        _ => Target::Func(n.index),
                    }
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
            datas: std::mem::take(&mut self.datas),
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
    /// 無名データ (内容) と、そのシンボルの GlobalValue
    datas: Vec<Vec<u8>>,
    gvs: HashMap<(u32, u32), ir::GlobalValue>,
}

fn is_place_expr(e: &Expr) -> bool {
    match e {
        Expr::Path(_) | Expr::Field(_) => true,
        Expr::Index(ix) => !matches!(&*ix.index, Expr::Range(_)),
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
                format!(
                    "mismatched types: expected `{}`, found `{}`",
                    self.prog.tn(want),
                    self.prog.tn(got)
                ),
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
        if off == 0 {
            a
        } else {
            self.b.ins().iadd_imm_s(a, off as i64)
        }
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
            Place::Mem(a, o, t) if t.is_agg() => (Some(self.addr_plus(a, o)), t),
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
            Place::Mem(a, o, t) if t.is_agg() => {
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
            t if t.is_agg() => {
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
            if ty.is_agg() {
                // 呼び出し側がコピー済みなので、そのメモリをそのまま使う
                self.scopes
                    .last_mut()
                    .unwrap()
                    .push((name, Loc::Ptr(v), ty));
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
                        Pat::Type(pt) => (
                            pat_name(&pt.pat)?,
                            Some(self.prog.parse_ty(&pt.ty, &self.env)?),
                        ),
                        p => (
                            pat_name(p)?,
                            self.lets.get(&crate::infer::let_key(l)).copied(),
                        ),
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
                    let exp = if last && semi.is_none() {
                        expected
                    } else {
                        None
                    };
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
        // 型推論が記録した型があればそれ (呼び出し・メソッド呼び出し・構築など)
        if let Some(&t) = self.lets.get(&(e as *const Expr as usize)) {
            return Some(t);
        }
        self.peek_syn(e)
    }

    fn peek_syn(&self, e: &Expr) -> Option<Ty> {
        match e {
            Expr::Try(t) => {
                let Ty::Struct(sid) = self.peek(&t.expr)? else {
                    return None;
                };
                let sd = &self.prog.structs[sid as usize];
                sd.variants
                    .iter()
                    .find(|v| v.name == "Some" || v.name == "Ok")
                    .and_then(|v| v.fields.first())
                    .map(|f| f.1)
            }
            Expr::Lit(l) => match &l.lit {
                Lit::Int(i) => Ty::from_suffix(i.suffix()),
                Lit::Bool(_) => Some(Ty::Bool),
                Lit::Byte(_) => Some(Ty::U8),
                Lit::Char(_) => Some(Ty::Char),
                Lit::Str(_) => Some(Ty::Ref(false, intern(Ty::Str))),
                Lit::ByteStr(b) => Some(Ty::Ref(
                    false,
                    intern(Ty::Array(intern(Ty::U8), b.value().len() as u32)),
                )),
                _ => None,
            },
            Expr::Path(p)
                if p.path.get_ident().is_some_and(|id| {
                    self.lookup(&id.to_string()).is_none()
                        && self.prog.const_index.contains_key(&id.to_string())
                }) =>
            {
                let ci = self.prog.const_index[&p.path.get_ident().unwrap().to_string()];
                Some(self.prog.consts[ci as usize].ty)
            }
            Expr::Path(p) if p.path.segments.len() == 2 => self
                .prog
                .variant_by_path(&p.path, self.self_sid)
                .map(|v| Ty::Struct(v.0)),
            Expr::Call(c) if matches!(&*c.func, Expr::Path(p) if p.path.segments.len() == 2) => {
                let Expr::Path(p) = &*c.func else {
                    unreachable!()
                };
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
            Expr::Call(c) => self
                .callee(&c.func)
                .map(|(_, s)| s.ret)
                .filter(|&t| !self.prog.has_param(t)),
            Expr::Cast(c) => self.prog.parse_ty(&c.ty, &self.env).ok(),
            Expr::Index(ix) if !matches!(&*ix.index, Expr::Range(_)) => {
                let mut t = self.peek(&ix.expr)?;
                loop {
                    match t {
                        Ty::Array(e, _) => return Some(*e),
                        Ty::Ref(_, Ty::Slice(e)) => return Some(**e),
                        Ty::Ref(_, i) => t = *i,
                        _ => return None,
                    }
                }
            }
            Expr::Array(a) => {
                let e = a.elems.iter().find_map(|e| self.peek(e))?;
                Some(Ty::Array(intern(e), a.elems.len() as u32))
            }
            Expr::MethodCall(mc) => {
                let mut t = self.peek(&mc.receiver)?;
                while let Ty::Ref(_, i) = t {
                    t = *i;
                }
                let Ty::Struct(sid) = t else { return None };
                let name = mc.method.to_string();
                let id = self
                    .prog
                    .method(sid, &name)
                    .or_else(|| self.prog.default_method(sid, &name))?;
                Some(
                    self.prog
                        .subst(self.prog.sigs[id as usize].ret, &[Ty::Struct(sid)]),
                )
                .filter(|&t| !self.prog.has_param(t))
            }
            Expr::Paren(p) => self.peek(&p.expr),
            Expr::Group(g) => self.peek(&g.expr),
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => match self.peek(&u.expr)? {
                Ty::Ref(_, i) => Some(*i),
                _ => None,
            },
            Expr::Unary(u) => self.peek(&u.expr),
            Expr::Reference(r) => {
                Some(Ty::Ref(r.mutability.is_some(), intern(self.peek(&r.expr)?)))
            }
            Expr::Field(f) => {
                let mut t = self.peek(&f.base)?;
                while let Ty::Ref(_, i) = t {
                    t = *i;
                }
                let Ty::Struct(sid) = t else { return None };
                self.prog
                    .field(sid, &member_name(&f.member))
                    .map(|(t, _)| t)
            }
            Expr::Struct(s) => self
                .prog
                .struct_by_path(&s.path, self.self_sid)
                .or_else(|| {
                    self.prog
                        .variant_by_path(&s.path, self.self_sid)
                        .map(|v| v.0)
                })
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
            let Ok(Ty::Struct(sid)) = self.prog.parse_ty(&q.ty, &self.env) else {
                return None;
            };
            let m = p.path.segments.last()?.ident.to_string();
            let i = self
                .prog
                .method(sid, &m)
                .or_else(|| self.prog.default_method(sid, &m))?;
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
                    None => match self.prog.const_index.get(&id.to_string()) {
                        // static はデータのアドレス
                        Some(&ci) if self.prog.consts[ci as usize].stat.is_some() => {
                            let t = self.prog.consts[ci as usize].ty;
                            let a = self.symbol_addr(NS_STATIC, ci);
                            Ok(Place::Mem(a, 0, t))
                        }
                        _ => err(p.span(), format!("cannot find value `{id}`")),
                    },
                }
            }
            Expr::Field(f) => {
                let fname = member_name(&f.member);
                let (a, o, sid) = self.struct_base(&f.base)?;
                match self.prog.field(sid, &fname) {
                    Some((ft, foff)) => Ok(Place::Mem(a, o + foff as i32, ft)),
                    None => err(
                        f.member.span(),
                        format!(
                            "no field `{fname}` on type `{}`",
                            self.prog.structs[sid as usize].name
                        ),
                    ),
                }
            }
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => {
                let (v, t) = self.expr(&u.expr, None)?;
                match (v, t) {
                    (Some(_), t) if t.is_fat() => err(
                        u.span(),
                        "dereferencing a slice is not supported (index it instead)",
                    ),
                    (Some(p), Ty::Ref(_, inner)) | (Some(p), Ty::Ptr(_, inner)) => {
                        Ok(Place::Mem(p, 0, *inner))
                    }
                    _ => err(
                        u.span(),
                        format!("type `{}` cannot be dereferenced", self.prog.tn(t)),
                    ),
                }
            }
            Expr::Index(ix) if !matches!(&*ix.index, Expr::Range(_)) => {
                let (base, len, elem) = self.indexable(&ix.expr)?;
                let (i, it) = self.expr(&ix.index, Some(Ty::Usize))?;
                let Some(i) = i else {
                    return err(ix.span(), "index diverges");
                };
                self.same(ix.index.span(), it, Ty::Usize)?;
                // 範囲外なら panic (rustc と同じく境界検査する)
                let ok = self.b.ins().icmp(IntCC::UnsignedLessThan, i, len);
                self.panic_unless(ok);
                let addr = self.elem_addr(base, i, elem);
                Ok(Place::Mem(addr, 0, elem))
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
                (Some(a), t) if t.is_agg() => Place::Mem(a, 0, t),
                (Some(p), Ty::Ref(_, i)) => Place::Mem(p, 0, *i),
                (_, t) => {
                    return err(
                        base.span(),
                        format!("type `{}` has no fields", self.prog.tn(t)),
                    );
                }
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
                    return err(
                        base.span(),
                        format!("type `{}` has no fields", self.prog.tn(p.ty())),
                    );
                }
            }
        }
    }

    // --- 式 -------------------------------------------------------------

    /// 式を評価し、期待型への暗黙の変換 (`&[T; N]` → `&[T]`) をする
    fn expr(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let (v, t) = self.expr_inner(e, expected)?;
        if let (Some(want @ Ty::Ref(m2, Ty::Slice(we))), Some(v), Ty::Ref(m1, Ty::Array(ge, n))) =
            (expected, v, t)
        {
            if we == ge && (m1 || !m2) {
                let fat = self.make_fat(v, *n as i64);
                return Ok((Some(fat), want));
            }
        }
        Ok((v, t))
    }

    fn expr_inner(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        // 期待型がなければ、型推論が後ろの使用から決めた型を使う
        let expected = match expected {
            Some(t) => Some(t),
            None if matches!(
                e,
                Expr::Lit(_) | Expr::Struct(_) | Expr::Call(_) | Expr::Path(_)
            ) =>
            {
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
                if self.prog.structs[sid as usize].variants[vi].kind
                    != crate::front::VariantKind::Unit
                {
                    return err(
                        p.span(),
                        "this variant has fields: use `E::V(..)` or `E::V { .. }`",
                    );
                }
                self.variant_value(sid, vi, &[], expected)
            }
            Expr::Match(m) => self.match_(m, expected),
            // const は式そのものを埋め込む
            Expr::Path(p) if self.const_of(p).is_some() => {
                let ci = self.const_of(p).unwrap();
                let c = &self.prog.consts[ci as usize];
                let (t, src) = (c.ty, c.expr.clone());
                let ce: Expr = syn::parse_str(&src).or_else(|er| err(p.span(), er))?;
                let (v, vt) = self.expr(&ce, Some(t)).map_err(|e| {
                    format!("{}:1: in constant `{}`: {e}", p.span().start().line, c.name)
                })?;
                self.same(p.span(), vt, t)?;
                Ok((v, t))
            }
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
                self.loops.push(Loop {
                    cont: header,
                    brk: exit,
                    breaks: 0,
                });
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
                self.loops.push(Loop {
                    cont: body,
                    brk: exit,
                    breaks: 0,
                });
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
                    return err(
                        br.span(),
                        "labeled break / break with value is not supported",
                    );
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
            Expr::Tuple(t) if t.elems.is_empty() => Ok((None, Ty::Unit)),
            Expr::Tuple(t) => self.tuple_lit(t, expected),
            Expr::Array(a) => self.array_lit(a, expected),
            Expr::Repeat(r) => self.repeat_lit(r, expected),
            Expr::Index(ix) if matches!(&*ix.index, Expr::Range(_)) => {
                err(ix.span(), "range indexing needs a reference: `&a[lo..hi]`")
            }
            Expr::Index(_) => {
                let p = self.place(e)?;
                Ok(self.read(p))
            }
            Expr::ForLoop(f) => self.for_(f),
            Expr::MethodCall(mc) => self.method_call(mc),
            Expr::Cast(c) => {
                let to = self.prog.parse_ty(&c.ty, &self.env)?;
                let hint = self.peek(&c.expr).or(Some(Ty::I32));
                let (v, from) = self.expr(&c.expr, hint)?;
                if from == Ty::Never {
                    return Ok((None, Ty::Never));
                }
                // 参照 → 生ポインタ、生ポインタ ⇔ 生ポインタ / usize
                match (from, to) {
                    (Ty::Ref(_, a), Ty::Ptr(_, b))
                        if a == b || matches!(*a, Ty::Array(e, _) if e == b) =>
                    {
                        return Ok((v, to));
                    }
                    (Ty::Ptr(..), Ty::Ptr(..)) => return Ok((v, to)),
                    (Ty::Ptr(..), t) if matches!(t, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
                        return Ok((v, to));
                    }
                    (t, Ty::Ptr(..)) if matches!(t, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
                        return Ok((v, to));
                    }
                    _ => {}
                }
                // フィールドのない enum → 整数: タグ (= 判別値) を読む
                if let Ty::Struct(sid) = from {
                    let sd = &self.prog.structs[sid as usize];
                    if to.is_int() && sd.is_enum && sd.variants.iter().all(|v| v.fields.is_empty())
                    {
                        let tag = sd.tag;
                        let t = self
                            .b
                            .ins()
                            .load(clif_ty(tag).unwrap(), mf(), v.unwrap(), 0);
                        return Ok((Some(self.convert(t, tag, to)), to));
                    }
                }
                // 整数 ⇔ char / bool → 整数 (char には u8 からだけ)
                let ok = (to.is_int() && (from.is_int() || matches!(from, Ty::Bool | Ty::Char)))
                    || (to == Ty::Char && matches!(from, Ty::U8 | Ty::Char));
                if !ok {
                    return err(
                        c.span(),
                        format!(
                            "unsupported cast `{}` as `{}`",
                            self.prog.tn(from),
                            self.prog.tn(to)
                        ),
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
        // `&a[lo..hi]` → スライス (fat pointer)
        if let Expr::Index(ix) = &*r.expr {
            if let Expr::Range(rg) = &*ix.index {
                return self.slice_range(&ix.expr, rg, m);
            }
        }
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
            (Some(a), t) if t.is_agg() => a,
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
                    return err(
                        fid.span(),
                        format!("variant `{}` has no field named `{fid}`", v.name),
                    );
                };
                if args.iter().any(|&(i, _)| i == fi) {
                    return err(
                        fid.span(),
                        format!("field `{fid}` specified more than once"),
                    );
                }
                args.push((fi, &fv.expr));
            }
            if args.len() != v.fields.len() || s.rest.is_some() {
                return err(
                    s.span(),
                    format!("missing fields in initializer of `{}`", v.name),
                );
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
                return err(
                    fv.member.span(),
                    format!(
                        "struct `{}` has no field named `{name}`",
                        self.prog.structs[sid as usize].name
                    ),
                );
            };
            if args.iter().any(|&(i, _)| i == fi) {
                return err(
                    fv.member.span(),
                    format!("field `{name}` specified more than once"),
                );
            }
            args.push((fi, &fv.expr));
        }
        if args.len() != fields.len() {
            return err(
                s.span(),
                format!(
                    "missing fields in initializer of `{}`",
                    self.prog.structs[sid as usize].name
                ),
            );
        }
        self.adt_value(sid, None, &args, expected, s.span())
    }

    fn call(&mut self, c: &syn::ExprCall, expected: Option<Ty>) -> R<V> {
        if let Expr::Path(p) = &*c.func {
            if let Some((sid, vi)) = self.prog.variant_by_path(&p.path, self.self_sid) {
                let v = &self.prog.structs[sid as usize].variants[vi];
                if v.kind != crate::front::VariantKind::Tuple || v.fields.len() != c.args.len() {
                    return err(
                        c.span(),
                        format!("variant `{}` takes {} fields", v.name, v.fields.len()),
                    );
                }
                let args: Vec<(usize, &Expr)> = c.args.iter().enumerate().collect();
                return self.variant_value(sid, vi, &args, expected);
            }
            // タプル構造体の構築 `P(1, 2)`
            if let Some(sid) = self.prog.struct_by_path(&p.path, self.self_sid) {
                if self.lookup(&p.path.segments[0].ident.to_string()).is_none()
                    && self.prog.fn_by_path(&p.path, self.self_sid).is_none()
                {
                    let n = self.prog.structs[sid as usize].fields.len();
                    if n != c.args.len() {
                        return err(c.span(), format!("this struct takes {n} fields"));
                    }
                    let args: Vec<(usize, &Expr)> = c.args.iter().enumerate().collect();
                    return self.adt_value(sid, None, &args, expected, c.span());
                }
            }
        }
        if let Some(r) = self.intrinsic_call(c, expected)? {
            return Ok(r);
        }
        let Some((id, _)) = self.callee(&c.func) else {
            return err(
                c.func.span(),
                "only direct calls to named functions are supported",
            );
        };
        let args: Vec<&Expr> = c.args.iter().collect();
        let tf = match &*c.func {
            Expr::Path(p) => match &p.path.segments.last().unwrap().arguments {
                syn::PathArguments::AngleBracketed(a) => Some(a),
                _ => None,
            },
            _ => None,
        };
        self.call_fn(id, None, &args, tf, expected, c.span())
    }

    /// `recv.m(args)`: レシーバを自動 deref して struct を見つけ、
    /// メソッドの self 型に合わせて自動参照 (`&self` / `&mut self`) かコピー (`self`) で渡す
    fn method_call(&mut self, mc: &syn::ExprMethodCall) -> R<V> {
        if let Some(r) = self.builtin_method(mc)? {
            return Ok(r);
        }
        if let Some(r) = self.prim_trait_method(mc)? {
            return Ok(r);
        }
        let mut p = if is_place_expr(&mc.receiver) {
            self.place(&mc.receiver)?
        } else {
            match self.expr(&mc.receiver, None)? {
                (_, Ty::Never) => return Ok((None, Ty::Never)),
                (Some(a), t) if t.is_agg() => Place::Mem(a, 0, t),
                (Some(v), Ty::Ref(_, i)) => Place::Mem(v, 0, *i),
                (_, t) => {
                    return err(
                        mc.method.span(),
                        format!("no method `{}` on type `{}`", mc.method, self.prog.tn(t)),
                    );
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
                    return err(
                        mc.method.span(),
                        format!("no method `{}` on type `{t}`", mc.method),
                    );
                }
            }
        };
        let name = mc.method.to_string();
        let sname = self.prog.structs[sid as usize].name.clone();
        // 固有メソッド / trait 実装のメソッド → なければ実装している trait のデフォルトメソッド
        let Some(id) = self
            .prog
            .method(sid, &name)
            .or_else(|| self.prog.default_method(sid, &name))
        else {
            return err(
                mc.method.span(),
                format!("no method named `{name}` found for `{sname}`"),
            );
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
        self.call_fn(
            id,
            Some((recv, recv_ty)),
            &args,
            mc.turbofish.as_ref(),
            None,
            mc.span(),
        )
    }

    /// プリミティブ型への trait 実装のメソッド (`impl Tr for u32` の `x.m()`)
    fn prim_trait_method(&mut self, mc: &syn::ExprMethodCall) -> R<Option<V>> {
        let Some(mut t) = self.peek(&mc.receiver) else {
            return Ok(None);
        };
        while let Ty::Ref(_, i) = t {
            t = *i;
        }
        if !(t.is_int() || matches!(t, Ty::Bool | Ty::Char)) {
            return Ok(None);
        }
        let name = mc.method.to_string();
        let Some(id) = self.prog.prim_method(t, &name) else {
            return Ok(None);
        };
        // 受け手: 値ならそのまま、&self ならメモリに置いてそのアドレス
        let (v, vt) = self.expr(&mc.receiver, None)?;
        let mut v = v.unwrap();
        let mut vt = vt;
        while let Ty::Ref(_, i) = vt {
            v = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), v, 0);
            vt = *i;
        }
        let (recv, recv_ty) = match self.prog.sigs[id as usize].params.first() {
            Some(Ty::Ref(m, _)) => {
                let a = self.temp(t);
                self.b.ins().store(mf(), v, a, 0);
                (a, Ty::Ref(*m, intern(t)))
            }
            _ => (v, t),
        };
        let args: Vec<&Expr> = mc.args.iter().collect();
        self.call_fn(
            id,
            Some((recv, recv_ty)),
            &args,
            mc.turbofish.as_ref(),
            None,
            mc.span(),
        )
        .map(Some)
    }

    /// 型引数を解いて、呼ぶべき関数 (インスタンス / trait 実装) とそのシグネチャを決める
    /// turbofish (`f::<A, B>`) で決まる型パラメータ
    fn turbofish_sol(
        &self,
        sig: &FnSig,
        turbofish: Option<&syn::AngleBracketedGenericArguments>,
    ) -> R<Vec<Option<Ty>>> {
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

    fn resolve_call(
        &mut self,
        id: u32,
        arg_tys: &[Ty],
        mut sol: Vec<Option<Ty>>,
        ret_hint: Option<Ty>,
        span: proc_macro2::Span,
    ) -> R<(u32, FnSig)> {
        let sig = self.prog.sigs[id as usize].clone();
        if !sig.is_generic() {
            return Ok((id, sig));
        }
        for (&pt, &at) in sig.params.iter().zip(arg_tys) {
            if !self.prog.match_ty(pt, at, &mut sol) {
                return err(
                    span,
                    format!(
                        "mismatched types: expected `{}`, found `{}`",
                        self.prog.tn(pt),
                        self.prog.tn(at)
                    ),
                );
            }
        }
        // 引数から決まらない型パラメータは期待される戻り値型から (`let s: S<i64> = S::new()`)
        if let Some(rt) = ret_hint.filter(|_| sol.iter().any(|s| s.is_none())) {
            let mut s2 = sol.clone();
            if self.prog.match_ty(sig.ret, rt, &mut s2) {
                sol = s2;
            }
        }
        let Some(args) = sol.iter().copied().collect::<Option<Vec<Ty>>>() else {
            let i = sol.iter().position(|s| s.is_none()).unwrap();
            return err(
                span,
                format!("cannot infer type parameter `{}`", sig.generics[i].0),
            );
        };
        // trait の関数を `Tr::m(&x)` で呼んだら、実装があればそちらへ静的ディスパッチ
        if sig.trait_of.is_some() {
            let m = self.prog.names[id as usize]
                .rsplit("::")
                .next()
                .unwrap()
                .to_string();
            let imp = match args[0] {
                Ty::Struct(sid) => self.prog.method(sid, &m),
                t if t.is_int() || matches!(t, Ty::Bool | Ty::Char) => self
                    .prog
                    .index
                    .get(&format!("{}::{m}", self.prog.tn(t)))
                    .copied(),
                _ => None,
            };
            if let Some(imp) = imp {
                return Ok((imp, self.prog.sigs[imp as usize].clone()));
            }
            if !sig.has_body {
                return err(
                    span,
                    format!(
                        "`{}` has no implementation for `{}`",
                        self.prog.names[id as usize],
                        self.prog.tn(args[0])
                    ),
                );
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
        expected: Option<Ty>,
        span: proc_macro2::Span,
    ) -> R<V> {
        let sig0 = self.prog.sigs[id as usize].clone();
        let skip = recv.is_some() as usize;
        if sig0.params.len() != args.len() + skip {
            return err(
                span,
                format!(
                    "expected {} arguments, found {}",
                    sig0.params.len() - skip,
                    args.len()
                ),
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
            let (v, mut t) = self.expr(a, exp)?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            // ジェネリックな `&[T]` に `&[U; N]` を渡したら、型を解く前にスライスにする
            let mut v = v;
            if let (Ty::Ref(pm, Ty::Slice(_)), Ty::Ref(am, Ty::Array(e, n)), Some(val)) = (pt, t, v)
            {
                if am || !pm {
                    v = Some(self.make_fat(val, *n as i64));
                    t = Ty::Ref(pm, intern(Ty::Slice(e)));
                }
            }
            // 自動 deref 強制: `&&T` を `&T` の引数に渡す
            let want = self.prog.partial_subst(pt, &sol);
            while let (Ty::Ref(_, _), Ty::Ref(_, inner @ Ty::Ref(..)), Some(val)) = (want, t, v) {
                if t == want || self.prog.has_param(want) {
                    break;
                }
                // fat な参照 (`&str` / `&[T]`) はメモリ上の 16 byte のアドレスが値なのでそのまま
                v = Some(if inner.is_fat() {
                    val
                } else {
                    self.b.ins().load(PTR, mf(), val, 0)
                });
                t = *inner;
            }
            self.prog.match_ty(pt, t, &mut sol);
            vals.push((v.unwrap(), t, a.span()));
        }
        let arg_tys: Vec<Ty> = vals.iter().map(|v| v.1).collect();
        let sol0 = self.turbofish_sol(&sig0, turbofish)?;
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
        if pt.is_agg() {
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
            Lit::Bool(b) => Ok((
                Some(self.b.ins().iconst(types::I8, b.value as i64)),
                Ty::Bool,
            )),
            Lit::Byte(b) => Ok((
                Some(self.b.ins().iconst(types::I8, b.value() as i64)),
                Ty::U8,
            )),
            Lit::Char(c) => Ok((
                Some(self.b.ins().iconst(types::I32, c.value() as i64)),
                Ty::Char,
            )),
            // "..." は &'static str (fat pointer)、b"..." は &'static [u8; N]
            Lit::Str(st) => {
                let bytes = st.value().into_bytes();
                let n = bytes.len() as i64;
                let a = self.data_addr(bytes);
                Ok((Some(self.make_fat(a, n)), Ty::Ref(false, intern(Ty::Str))))
            }
            Lit::ByteStr(bs) => {
                let bytes = bs.value();
                let n = bytes.len() as u32;
                let a = self.data_addr(bytes);
                Ok((
                    Some(a),
                    Ty::Ref(false, intern(Ty::Array(intern(Ty::U8), n))),
                ))
            }
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
            _ => err(
                u.span(),
                format!("unsupported unary operator for `{}`", self.prog.tn(t)),
            ),
        }
    }

    fn binary(&mut self, b: &syn::ExprBinary, expected: Option<Ty>) -> R<V> {
        use BinOp::*;
        // 短絡評価
        if let And(_) | Or(_) = b.op {
            let (l, lt) = self.expr(&b.left, Some(Ty::Bool))?;
            self.same(b.left.span(), lt, Ty::Bool)?;
            let Some(l) = l else {
                return Ok((None, Ty::Never));
            };
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
            let rhint = if matches!(op, Shl(_) | Shr(_)) {
                self.peek(&b.right)
            } else {
                Some(t)
            };
            let (r, rt) = self.expr(&b.right, rhint.or(Some(t)))?;
            let Some(r) = r else {
                return Ok((None, Ty::Never));
            };
            let Some(l) = l else {
                return err(b.span(), "unsupported operand");
            };
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
            self.peek(&b.left)
                .or_else(|| self.peek(&b.right))
                .or(expected)
        };
        let (l, lt) = self.expr(&b.left, hint)?;
        let Some(l) = l else {
            return Ok((None, Ty::Never));
        };
        let rhint = if is_shift {
            self.peek(&b.right)
        } else {
            Some(lt)
        };
        let (r, rt) = self.expr(&b.right, rhint.or(Some(lt)))?;
        let Some(r) = r else {
            return Ok((None, Ty::Never));
        };

        if is_cmp(&b.op) {
            self.same(b.right.span(), rt, lt)?;
            // 集約型の == / != : 手書きの eq があればそれ、なければ構造的に比べる (derive(PartialEq) 相当)
            if matches!(b.op, Eq(_) | Ne(_)) && lt.is_agg() {
                let e = self.agg_eq(l, r, lt, b.span())?;
                let e = if matches!(b.op, Ne(_)) {
                    self.b.ins().bxor_imm_u(e, 1)
                } else {
                    e
                };
                return Ok((Some(e), Ty::Bool));
            }
            if !(lt.is_int() || matches!(lt, Ty::Bool | Ty::Char)) {
                return err(
                    b.span(),
                    format!("comparison of `{}` is not supported", self.prog.tn(lt)),
                );
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

    /// a, b (t 型の値。集約型ならアドレス) が等しいか (i8 の 0/1)
    fn agg_eq(&mut self, a: Value, b: Value, t: Ty, span: proc_macro2::Span) -> R<Value> {
        if let Ty::Struct(sid) = t {
            if let Some(id) = self.prog.method(sid, "eq") {
                let sig = self.prog.sigs[id as usize].clone();
                if !sig.is_generic() {
                    let f = self.fref(id, &sig);
                    let call = self.b.ins().call(f, &[a, b]);
                    return Ok(self.b.inst_results(call)[0]);
                }
            }
        }
        self.eq_at(a, b, t, span)
    }

    fn eq_at(&mut self, a: Value, b: Value, t: Ty, span: proc_macro2::Span) -> R<Value> {
        let one = |this: &mut Self| this.b.ins().iconst(types::I8, 1);
        match t {
            Ty::Unit => Ok(one(self)),
            t if t.is_int() || matches!(t, Ty::Bool | Ty::Char | Ty::Ptr(..)) => {
                Ok(self.b.ins().icmp(IntCC::Equal, a, b))
            }
            // 参照は指す先の値で比べる
            Ty::Ref(_, inner) if !t.is_fat() => {
                let (x, y) = match clif_ty(*inner) {
                    Some(ct) if !inner.is_agg() => (
                        self.b.ins().load(ct, mf(), a, 0),
                        self.b.ins().load(ct, mf(), b, 0),
                    ),
                    _ => (a, b),
                };
                self.eq_at(x, y, *inner, span)
            }
            Ty::Array(e, n) => {
                let (es, _) = self.prog.size_align(*e);
                let mut acc = one(self);
                for i in 0..n {
                    let (x, y) = self.load_field(a, b, *e, i * es);
                    let c = self.eq_at(x, y, *e, span)?;
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
                let (tag, variants) = (
                    sd.tag,
                    sd.variants
                        .iter()
                        .map(|v| (v.discr, v.fields.clone()))
                        .collect::<Vec<_>>(),
                );
                let ct = clif_ty(tag).unwrap();
                let ta = self.b.ins().load(ct, mf(), a, 0);
                let tb = self.b.ins().load(ct, mf(), b, 0);
                let tag_eq = self.b.ins().icmp(IntCC::Equal, ta, tb);
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
                Ok(self.b.ins().band(tag_eq, any))
            }
            t => err(
                span,
                format!("`==` is not supported for `{}`", self.prog.tn(t)),
            ),
        }
    }

    fn agg_or_scalar_eq(&mut self, x: Value, y: Value, t: Ty, span: proc_macro2::Span) -> R<Value> {
        if t.is_agg() {
            self.agg_eq(x, y, t, span)
        } else {
            self.eq_at(x, y, t, span)
        }
    }

    /// a + off / b + off にある t 型の値 (集約型ならアドレス)
    fn load_field(&mut self, a: Value, b: Value, t: Ty, off: u32) -> (Value, Value) {
        match clif_ty(t) {
            Some(ct) if !t.is_agg() => (
                self.b.ins().load(ct, mf(), a, off as i32),
                self.b.ins().load(ct, mf(), b, off as i32),
            ),
            _ => (self.addr_plus(a, off as i32), self.addr_plus(b, off as i32)),
        }
    }

    fn arith(
        &mut self,
        b: &syn::ExprBinary,
        op: &BinOp,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> R<Value> {
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
            return err(
                b.span(),
                format!("unsupported operator for `{}`", self.prog.tn(lt)),
            );
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
        let (tv, tt) = self.block(&i.then_branch, expected)?;
        self.finish_arm(
            tv,
            tt,
            merge,
            &mut result_ty,
            &mut param,
            i.then_branch.span(),
        )?;

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
    fn variant_value(
        &mut self,
        sid: u32,
        vi: usize,
        args: &[(usize, &Expr)],
        expected: Option<Ty>,
    ) -> R<V> {
        self.adt_value(
            sid,
            Some(vi),
            args,
            expected,
            proc_macro2::Span::call_site(),
        )
    }

    /// struct (vi = None) / enum のバリアント (vi = Some) の値を一時領域に作る。args は (フィールド番号, 式)。
    /// ジェネリックな定義なら、期待型 → 実引数の型 の順にインスタンスを決める
    fn adt_value(
        &mut self,
        base: u32,
        vi: Option<usize>,
        args: &[(usize, &Expr)],
        expected: Option<Ty>,
        span: proc_macro2::Span,
    ) -> R<V> {
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
                Some(Ty::Struct(e))
                    if self.prog.base_of(e) == self.prog.base_of(base)
                        && !self.prog.has_param(Ty::Struct(e)) =>
                {
                    Some(e)
                }
                _ => None,
            }
        };
        let gen_fields = fields_of(self, self.prog.base_of(base));
        let mut sol: Vec<Option<Ty>> = vec![None; n_gen];
        let mut vals = vec![];
        for &(fi, e) in args {
            let exp = match inst {
                Some(i) => Some(fields_of(self, i)[fi].1),
                None => Some(self.prog.partial_subst(gen_fields[fi].1, &sol))
                    .filter(|&t| !self.prog.has_param(t)),
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
                Some(targs) => self
                    .prog
                    .instantiate(self.prog.base_of(base), targs)
                    .or_else(|e| err(span, e))?,
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

    // --- タプル・配列・スライス ------------------------------------------

    fn make_fat(&mut self, ptr: Value, len: i64) -> Value {
        let l = self.b.ins().iconst(types::I64, len);
        self.make_fat_v(ptr, l)
    }

    /// (ポインタ, 長さ) の組を一時領域に作り、そのアドレスを返す
    fn make_fat_v(&mut self, ptr: Value, len: Value) -> Value {
        let ss =
            self.b
                .create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3));
        let a = self.b.ins().stack_addr(PTR, ss, 0);
        self.b.ins().store(mf(), ptr, a, 0);
        self.b.ins().store(mf(), len, a, 8);
        a
    }

    fn elem_addr(&mut self, base: Value, i: Value, elem: Ty) -> Value {
        let (size, _) = self.prog.size_align(elem);
        let off = self.b.ins().imul_imm_s(i, size as i64);
        self.b.ins().iadd(base, off)
    }

    /// 添字アクセスの対象 (配列 / スライス / それらへの参照) の (先頭アドレス, 長さ, 要素型)
    fn indexable(&mut self, e: &Expr) -> R<(Value, Value, Ty)> {
        let is_const = matches!(e, Expr::Path(p) if self.const_of(p).is_some());
        let mut p = if is_place_expr(e) && !is_const {
            self.place(e)?
        } else {
            match self.expr(e, None)? {
                (Some(a), t) if t.is_agg() => Place::Mem(a, 0, t),
                (Some(v), Ty::Ref(_, i)) => Place::Mem(v, 0, *i),
                (_, t) => {
                    return err(
                        e.span(),
                        format!("cannot index into a value of type `{}`", self.prog.tn(t)),
                    );
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
                Place::Mem(a, o, Ty::Ref(_, i)) => {
                    let ptr = self.b.ins().load(PTR, mf(), a, o);
                    Place::Mem(ptr, 0, *i)
                }
                Place::Var(v, Ty::Ref(_, i)) => {
                    let ptr = self.b.use_var(v);
                    Place::Mem(ptr, 0, *i)
                }
                p => {
                    return err(
                        e.span(),
                        format!(
                            "cannot index into a value of type `{}`",
                            self.prog.tn(p.ty())
                        ),
                    );
                }
            }
        }
    }

    /// `&a[lo..hi]` / `&a[..]` / `&a[lo..=hi]`
    fn slice_range(&mut self, base: &Expr, rg: &syn::ExprRange, m: bool) -> R<V> {
        let (ptr, len, elem) = self.indexable(base)?;
        let lo = match &rg.start {
            Some(e) => {
                let (v, t) = self.expr(e, Some(Ty::Usize))?;
                self.same(e.span(), t, Ty::Usize)?;
                v.unwrap()
            }
            None => self.b.ins().iconst(types::I64, 0),
        };
        let hi = match &rg.end {
            Some(e) => {
                let (v, t) = self.expr(e, Some(Ty::Usize))?;
                self.same(e.span(), t, Ty::Usize)?;
                let v = v.unwrap();
                match rg.limits {
                    syn::RangeLimits::Closed(_) => self.b.ins().iadd_imm_s(v, 1),
                    _ => v,
                }
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

    fn tuple_lit(&mut self, t: &syn::ExprTuple, expected: Option<Ty>) -> R<V> {
        let base = self.prog.tuple_base(t.elems.len());
        let args: Vec<(usize, &Expr)> = t.elems.iter().enumerate().collect();
        self.adt_value(base, None, &args, expected, t.span())
    }

    fn array_lit(&mut self, a: &syn::ExprArray, expected: Option<Ty>) -> R<V> {
        let mut elem = match expected {
            Some(Ty::Array(t, _)) | Some(Ty::Slice(t)) => Some(*t),
            _ => None,
        };
        let mut vals = vec![];
        for e in &a.elems {
            let (v, t) = self.expr(e, elem)?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            match elem {
                Some(et) => self.same(e.span(), t, et)?,
                None => elem = Some(t),
            }
            vals.push(v);
        }
        let Some(elem) = elem else {
            return err(a.span(), "type annotations needed for an empty array");
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
    fn array_len(&self, e: &Expr) -> R<u32> {
        match e {
            Expr::Lit(syn::ExprLit {
                lit: Lit::Int(i), ..
            }) => i.base10_parse::<u32>().or_else(|er| err(i.span(), er)),
            Expr::Path(p) if p.path.get_ident().is_some() => {
                let ts = crate::front::TySyn::Array(
                    Box::new(crate::front::TySyn::Prim(Ty::U8)),
                    crate::front::ArrLen::Const(p.path.get_ident().unwrap().to_string()),
                );
                match self.prog.resolve(&ts) {
                    Ok(Ty::Array(_, n)) => Ok(n),
                    Ok(_) => unreachable!(),
                    Err(er) => err(p.span(), er),
                }
            }
            _ => err(e.span(), "array length must be a literal or a const"),
        }
    }

    fn repeat_lit(&mut self, r: &syn::ExprRepeat, expected: Option<Ty>) -> R<V> {
        let n = self.array_len(&r.len)?;
        let exp = match expected {
            Some(Ty::Array(t, _)) => Some(*t),
            _ => None,
        };
        let (v, elem) = self.expr(&r.expr, exp)?;
        if elem == Ty::Never {
            return Ok((None, Ty::Never));
        }
        let t = Ty::Array(intern(elem), n);
        let base = self.temp(t);
        let (size, _) = self.prog.size_align(elem);
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
            let c = self
                .b
                .ins()
                .icmp_imm_u(IntCC::UnsignedLessThan, iv, n as i64);
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

    /// 配列 / スライスの組み込みメソッド (len / is_empty / get / get_mut / first / last / swap / contains)
    fn builtin_method(&mut self, mc: &syn::ExprMethodCall) -> R<Option<V>> {
        let name = mc.method.to_string();
        let Some(mut rt) = self.peek(&mc.receiver) else {
            return Ok(None);
        };
        while let Ty::Ref(_, i) = rt {
            if rt.is_fat() {
                break;
            }
            rt = *i;
        }
        let elem = match rt {
            Ty::Array(t, _) => *t,
            Ty::Ref(_, Ty::Slice(t)) => **t,
            // &str は [u8] と同じ表現
            Ty::Ref(_, Ty::Str) => return self.str_method(mc),
            Ty::Ptr(_, t) => return self.ptr_method(mc, rt, *t),
            t if t.is_int() || t == Ty::Bool || t == Ty::Char => return self.int_method(mc, t),
            _ => return Ok(None),
        };
        let args: Vec<&Expr> = mc.args.iter().collect();
        let opt = |this: &Self, t: Ty| -> R<Ty> {
            let ob = *this
                .prog
                .struct_index
                .get("Option")
                .ok_or("Option is not available")?;
            Ok(Ty::Struct(this.prog.instantiate(ob, vec![t])?))
        };
        match name.as_str() {
            "len" | "is_empty" => {
                let (_, len, _) = self.indexable(&mc.receiver)?;
                if name == "len" {
                    Ok(Some((Some(len), Ty::Usize)))
                } else {
                    let c = self.b.ins().icmp_imm_u(IntCC::Equal, len, 0);
                    Ok(Some((Some(c), Ty::Bool)))
                }
            }
            "get" | "get_mut" | "first" | "last" => {
                let (base, len, elem) = self.indexable(&mc.receiver)?;
                let i = match name.as_str() {
                    "first" => self.b.ins().iconst(types::I64, 0),
                    "last" => self.b.ins().iadd_imm_s(len, -1),
                    _ => {
                        let Some(a) = args.first() else {
                            return err(mc.span(), "get needs an index");
                        };
                        let (v, t) = self.expr(a, Some(Ty::Usize))?;
                        self.same(a.span(), t, Ty::Usize)?;
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
                let (base, len, elem) = self.indexable(&mc.receiver)?;
                let [a1, a2] = args[..] else {
                    return err(mc.span(), "swap needs two indices");
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
                    return err(
                        mc.span(),
                        "contains is only supported for integer / bool / char elements",
                    );
                }
                let (base, len, elem) = self.indexable(&mc.receiver)?;
                let Some(a) = args.first() else {
                    return err(mc.span(), "contains needs an argument");
                };
                let (x, xt) = self.expr(a, Some(Ty::Ref(false, intern(elem))))?;
                self.same(a.span(), xt, Ty::Ref(false, intern(elem)))?;
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
                self.b.ins().brif(c, body, &[], done, &[f0.into()]);
                self.b.switch_to_block(body);
                let addr = self.elem_addr(base, iv, elem);
                let ev = self.b.ins().load(clif_ty(elem).unwrap(), mf(), addr, 0);
                let eq = self.b.ins().icmp(IntCC::Equal, ev, xv);
                let nx = self.b.ins().iadd_imm_s(iv, 1);
                self.b.def_var(i, nx);
                let t1 = self.b.ins().iconst(types::I8, 1);
                self.b.ins().brif(eq, done, &[t1.into()], head, &[]);
                self.b.switch_to_block(done);
                Ok(Some((Some(res), Ty::Bool)))
            }
            "as_ptr" | "as_mut_ptr" => {
                let (base, _, elem) = self.indexable(&mc.receiver)?;
                Ok(Some((
                    Some(base),
                    Ty::Ptr(name == "as_mut_ptr", intern(elem)),
                )))
            }
            "iter" | "iter_mut" | "into_iter" | "enumerate" | "rev" | "copied" | "cloned" => err(
                mc.span(),
                format!("`.{name}()` is only supported directly in a `for` loop"),
            ),
            _ => Ok(None),
        }
    }

    /// &str: len / is_empty / as_bytes / as_ptr (表現は &[u8] と同じ)
    fn str_method(&mut self, mc: &syn::ExprMethodCall) -> R<Option<V>> {
        let (v, _) = self.expr(&mc.receiver, None)?;
        let Some(fat) = v else {
            return Ok(Some((None, Ty::Never)));
        };
        let u8s = Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8))));
        Ok(Some(match mc.method.to_string().as_str() {
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
            m => {
                return err(
                    mc.method.span(),
                    format!(
                        "no method `{m}` on `&str` (supported: len, is_empty, as_bytes, as_ptr)"
                    ),
                );
            }
        }))
    }

    /// 生ポインタ: add / sub / offset / is_null / read / write / read_volatile / write_volatile / cast
    fn ptr_method(&mut self, mc: &syn::ExprMethodCall, pt: Ty, elem: Ty) -> R<Option<V>> {
        let (p, _) = self.expr(&mc.receiver, Some(pt))?;
        let p = p.unwrap();
        let args: Vec<&Expr> = mc.args.iter().collect();
        let name = mc.method.to_string();
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
            "read" | "read_volatile" | "read_unaligned" => self.read(Place::Mem(p, 0, elem)),
            "write" | "write_volatile" | "write_unaligned" => {
                let (v, vt) = self.expr(args[0], Some(elem))?;
                self.same(args[0].span(), vt, elem)?;
                self.write(Place::Mem(p, 0, elem), v);
                (None, Ty::Unit)
            }
            "cast" => {
                let to = match &mc.turbofish {
                    Some(tf) => match tf.args.first() {
                        Some(syn::GenericArgument::Type(t)) => self.prog.parse_ty(t, &self.env)?,
                        _ => return err(mc.span(), "cast needs a type argument"),
                    },
                    None => return err(mc.span(), "`cast` needs a turbofish: `p.cast::<T>()`"),
                };
                let Ty::Ptr(m, _) = pt else { unreachable!() };
                (Some(p), Ty::Ptr(m, intern(to)))
            }
            m => {
                return err(
                    mc.method.span(),
                    format!("unsupported raw pointer method `{m}`"),
                );
            }
        }))
    }

    /// 整数の組み込みメソッド (wrapping_* / checked_* / saturating_* / min / max / abs / pow /
    /// count_ones / leading_zeros / trailing_zeros / is_power_of_two / rotate_* / swap_bytes / signum)
    fn int_method(&mut self, mc: &syn::ExprMethodCall, t: Ty) -> R<Option<V>> {
        let name = mc.method.to_string();
        let args: Vec<&Expr> = mc.args.iter().collect();
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
        ];
        if !known.contains(&name.as_str()) {
            return Ok(None);
        }
        let (a, _) = self.expr(&mc.receiver, Some(t))?;
        let a = a.unwrap();
        let ct = clif_ty(t).unwrap();
        let s = t.signed();
        let arg = |this: &mut Self, i: usize, want: Ty| -> R<Value> {
            let Some(e) = args.get(i) else {
                return err(mc.span(), format!("`{name}` needs an argument"));
            };
            let (v, vt) = this.expr(e, Some(want))?;
            this.same(e.span(), vt, want)?;
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
            "abs" => (Some(self.b.ins().iabs(a)), t),
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
                let z = self.b.ins().iconst(ct, 0);
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
            "pow" => {
                // 繰り返し二乗法
                let e0 = arg(self, 0, Ty::U32)?;
                let r = self.b.declare_var(ct);
                let bv = self.b.declare_var(ct);
                let ev = self.b.declare_var(types::I32);
                let one = self.b.ins().iconst(ct, 1);
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
                let rm = self.b.ins().imul(rv, bb);
                let rn = self.b.ins().select(bit, rm, rv);
                self.b.def_var(r, rn);
                let b2 = self.b.ins().imul(bb, bb);
                self.b.def_var(bv, b2);
                let e2 = self.b.ins().ushr_imm_u(e, 1);
                self.b.def_var(ev, e2);
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(done);
                (Some(self.b.use_var(r)), t)
            }
            "saturating_add" | "saturating_sub" => {
                let b = arg(self, 0, t)?;
                let ins = self.b.ins();
                let v = match (name.as_str(), s) {
                    ("saturating_add", false) => ins.uadd_sat(a, b),
                    ("saturating_add", true) => ins.sadd_sat(a, b),
                    ("saturating_sub", false) => ins.usub_sat(a, b),
                    _ => ins.ssub_sat(a, b),
                };
                (Some(v), t)
            }
            "checked_add" | "checked_sub" | "checked_mul" | "checked_div" | "checked_rem" => {
                let b = arg(self, 0, t)?;
                let (v, of) = match name.as_str() {
                    "checked_add" if s => self.b.ins().sadd_overflow(a, b),
                    "checked_add" => self.b.ins().uadd_overflow(a, b),
                    "checked_sub" if s => self.b.ins().ssub_overflow(a, b),
                    "checked_sub" => self.b.ins().usub_overflow(a, b),
                    "checked_mul" if s => self.b.ins().smul_overflow(a, b),
                    "checked_mul" => self.b.ins().umul_overflow(a, b),
                    _ => {
                        // 0 除算 (と符号付きの MIN / -1) は None。安全な除数に差し替えてから割る
                        let zero = self.b.ins().icmp_imm_u(IntCC::Equal, b, 0);
                        let bad = if s {
                            let min = crate::front::prim_assoc_const(t, "MIN").unwrap() as i64;
                            let am = self.b.ins().icmp_imm_s(IntCC::Equal, a, min);
                            let bm = self.b.ins().icmp_imm_s(IntCC::Equal, b, -1);
                            let both = self.b.ins().band(am, bm);
                            self.b.ins().bor(zero, both)
                        } else {
                            zero
                        };
                        let one = self.b.ins().iconst(ct, 1);
                        let safe = self.b.ins().select(bad, one, b);
                        let ins = self.b.ins();
                        let q = match (name.as_str(), s) {
                            ("checked_div", true) => ins.sdiv(a, safe),
                            ("checked_div", false) => ins.udiv(a, safe),
                            ("checked_rem", true) => ins.srem(a, safe),
                            _ => ins.urem(a, safe),
                        };
                        (q, bad)
                    }
                };
                let ot = {
                    let ob = *self
                        .prog
                        .struct_index
                        .get("Option")
                        .ok_or("Option is not available")?;
                    Ty::Struct(self.prog.instantiate(ob, vec![t])?)
                };
                let Ty::Struct(oi) = ot else { unreachable!() };
                let out = self.temp(ot);
                let some = self.prog.structs[oi as usize]
                    .variants
                    .iter()
                    .position(|v| v.name == "Some")
                    .unwrap();
                let none = self.prog.structs[oi as usize]
                    .variants
                    .iter()
                    .position(|v| v.name == "None")
                    .unwrap();
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
                (Some(out), ot)
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

    // --- for ループ ------------------------------------------------------

    fn for_(&mut self, f: &syn::ExprForLoop) -> R<V> {
        if f.label.is_some() {
            return err(f.span(), "loop labels are not supported");
        }
        // 外側からアダプタ (`.rev()` / `.enumerate()` / `.iter()` ...) を剥がす
        let mut e: &Expr = &f.expr;
        let mut adapters = vec![];
        loop {
            match e {
                Expr::MethodCall(mc)
                    if mc.args.is_empty()
                        && matches!(
                            mc.method.to_string().as_str(),
                            "iter"
                                | "iter_mut"
                                | "into_iter"
                                | "copied"
                                | "cloned"
                                | "enumerate"
                                | "rev"
                        ) =>
                {
                    adapters.push(mc.method.to_string());
                    e = &mc.receiver;
                }
                Expr::Paren(p) => e = &p.expr,
                _ => break,
            }
        }
        adapters.reverse();
        let rev = adapters.iter().any(|a| a == "rev");
        let enumerate = adapters.iter().any(|a| a == "enumerate");
        if let Expr::Range(rg) = e {
            return self.for_range(f, rg, rev, enumerate);
        }
        // 配列 / スライスを添字で回す
        let (by_ref, base_e): (Option<bool>, &Expr) = match e {
            Expr::Reference(r) => (Some(r.mutability.is_some()), &r.expr),
            _ => (None, e),
        };
        // `&a[lo..hi]` は部分スライス (参照ごと評価する)
        let sub = matches!(base_e, Expr::Index(ix) if matches!(&*ix.index, Expr::Range(_)));
        let (ptr, len, elem) = self.indexable(if sub { e } else { base_e })?;
        let mut mode = by_ref;
        let base_is_ref = by_ref.is_none() && matches!(self.peek(base_e), Some(Ty::Ref(..)));
        if base_is_ref {
            mode = Some(matches!(self.peek(base_e), Some(Ty::Ref(true, _))));
        }
        for a in &adapters {
            match a.as_str() {
                "iter" => mode = Some(false),
                "iter_mut" => mode = Some(true),
                "copied" | "cloned" => mode = None,
                _ => {}
            }
        }
        let i = self.b.declare_var(types::I64);
        let start = if rev {
            len
        } else {
            self.b.ins().iconst(types::I64, 0)
        };
        self.b.def_var(i, start);
        let head = self.b.create_block();
        let body = self.b.create_block();
        let step = self.b.create_block();
        let exit = self.b.create_block();
        self.b.ins().jump(head, &[]);
        self.b.switch_to_block(head);
        let iv = self.b.use_var(i);
        let c = if rev {
            self.b.ins().icmp_imm_u(IntCC::UnsignedGreaterThan, iv, 0)
        } else {
            self.b.ins().icmp(IntCC::UnsignedLessThan, iv, len)
        };
        self.b.ins().brif(c, body, &[], exit, &[]);
        self.b.switch_to_block(body);
        let idx = if rev {
            let v = self.b.ins().iadd_imm_s(iv, -1);
            self.b.def_var(i, v);
            v
        } else {
            iv
        };
        let addr = self.elem_addr(ptr, idx, elem);
        let (val, vt) = match mode {
            Some(m) => (Some(addr), Ty::Ref(m, intern(elem))),
            None => self.read(Place::Mem(addr, 0, elem)),
        };
        let (val, vt) = if enumerate {
            self.make_pair(idx, val, vt)?
        } else {
            (val, vt)
        };
        self.for_body(f, val, vt, step, exit)?;
        self.b.switch_to_block(step);
        if !rev {
            let iv = self.b.use_var(i);
            let nx = self.b.ins().iadd_imm_s(iv, 1);
            self.b.def_var(i, nx);
        }
        self.b.ins().jump(head, &[]);
        self.b.switch_to_block(exit);
        Ok((None, Ty::Unit))
    }

    /// enumerate 用の (usize, T) を一時領域に作る
    fn make_pair(&mut self, idx: Value, v: Option<Value>, t: Ty) -> R<V> {
        let base = self.prog.tuple_base(2);
        let inst = self.prog.instantiate(base, vec![Ty::Usize, t])?;
        let tt = Ty::Struct(inst);
        let a = self.temp(tt);
        let f = self.prog.structs[inst as usize].fields.clone();
        self.write(Place::Mem(a, f[0].2 as i32, Ty::Usize), Some(idx));
        self.write(Place::Mem(a, f[1].2 as i32, t), v);
        Ok((Some(a), tt))
    }

    /// ループ本体: パターンに値を束縛して本体を実行し、step (continue 先) へ
    fn for_body(
        &mut self,
        f: &syn::ExprForLoop,
        val: Option<Value>,
        vt: Ty,
        step: Block,
        exit: Block,
    ) -> R<()> {
        self.scopes.push(vec![]);
        let place = match val {
            Some(a) if vt.is_agg() => Place::Mem(a, 0, vt),
            Some(v) => {
                let var = self.b.declare_var(clif_ty(vt).unwrap());
                self.b.def_var(var, v);
                Place::Var(var, vt)
            }
            None => Place::Zst(vt),
        };
        self.pat_bind(&f.pat, place, BindMode::Move)?;
        self.loops.push(Loop {
            cont: step,
            brk: exit,
            breaks: 0,
        });
        let (_, bt) = self.block(&f.body, Some(Ty::Unit))?;
        self.loops.pop();
        self.scopes.pop();
        if bt != Ty::Never {
            self.same(f.body.span(), bt, Ty::Unit)?;
            self.b.ins().jump(step, &[]);
        } else {
            self.b.ins().trap(TrapCode::unwrap_user(1));
        }
        Ok(())
    }

    /// `for i in lo..hi` / `lo..=hi` / `(lo..hi).rev()`
    fn for_range(
        &mut self,
        f: &syn::ExprForLoop,
        rg: &syn::ExprRange,
        rev: bool,
        enumerate: bool,
    ) -> R<V> {
        let (Some(lo_e), Some(hi_e)) = (&rg.start, &rg.end) else {
            return err(rg.span(), "for loops need a bounded range");
        };
        let hint = self.peek(lo_e).or_else(|| self.peek(hi_e));
        let (lo, lt) = self.expr(lo_e, hint)?;
        let (hi, ht) = self.expr(hi_e, Some(lt))?;
        self.same(hi_e.span(), ht, lt)?;
        if !lt.is_int() {
            return err(rg.span(), "range bounds must be integers");
        }
        let (lo, hi) = (lo.unwrap(), hi.unwrap());
        let inclusive = matches!(rg.limits, syn::RangeLimits::Closed(_));
        let s = lt.signed();
        let ct = clif_ty(lt).unwrap();
        let lt_cc = if s {
            IntCC::SignedLessThan
        } else {
            IntCC::UnsignedLessThan
        };
        let le_cc = if s {
            IntCC::SignedLessThanOrEqual
        } else {
            IntCC::UnsignedLessThanOrEqual
        };
        let i = self.b.declare_var(ct);
        let n = self.b.declare_var(types::I64);
        let z = self.b.ins().iconst(types::I64, 0);
        self.b.def_var(n, z);
        let head = self.b.create_block();
        let body = self.b.create_block();
        let step = self.b.create_block();
        let exit = self.b.create_block();
        // 空かどうか
        let nonempty = self
            .b
            .ins()
            .icmp(if inclusive { le_cc } else { lt_cc }, lo, hi);
        // 現在値: 前向きなら lo から、逆向きなら最後の要素から
        let first = if !rev {
            lo
        } else if inclusive {
            hi
        } else {
            self.b.ins().iadd_imm_s(hi, -1)
        };
        self.b.def_var(i, first);
        self.b.ins().brif(nonempty, head, &[], exit, &[]);
        self.b.switch_to_block(head);
        self.b.ins().jump(body, &[]);
        self.b.switch_to_block(body);
        let iv = self.b.use_var(i);
        let (val, vt) = if enumerate {
            let nv = self.b.use_var(n);
            self.make_pair(nv, Some(iv), lt)?
        } else {
            (Some(iv), lt)
        };
        self.for_body(f, val, vt, step, exit)?;
        self.b.switch_to_block(step);
        let iv = self.b.use_var(i);
        let nv = self.b.use_var(n);
        let nn = self.b.ins().iadd_imm_s(nv, 1);
        self.b.def_var(n, nn);
        // 最後の要素を処理したら抜ける (lo..=MAX でも溢れない)
        let last = if !rev {
            if inclusive {
                hi
            } else {
                self.b.ins().iadd_imm_s(hi, -1)
            }
        } else {
            lo
        };
        let done = self.b.ins().icmp(IntCC::Equal, iv, last);
        let nx = if rev {
            self.b.ins().iadd_imm_s(iv, -1)
        } else {
            self.b.ins().iadd_imm_s(iv, 1)
        };
        let cont = self.b.create_block();
        self.b.ins().brif(done, exit, &[], cont, &[]);
        self.b.switch_to_block(cont);
        self.b.def_var(i, nx);
        self.b.ins().jump(body, &[]);
        self.b.switch_to_block(exit);
        Ok((None, Ty::Unit))
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
        p.parse2(mac.tokens.clone())
            .map(|v| v.into_iter().collect())
            .or_else(|e| err(mac.span(), e))
    }

    /// 対応しているマクロ: panic! / unreachable! / todo! / unimplemented! / assert! / assert_eq! /
    /// assert_ne! (と debug_ 版) / matches!。メッセージの書式化はしない
    fn macro_(&mut self, mac: &syn::Macro, expected: Option<Ty>) -> R<V> {
        let name = mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        match name.as_str() {
            "panic" | "unreachable" | "todo" | "unimplemented" => Ok(self.panic_call()),
            "assert" | "debug_assert" => {
                let args = self.macro_args(mac)?;
                let Some(c) = args.first() else {
                    return err(mac.span(), "assert! needs a condition");
                };
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
                let (Some(l), Some(r)) = (l, r) else {
                    return Ok((None, Ty::Unit));
                };
                if !(lt.is_int() || matches!(lt, Ty::Bool | Ty::Char)) {
                    return err(
                        mac.span(),
                        format!("{name}! is only supported for integers, bool and char"),
                    );
                }
                let cc = if name.ends_with("_eq") {
                    IntCC::Equal
                } else {
                    IntCC::NotEqual
                };
                let c = self.b.ins().icmp(cc, l, r);
                self.panic_unless(c);
                Ok((None, Ty::Unit))
            }
            "matches" => {
                use syn::parse::Parser;
                let parser =
                    |input: syn::parse::ParseStream| -> syn::Result<(Expr, Pat, Option<Expr>)> {
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
                let (e, pat, guard) = parser
                    .parse2(mac.tokens.clone())
                    .or_else(|er| err(mac.span(), er))?;
                let Some(place) = self.scrutinee(&e)? else {
                    return Ok((None, Ty::Never));
                };
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
        let Some(place) = self.scrutinee(&t.expr)? else {
            return Ok((None, Ty::Never));
        };
        let (place, _) = self.pat_deref(place, BindMode::Move);
        let Ty::Struct(inst) = place.ty() else {
            return err(
                t.span(),
                format!(
                    "the `?` operator cannot be applied to type `{}`",
                    self.prog.tn(place.ty())
                ),
            );
        };
        let family = self.prog.structs[self.prog.base_of(inst) as usize]
            .name
            .clone();
        let (okn, errn) = match family.as_str() {
            "Option" => ("Some", "None"),
            "Result" => ("Ok", "Err"),
            _ => {
                return err(
                    t.span(),
                    format!(
                        "the `?` operator cannot be applied to type `{}`",
                        self.prog.tn(place.ty())
                    ),
                );
            }
        };
        let vpos = |this: &Self, sid: u32, n: &str| {
            this.prog.structs[sid as usize]
                .variants
                .iter()
                .position(|v| v.name == n)
                .unwrap()
        };
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
                    format!(
                        "the `?` operator can only be used in a function that returns `{family}` (returns `{}`)",
                        self.prog.tn(ret)
                    ),
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

    /// コンパイラが直接扱う関数: `size_of::<T>()` / `align_of::<T>()` / `ptr::null::<T>()` / `null_mut`
    fn intrinsic_call(&mut self, c: &syn::ExprCall, expected: Option<Ty>) -> R<Option<V>> {
        let Expr::Path(p) = &*c.func else {
            return Ok(None);
        };
        let last = p.path.segments.last().unwrap();
        let name = last.ident.to_string();
        if !matches!(name.as_str(), "size_of" | "align_of" | "null" | "null_mut") {
            return Ok(None);
        }
        // ユーザが同名の関数を定義していればそちら
        if p.path.segments.len() == 1 && self.prog.index.contains_key(&name) {
            return Ok(None);
        }
        let targ = match &last.arguments {
            syn::PathArguments::AngleBracketed(a) => match a.args.first() {
                Some(syn::GenericArgument::Type(t)) => Some(self.prog.parse_ty(t, &self.env)?),
                _ => None,
            },
            _ => None,
        };
        Ok(Some(match name.as_str() {
            "size_of" | "align_of" => {
                let Some(t) = targ else {
                    return err(c.span(), format!("`{name}` needs a type: `{name}::<T>()`"));
                };
                let (sz, al) = self.prog.size_align(t);
                let v = if name == "size_of" { sz } else { al };
                (Some(self.b.ins().iconst(types::I64, v as i64)), Ty::Usize)
            }
            _ => {
                let elem = match (targ, expected) {
                    (Some(t), _) => t,
                    (None, Some(Ty::Ptr(_, t))) => *t,
                    _ => Ty::U8,
                };
                (
                    Some(self.b.ins().iconst(PTR, 0)),
                    Ty::Ptr(name == "null_mut", intern(elem)),
                )
            }
        }))
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

    /// 変数でもバリアントでもないパス: `u32::MAX` などの関連定数、`core::ptr::null()` 以外のパス
    fn path_value(&mut self, p: &syn::ExprPath, _expected: Option<Ty>) -> R<V> {
        let segs: Vec<String> = p
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        if segs.len() == 2 {
            if let Some(t) = crate::front::Ty::from_suffix(&segs[0]).or(if segs[0] == "char" {
                Some(Ty::Char)
            } else {
                None
            }) {
                if let Some(v) = crate::front::prim_assoc_const(t, &segs[1]) {
                    let t = if segs[1] == "BITS" { Ty::U32 } else { t };
                    let c = self.b.ins().iconst(clif_ty(t).unwrap(), v as i64);
                    return Ok((Some(c), t));
                }
            }
        }
        // `core::...::NAME` (const / static) は最後のセグメントで
        if segs.len() >= 2 && matches!(segs[0].as_str(), "core" | "crate" | "self") {
            let last: syn::Path = syn::Path::from(p.path.segments.last().unwrap().ident.clone());
            let e = Expr::Path(syn::ExprPath {
                attrs: vec![],
                qself: None,
                path: last,
            });
            return self.expr(&e, _expected);
        }
        err(p.span(), "cannot find value (unsupported path)")
    }

    /// 1 セグメントのパスが (ローカル変数でない) const なら、その index
    fn const_of(&self, p: &syn::ExprPath) -> Option<u32> {
        let id = p.path.get_ident()?.to_string();
        if self.lookup(&id).is_some() {
            return None;
        }
        let &ci = self.prog.const_index.get(&id)?;
        self.prog.consts[ci as usize].stat.is_none().then_some(ci)
    }

    /// 無名データ / static のシンボルのアドレス
    fn symbol_addr(&mut self, ns: u32, index: u32) -> Value {
        let gv = match self.gvs.get(&(ns, index)) {
            Some(&g) => g,
            None => {
                let name = self
                    .b
                    .func
                    .declare_imported_user_function(UserExternalName::new(ns, index));
                let g = self.b.create_global_value(ir::GlobalValueData::Symbol {
                    name: ExternalName::user(name),
                    offset: ir::immediates::Imm64::new(0),
                    colocated: true,
                    tls: false,
                });
                self.gvs.insert((ns, index), g);
                g
            }
        };
        self.b.ins().symbol_value(PTR, gv)
    }

    /// 無名データを足してそのアドレスを返す (同じ内容は同じデータ)
    fn data_addr(&mut self, bytes: Vec<u8>) -> Value {
        let k = match self.datas.iter().position(|d| *d == bytes) {
            Some(k) => k,
            None => {
                self.datas.push(bytes);
                self.datas.len() - 1
            }
        };
        self.symbol_addr(NS_DATA, k as u32)
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
            (Some(a), t) if t.is_agg() => Some(Place::Mem(a, 0, t)),
            (Some(v), t) => {
                let var = self.b.declare_var(clif_ty(t).unwrap());
                self.b.def_var(var, v);
                Some(Place::Var(var, t))
            }
            (None, t) => Some(Place::Zst(t)),
        })
    }

    fn match_(&mut self, m: &syn::ExprMatch, expected: Option<Ty>) -> R<V> {
        let Some(place) = self.scrutinee(&m.expr)? else {
            return Ok((None, Ty::Never));
        };
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
        let Some(place) = self.scrutinee(&l.expr)? else {
            return Ok((None, Ty::Never));
        };
        let else_blk = self.b.create_block();
        let merge = self.b.create_block();
        let mut result_ty: Option<Ty> = None;
        let mut param: Option<Value> = None;
        self.pat_test(&l.pat, place, else_blk)?;
        self.scopes.push(vec![]);
        self.pat_bind(&l.pat, place, BindMode::Move)?;
        let (tv, tt) = self.block(&i.then_branch, expected)?;
        self.scopes.pop();
        self.finish_arm(
            tv,
            tt,
            merge,
            &mut result_ty,
            &mut param,
            i.then_branch.span(),
        )?;
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
        let Expr::Let(l) = &*w.cond else {
            unreachable!()
        };
        let header = self.b.create_block();
        let exit = self.b.create_block();
        self.b.ins().jump(header, &[]);
        self.b.switch_to_block(header);
        // 毎回 match 対象を評価し直す
        let Some(place) = self.scrutinee(&l.expr)? else {
            return Ok((None, Ty::Never));
        };
        self.pat_test(&l.pat, place, exit)?;
        self.scopes.push(vec![]);
        self.pat_bind(&l.pat, place, BindMode::Move)?;
        self.loops.push(Loop {
            cont: header,
            brk: exit,
            breaks: 0,
        });
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
    fn test_variant(
        &mut self,
        p: Place,
        sid: u32,
        vi: usize,
        span: proc_macro2::Span,
        fail: Block,
    ) -> R<()> {
        if p.ty() != Ty::Struct(sid) {
            let got = self.prog.tn(p.ty());
            return err(
                span,
                format!(
                    "mismatched types: expected `{got}`, found `{}`",
                    self.prog.tn(Ty::Struct(sid))
                ),
            );
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
    fn tuple_fields<'e>(
        elems: impl Iterator<Item = &'e Pat>,
        nfields: usize,
        span: proc_macro2::Span,
    ) -> R<Vec<(usize, &'e Pat)>> {
        let elems: Vec<&Pat> = elems.collect();
        let rest = elems.iter().position(|p| matches!(p, Pat::Rest(_)));
        let n_named = elems.len() - rest.is_some() as usize;
        if n_named > nfields || (rest.is_none() && n_named != nfields) {
            return err(
                span,
                format!(
                    "this pattern has {n_named} fields, but the corresponding variant has {nfields}"
                ),
            );
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
                let Some((sid, vi)) = self.variant_on(&path, p.ty()) else {
                    unreachable!()
                };
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
                    return err(
                        r.span(),
                        format!(
                            "mismatched types: `{}` is not a reference",
                            self.prog.tn(p.ty())
                        ),
                    );
                };
                let (q, _) = self.pat_deref_once(p);
                self.pat_test(&r.pat, q, fail)
            }
            Pat::Or(o) => {
                let ok = self.b.create_block();
                let n = o.cases.len();
                for (i, case) in o.cases.iter().enumerate() {
                    let nf = if i + 1 == n {
                        fail
                    } else {
                        self.b.create_block()
                    };
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
                            let cc = if s {
                                IntCC::SignedGreaterThanOrEqual
                            } else {
                                IntCC::UnsignedGreaterThanOrEqual
                            };
                            let c = self.b.ins().icmp(cc, v, lo);
                            self.require(c, fail);
                        }
                        if let Some(hi) = &r.end {
                            let hi = self.pat_const(hi, t)?;
                            let cc = match (&r.limits, s) {
                                (syn::RangeLimits::Closed(_), true) => IntCC::SignedLessThanOrEqual,
                                (syn::RangeLimits::Closed(_), false) => {
                                    IntCC::UnsignedLessThanOrEqual
                                }
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
                        for (fi, sub) in
                            Self::tuple_fields(ts.elems.iter(), fields.len(), ts.span())?
                        {
                            let (_, fty, off) = fields[fi];
                            self.pat_test(sub, Self::field_place(p, fty, off), fail)?;
                        }
                        Ok(())
                    }
                    Pat::Tuple(pt) => {
                        let fields = self.tuple_pat_fields(p, pt.elems.len(), pt.span())?;
                        for (fi, sub) in
                            Self::tuple_fields(pt.elems.iter(), fields.len(), pt.span())?
                        {
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
    fn tuple_struct_fields(
        &self,
        path: &syn::Path,
        t: Ty,
        span: proc_macro2::Span,
    ) -> R<Vec<(String, Ty, u32)>> {
        let Some(sid) = self.prog.struct_by_path(path, self.self_sid) else {
            return err(span, self.no_variant_msg(path));
        };
        match t {
            Ty::Struct(x) if self.prog.base_of(x) == self.prog.base_of(sid) => {
                Ok(self.prog.structs[x as usize].fields.clone())
            }
            _ => err(
                span,
                format!(
                    "mismatched types: expected `{}`, found `{}`",
                    self.prog.tn(t),
                    self.prog.structs[sid as usize].name
                ),
            ),
        }
    }

    /// タプルパターンのフィールド (match 対象はタプルのインスタンス)
    fn tuple_pat_fields(
        &self,
        p: Place,
        n_pat: usize,
        span: proc_macro2::Span,
    ) -> R<Vec<(String, Ty, u32)>> {
        match p.ty() {
            Ty::Struct(x) if self.prog.is_tuple(x) => {
                Ok(self.prog.structs[x as usize].fields.clone())
            }
            t => err(
                span,
                format!(
                    "mismatched types: expected a tuple of {n_pat} elements, found `{}`",
                    self.prog.tn(t)
                ),
            ),
        }
    }

    fn pat_deref_once(&mut self, p: Place) -> (Place, bool) {
        match p {
            Place::Var(v, Ty::Ref(m, i)) => (Place::Mem(self.b.use_var(v), 0, *i), m),
            Place::Mem(a, o, Ty::Ref(m, i)) => {
                (Place::Mem(self.b.ins().load(PTR, mf(), a, o), 0, *i), m)
            }
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
        let fields: Vec<(String, Ty, u32)> =
            if let Some((sid, vi)) = self.variant_on(&ps.path, p.ty()) {
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
                    return err(
                        o.span(),
                        "bindings inside `|` patterns are not supported yet",
                    );
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
            Pat::Tuple(pt) => {
                let (p, mode) = self.pat_deref(p, mode);
                let fields = self.tuple_pat_fields(p, pt.elems.len(), pt.span())?;
                for (fi, sub) in Self::tuple_fields(pt.elems.iter(), fields.len(), pt.span())? {
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
    (prog.structs[sid as usize].variants[vi].kind == crate::front::VariantKind::Unit)
        .then_some(path)
}

fn has_bindings(p: &Pat) -> bool {
    match p {
        Pat::Ident(pi) if pi.ident == "None" && pi.subpat.is_none() => false,
        Pat::Ident(_) => true,
        Pat::Paren(pp) => has_bindings(&pp.pat),
        Pat::Reference(r) => has_bindings(&r.pat),
        Pat::Or(o) => o.cases.iter().any(has_bindings),
        Pat::TupleStruct(ts) => ts.elems.iter().any(has_bindings),
        Pat::Tuple(t) => t.elems.iter().any(has_bindings),
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
