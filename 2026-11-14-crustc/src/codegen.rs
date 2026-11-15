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
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
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
    /// この関数が参照する vtable (関数 ID の列)。.o では内容で重複を除く
    pub vtables: Vec<Vec<u32>>,
    /// panic の trap の場所: (関数の中のオフセット, 種類・行・列, メッセージ (datas の index。なければ u32::MAX))
    pub traps: Vec<(u32, u32, u32)>,
    /// 場所 (行) を機械語やデータに書き込んだなら、そのときのアイテムの開始行 - 1。
    /// キャッシュから読むときに今の開始行と違えばコンパイルし直す (行がずれたら場所も変わる)
    pub line_base: Option<u32>,
    /// キャッシュから読んだ機械語の参照 (長さ, ハッシュ)。中身は前回の `.o` から取り出す (code は空)
    pub code_ref: Option<(u32, u64)>,
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
    /// Blob::vtables の index
    Vtable(u32),
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
        Ty::F32 => types::F32,
        Ty::F64 => types::F64,
        t => match t.bits() {
            0 => return None,
            8 => types::I8,
            16 => types::I16,
            32 => types::I32,
            128 => types::I128,
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
        // (サイズ 0 のスカラー `()` の引数は渡さない)
        if !p.is_agg() && clif_ty(p).is_none() {
            continue;
        }
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
    vtables: Vec<Vec<u32>>,
    traps: Vec<(u32, u32)>,
    /// 次にコンパイルする関数のアイテムの開始行 - 1 (span の行はアイテムの中の行なので足す)
    pub line_base: u32,
    loc_used: bool,
}

/// panic の trap の最初のコード (1〜4 はほかの trap)。関数ごとに 245 か所まで場所を覚える
const TRAP_SITE_BASE: u8 = 5;
const TRAP_SITE_MAX: usize = 245;

/// UserExternalName の名前空間: 0 = 関数, 1 = 無名データ, 2 = static, 3 = vtable
pub const NS_DATA: u32 = 1;
pub const NS_STATIC: u32 = 2;
pub const NS_VTABLE: u32 = 3;

impl Worker {
    pub fn new() -> Self {
        Worker {
            ctx: Context::new(),
            fbc: FunctionBuilderContext::new(),
            requested: vec![],
            datas: vec![],
            vtables: vec![],
            traps: vec![],
            line_base: 0,
            loc_used: false,
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
        let (sig, env, body_id) = {
            let g = insts.lock().unwrap();
            (g.sig(prog, id).clone(), g.env(prog, id), g.body_of(id))
        };
        // クロージャの本体: キャプチャ (名前, 変数の型, by-ref か, オフセット)
        let mut caps: Vec<(String, Ty, bool, u32)> = vec![];
        if let Some(cd) = prog.closures.get(&body_id) {
            if let Some(Ty::Ptr(_, Ty::Struct(sid))) = sig.params.first() {
                let sd = &prog.structs[*sid as usize];
                for (n, (_, ft, off)) in cd.captures.iter().zip(&sd.fields) {
                    match (cd.by_move, ft) {
                        (false, Ty::Ptr(_, t)) => caps.push((n.clone(), **t, true, *off)),
                        _ => caps.push((n.clone(), *ft, false, *off)),
                    }
                }
            }
        }
        // この本体に書かれたクロージャ (前順) の番号: 親なら 0 から、クロージャ k なら k+1 から
        let (root, base_k) = match prog.closures.get(&body_id) {
            Some(cd) => (cd.parent, cd.k + 1),
            None => (body_id, 0),
        };
        let mut closure_ix = HashMap::new();
        let (mut taken, closures) = addr_taken(f);
        for (i, c) in closures.into_iter().enumerate() {
            let k = base_k + i as u32;
            closure_ix.insert(c, k);
            // 参照でキャプチャされる変数はスタックに置く
            if let Some(cd) = prog.closure_by.get(&(root, k)).and_then(|i| prog.closures.get(i)) {
                if !cd.by_move {
                    taken.extend(cd.captures.iter().cloned());
                }
            }
        }
        let extra: Vec<(String, Ty)> = caps.iter().map(|c| (c.0.clone(), c.1)).collect();
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
                // (ユーザのコードだけ検査する: prelude は折り返す演算 (ハッシュなど) を書いている)
                user_code: prog.item_of.get(body_id as usize).copied().flatten().is_some_and(|i| i < prog.prelude_from),
                overflow_checks: prog.overflow_checks
                    && (prog.item_of.get(body_id as usize).copied().flatten().is_some_and(|i| i < prog.prelude_from)
                        || f.attrs.iter().any(|a| a.path().is_ident("rustc_inherit_overflow_checks"))),
                traps: vec![],
                line_base: self.line_base,
                loc_used: false,
                index_panic_block: None,
                caller_loc: matches!(
                    prog.names.get(body_id as usize).map(|n| n.as_str()),
                    Some("Option::unwrap" | "Option::expect" | "Result::unwrap" | "Result::expect" | "Result::unwrap_err" | "Result::expect_err")
                ) && prog.item_of.get(body_id as usize).copied().flatten().is_some_and(|i| i >= prog.prelude_from),
                self_sid: env.self_sid,
                sret: None,
                frefs: HashMap::new(),
                lets: crate::infer::infer_ctx(prog, f, sig, &env, &extra, Some(sig.ret), Some((root, base_k))).0,
                addr_taken: taken,
                caps,
                root,
                closure_ix,
                closure_hint: None,
                seg_self: None,
                loop_bounds: vec![],
                pending_bound: None,
                checked: (None, vec![]),
                in_fmt_args: 0,
                owned: vec![],
                temps: vec![],
                owned_binds: 0,
                last_scrut: None,
                env,
                insts,
                requested: vec![],
                datas: vec![],
                vtables: vec![],
                gvs: HashMap::new(),
                cmp_depth: 0,
                pending_label: None,
            };
            let r = if prog.index.get(crate::prelude::DROP_GLUE) == Some(&body_id) {
                cx.drop_glue_body(sig)
            } else if prog.index.get("__exr_hash") == Some(&body_id) {
                cx.hash_glue_body(sig)
            } else if prog.index.get("__exr_cmp") == Some(&body_id) {
                cx.cmp_glue_body(sig)
            } else if prog.index.get("__exr_eq") == Some(&body_id) {
                cx.eq_glue_body(sig)
            } else if prog.index.get("__exr_clone") == Some(&body_id) {
                cx.clone_glue_body(sig)
            } else if prog.index.get("__exr_default") == Some(&body_id) {
                cx.default_glue_body(sig)
            } else if prog.index.get("__exr_display") == Some(&body_id) {
                cx.fmt_glue_body(sig, false)
            } else if prog.index.get("__exr_debug") == Some(&body_id) {
                cx.fmt_glue_body(sig, true)
            } else {
                cx.body(f, sig)
            };
            self.requested = std::mem::take(&mut cx.requested);
            self.datas = std::mem::take(&mut cx.datas);
            self.vtables = std::mem::take(&mut cx.vtables);
            self.traps = std::mem::take(&mut cx.traps);
            self.loc_used = cx.loc_used;
            r?;
            cx.b.seal_all_blocks();
            cx.b.finalize(isa.frontend_config());
        }
        Ok(())
    }

    fn emit(&mut self, prog: &Program, id: u32, isa: &dyn TargetIsa) -> R<Blob> {
        self.ctx
            .compile(isa, &mut ControlPlane::default())
            .map_err(|e| {
                let n = prog.names.get(id as usize).cloned().unwrap_or_else(|| format!("instance #{id}"));
                format!("{n}: codegen error: {:?}", e.inner)
            })?;
        let code = self.ctx.compiled_code().unwrap();
        let mut relocs = vec![];
        for r in code.buffer.relocs() {
            let target = match r.target {
                RelocTarget::ExternalName(ExternalName::User(reff)) => {
                    let n = &self.ctx.func.params.user_named_funcs()[reff];
                    match n.namespace {
                        NS_DATA => Target::Data(n.index),
                        NS_STATIC => Target::Static(n.index),
                        NS_VTABLE => Target::Vtable(n.index),
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
        // panic の trap (コード 5 + k) の場所と、その種類・行・列・メッセージ
        let infos = std::mem::take(&mut self.traps);
        let mut traps = vec![];
        for t in code.buffer.traps() {
            let c = t.code.as_raw().get() as usize;
            // (表には 1 か所 8 byte で載せるので、関数の先頭から 1 MiB を超える場所は載せない)
            if (TRAP_SITE_BASE as usize..TRAP_SITE_BASE as usize + infos.len()).contains(&c) && t.offset < 1 << 20 {
                let (info, msg) = infos[c - TRAP_SITE_BASE as usize];
                traps.push((t.offset, info, msg));
            }
        }
        Ok(Blob {
            align: code.buffer.min_alignment.max(1),
            code: code.buffer.data().to_vec(),
            relocs,
            datas: std::mem::take(&mut self.datas),
            vtables: std::mem::take(&mut self.vtables),
            traps,
            line_base: self.loc_used.then_some(self.line_base),
            code_ref: None,
        })
    }
}

/// `&x` / `&mut x.f` などでアドレスを取られるローカル変数名 (shadowing は無視した過大近似)と、
/// 本体のクロージャ式 (前順。[`crate::closure::closures_of`] と同じ順)。1 回の走査で集める
fn addr_taken(f: &syn::ItemFn) -> (HashSet<String>, Vec<(usize, usize)>) {
    struct V(HashSet<String>, Vec<(usize, usize)>);
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
        fn visit_expr_closure(&mut self, c: &'ast syn::ExprClosure) {
            self.1.push(crate::closure::closure_key(c));
            syn::visit::visit_expr_closure(self, c);
        }
        // (マクロの引数の中のクロージャも、要約と同じ順に数える)
        fn visit_macro(&mut self, m: &'ast syn::Macro) {
            // (`assert_eq!(a, b)` は `let l = &(a);` に展開して値を出すので、a と b の変数は借用される)
            if ["assert_eq", "assert_ne", "debug_assert_eq", "debug_assert_ne"].iter().any(|n| m.path.is_ident(n)) {
                use syn::punctuated::Punctuated;
                if let Ok(es) = m.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated) {
                    for e in es.iter().take(2) {
                        if let Some(n) = root(e) {
                            self.0.insert(n);
                        }
                    }
                }
            }
            let es = crate::closure::macro_exprs(m);
            if es.is_empty() {
                // (クロージャのない引数 (`println!("{:?}", a.partial_cmp(&n))`) の借用も数える)
                use syn::punctuated::Punctuated;
                // (引数そのものの `&x` は書式化が値として読むので数えない: レジスタのままにする)
                if let Ok(es) = m.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated) {
                    for e in es.iter() {
                        match e {
                            Expr::Reference(r) => self.visit_expr(&r.expr),
                            Expr::Path(_) | Expr::Lit(_) => {}
                            e => self.visit_expr(e),
                        }
                    }
                }
            }
            for e in es {
                self.visit_expr(&e);
            }
        }
        fn visit_item(&mut self, _: &'ast syn::Item) {}
    }
    let mut v = V(HashSet::new(), vec![]);
    v.visit_block(&f.block);
    (v.0, v.1)
}

/// 添字の検査を覚えるときの長さ (定数なら値で比べる: 配列の長さは場所ごとに iconst を作り直すため)
#[derive(Clone, Copy, PartialEq)]
enum LenKey {
    Const(i64),
    Val(Value),
}

/// 式の評価結果。Unit / Never のとき値は None。struct のときはアドレス。
type V = (Option<Value>, Ty);

#[derive(Clone, Copy, PartialEq)]
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
    /// `'outer: loop` のラベル
    label: Option<String>,
    /// `break v` の値の型 (brk の引数で渡す)。ラベル付きブロックは continue できない
    val: Option<Ty>,
    is_block: bool,
    /// ループに入ったときのスコープの深さと一時値の数 (break / continue でそこまで drop する)
    depth: usize,
    temps: usize,
}

/// drop の責任を持つ値 (所有しているローカル変数・一時値)。flag が 1 のときだけ drop する
/// (ムーブしたら 0 にする。分岐によってムーブしたりしなかったりするので実行時のフラグにする)
#[derive(Clone, Copy)]
struct Owned {
    loc: Loc,
    ty: Ty,
    flag: StackSlot,
    /// ローカル変数ならスコープの深さ
    depth: usize,
}

struct Cx<'a, 'p> {
    b: FunctionBuilder<'a>,
    prog: &'p Program,
    cc: CallConv,
    scopes: Vec<Vec<(String, Loc, Ty)>>,
    loops: Vec<Loop>,
    /// 次に作るループに付けるラベル
    pending_label: Option<String>,
    ret: Ty,
    is_main: bool,
    /// ユーザのコード (prelude でない) か。0 除算の検査はこれだけで入れる
    user_code: bool,
    /// 算術のオーバーフロー検査 (rustc の debug ビルドと同じ。-O なら入れない)
    overflow_checks: bool,
    /// panic の trap の情報 (trap コード 5 + k が k 番目): (種類・行・列, メッセージの datas の index)
    traps: Vec<(u32, u32)>,
    /// アイテムの開始行 - 1 と、場所を書き込んだか
    line_base: u32,
    loc_used: bool,
    /// 添字の範囲外で panic する (関数で共有する) ブロック
    index_panic_block: Option<Block>,
    /// `Option::unwrap` などの本体 (#[track_caller] 相当: panic の場所は呼んだ側)
    caller_loc: bool,
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
    vtables: Vec<Vec<u32>>,
    gvs: HashMap<(u32, u32), ir::GlobalValue>,
    /// glue なしで大小比較を展開している深さ (参照で再帰する型の歯止め)
    cmp_depth: u32,
    /// クロージャの本体なら、キャプチャ (名前, 型, by-ref か, `__env` 内のオフセット)
    caps: Vec<(String, Ty, bool, u32)>,
    /// クロージャを書いた大元の関数と、この本体のクロージャ式 (アドレス) → 番号
    root: u32,
    closure_ix: HashMap<(usize, usize), u32>,
    /// 次に作るクロージャの引数・戻り値の型の手がかり (呼び先の `F: Fn(A) -> B` から)
    closure_hint: Option<(Vec<Ty>, Option<Ty>)>,
    /// `Buf::<8>::new()` の型のセグメントの型 (呼ぶ関数の self 型から型引数を解く)
    seg_self: Option<Ty>,
    /// `for k in 0..C` (C は定数) のループ変数の場所と、その上限 (これより小さい)。配列の添字の検査を省く
    loop_bounds: Vec<(Loc, u64)>,
    /// for_range が次の for_body に渡す上限
    pending_bound: Option<u64>,
    /// 直前の添字の検査: (検査を通ったあとのブロック, (添字, 長さ)) の列。同じブロックの中なら同じ検査を省く
    checked: (Option<Block>, Vec<(Value, LenKey)>),
    /// 書式マクロの引数を評価している深さ (型推論の記録をソース上の位置で引く)
    in_fmt_args: u32,
    /// drop の責任を持つローカル変数 (スコープの深さ順) と、文の終わりで drop する一時値
    owned: Vec<Owned>,
    temps: Vec<Owned>,
    /// drop が要る値を値で束縛した回数 (パターンが match 対象からムーブしたかの判定用)
    owned_binds: usize,
    /// 直前の match 対象が一時値なら、その drop フラグ
    last_scrut: Option<StackSlot>,
}

/// 間接呼び出しの呼び先
#[derive(Clone, Copy)]
enum Indirect {
    /// `&dyn` の (データ, vtable) のアドレスと vtable の番号。データのポインタを第 1 引数に渡す
    Vtable(Value, usize),
    /// 関数ポインタ
    Ptr(Value),
}

/// 静的データに昇格できる定数式か (リテラル・構築・const・unit バリアントだけでできている)
fn is_promotable(cx: &Cx, e: &Expr) -> bool {
    match e {
        Expr::Lit(l) => !matches!(l.lit, Lit::Str(_) | Lit::ByteStr(_)),
        Expr::Paren(p) => is_promotable(cx, &p.expr),
        Expr::Unary(u) => matches!(u.op, UnOp::Neg(_)) && is_promotable(cx, &u.expr),
        Expr::Struct(s) => s.rest.is_none() && s.fields.iter().all(|f| is_promotable(cx, &f.expr)),
        Expr::Tuple(t) => t.elems.iter().all(|x| is_promotable(cx, x)),
        Expr::Array(a) => a.elems.iter().all(|x| is_promotable(cx, x)),
        Expr::Call(c) => {
            matches!(&*c.func, Expr::Path(p)
                if cx.prog.variant_by_path(&p.path, cx.self_sid).is_some()
                    || cx.prog.struct_by_path(&p.path, cx.self_sid).is_some())
                && c.args.iter().all(|x| is_promotable(cx, x))
        }
        Expr::Path(p) => {
            cx.const_of(p).is_some()
                || (cx.prog.variant_by_path(&p.path, cx.self_sid).is_some()
                    && p.path.get_ident().is_none_or(|i| cx.lookup(&i.to_string()).is_none()))
        }
        _ => false,
    }
}

/// `_` の穴を含む型か
fn has_hole(prog: &Program, t: Ty) -> bool {
    match t {
        Ty::Param(i) => i == crate::front::HOLE,
        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => has_hole(prog, *i),
        Ty::Struct(sid) => prog.structs[sid as usize].args.iter().any(|&a| has_hole(prog, a)),
        _ => false,
    }
}

/// `Iterator<Item = T>` の (Item, T)
fn assoc_constraint(b: &str) -> Option<(String, crate::front::TySyn)> {
    if !b.contains('=') || crate::front::parse_fn_bound(b).is_some() {
        return None;
    }
    let tb: syn::TraitBound = syn::parse_str(b).ok()?;
    let syn::PathArguments::AngleBracketed(ab) = &tb.path.segments.last()?.arguments else {
        return None;
    };
    ab.args.iter().find_map(|a| match a {
        syn::GenericArgument::AssocType(at) => Some((
            at.ident.to_string(),
            crate::front::parse_tysyn(&at.ty, None).ok()?,
        )),
        _ => None,
    })
}

fn m_name(mc: &syn::ExprMethodCall) -> String {
    mc.method.to_string()
}

fn strip_paren(e: &Expr) -> &Expr {
    match e {
        Expr::Paren(p) => strip_paren(&p.expr),
        Expr::Group(g) => strip_paren(&g.expr),
        e => e,
    }
}

fn is_place_expr(e: &Expr) -> bool {
    match e {
        Expr::Path(p) => p.qself.is_none() && p.path.get_ident().is_some(),
        Expr::Field(_) => true,
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

    /// ローカル変数 (スカラー) の値
    fn loc_value(&mut self, l: Loc) -> Option<Value> {
        match l {
            Loc::Var(v) => Some(self.b.use_var(v)),
            Loc::Slot(ss) => Some(self.b.ins().stack_load(PTR, PTR, ss, 0)),
            Loc::Ptr(a) => Some(self.b.ins().load(PTR, mf(), a, 0)),
            Loc::Zst => None,
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
        if self.needs_drop(ty) {
            self.owned_binds += 1;
            self.own_local(loc, ty);
        }
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
        let mut params = params.into_iter();
        for (arg, &ty) in f.sig.inputs.iter().zip(&sig.params) {
            let name = match arg {
                syn::FnArg::Receiver(_) => "self".to_string(),
                syn::FnArg::Typed(pt) => pat_name(&pt.pat)?,
            };
            if !ty.is_agg() && clif_ty(ty).is_none() {
                self.bind(name, ty, None);
                continue;
            }
            let Some(v) = params.next() else {
                return Err("internal: parameter count mismatch".into());
            };
            if ty.is_agg() {
                // 呼び出し側がコピー済みなので、そのメモリをそのまま使う (値渡しの引数は呼び出された側が drop する)
                self.scopes
                    .last_mut()
                    .unwrap()
                    .push((name, Loc::Ptr(v), ty));
                self.own_local(Loc::Ptr(v), ty);
            } else {
                self.bind(name, ty, Some(v));
            }
        }
        // クロージャ: キャプチャを `__env` のフィールド (by-ref なら指す先) として見せる
        if !self.caps.is_empty() {
            let env = match self.lookup("__env") {
                Some((l, _)) => self.loc_value(l),
                None => None,
            };
            let env = env.ok_or("closure without environment")?;
            for (n, t, by_ref, off) in self.caps.clone() {
                let a = if by_ref {
                    self.b.ins().load(PTR, mf(), env, off as i32)
                } else {
                    self.addr_plus(env, off as i32)
                };
                let loc = if clif_ty(t).is_none() && !t.is_agg() {
                    Loc::Zst
                } else {
                    Loc::Ptr(a)
                };
                self.scopes.last_mut().unwrap().push((n, loc, t));
            }
        }
        // std のプログラムの main: trap (添字の範囲外・unwrap の失敗・panic!) を rustc の panic と同じく
        // 終了コード 101 にするシグナルハンドラを最初に登録する
        if self.is_main && self.prog.panic_handler.is_none() && self.prog.index.contains_key("__exr_rt_init") {
            let span = proc_macro2::Span::call_site();
            let _ = self.call_named("__exr_rt_init", vec![Ty::Unit], &[], span);
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
        self.drop_for_exit(0, 0);
        self.emit_return(v);
        Ok(())
    }

    fn emit_return(&mut self, v: Option<Value>) {
        if self.is_main {
            // `fn main() -> Result<(), E>`: Err なら `Error: {:?}` を出して 1 で終わる
            if let (Ty::Struct(rs), Some(v)) = (self.ret, v) {
                let rd = &self.prog.structs[rs as usize];
                if rd.is_enum && rd.base.is_some_and(|b| self.prog.structs[b as usize].name == "Result") {
                    if let Some(ev) = rd.variants.iter().position(|x| x.name == "Err") {
                        let (_, et, off) = rd.variants[ev].fields[0];
                        let ok = self.b.create_block();
                        let span = proc_macro2::Span::call_site();
                        if self.test_variant(Place::Mem(v, 0, self.ret), rs, ev, span, ok).is_ok() {
                            let pa = self.addr_plus(v, off as i32);
                            let _ = self.call_named("__exr_main_err", vec![et], &[(pa, Ty::Ref(false, intern(et)))], span);
                            let one = self.b.ins().iconst(types::I32, 1);
                            self.b.ins().return_(&[one]);
                        }
                        self.b.switch_to_block(ok);
                    }
                }
            }
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
            let t0 = self.temps.len();
            match st {
                Stmt::Local(l) if !is_simple_let(&l.pat) => {
                    // 分解する let (`let S { x, y } = s;`): 反駁不能パターンとして束縛だけ行う
                    let Some(init) = &l.init else {
                        return err(l.span(), "`let` without initializer is not supported");
                    };
                    let ann = match &l.pat {
                        Pat::Type(pt) => self.prog.parse_ty(&pt.ty, &self.env).ok(),
                        _ => None,
                    };
                    let Some(place) = self.scrutinee_as(&init.expr, ann)? else {
                        diverged = true;
                        continue;
                    };
                    // `let PAT = e else { .. };`: 合わなければ else の (発散する) ブロックへ
                    if let Some((_, else_e)) = &init.diverge {
                        let else_blk = self.b.create_block();
                        self.pat_test(&l.pat, place, else_blk)?;
                        let cur = self.b.current_block().unwrap();
                        self.b.switch_to_block(else_blk);
                        let (_, et) = self.expr(else_e, Some(Ty::Never))?;
                        if et != Ty::Never {
                            return err(else_e.span(), "the `else` block of `let ... else` must diverge (return / break / panic)");
                        }
                        self.b.ins().trap(TrapCode::unwrap_user(2));
                        self.b.switch_to_block(cur);
                    }
                    let ob = self.owned_binds;
                    self.pat_bind(&l.pat, place, BindMode::Move)?;
                    if self.owned_binds > ob {
                        let f = self.last_scrut;
                        self.release(&init.expr, f);
                    }
                    self.extend_temps(t0);
                }
                Stmt::Local(l) if matches!(l.pat, Pat::Wild(_)) => {
                    // `let _ = e;`: 場所ならムーブしない。値ならすぐ drop する
                    let Some(init) = &l.init else {
                        return err(l.span(), "`let` without initializer is not supported");
                    };
                    if is_place_expr(&init.expr) {
                        let _ = self.place(&init.expr)?;
                    } else {
                        let (v, t) = self.expr(&init.expr, None)?;
                        if t == Ty::Never {
                            diverged = true;
                            continue;
                        }
                        if let Some(v) = v {
                            self.own_temp(v, t);
                        }
                    }
                    self.drop_temps(t0);
                }
                Stmt::Local(l) => {
                    let (name, ann) = match &l.pat {
                        Pat::Type(pt) => (
                            pat_name(&pt.pat)?,
                            Some(self.prog.parse_ty(&pt.ty, &self.env)?),
                        ),
                        // (型推論は強制変換の行き先 `[T]` などを入れてしまうことがあるので、サイズ不定の型は使わない)
                        p => (
                            pat_name(p)?,
                            self.lets
                                .get(&crate::infer::let_key(l))
                                .copied()
                                .filter(|t| !matches!(t, Ty::Slice(_) | Ty::Str | Ty::Dyn(_))),
                        ),
                    };
                    // (assert_eq! の展開: 右の値の型は左と同じ (`assert_eq!(o, None)`))
                    // (文字列のリテラルは除く: `String` と `"x"` は型が違ったまま比べる)
                    let hintable = |e: &Expr| {
                        let e = match e {
                            Expr::Reference(r) => match &*r.expr {
                                Expr::Paren(p) => &*p.expr,
                                e => e,
                            },
                            e => e,
                        };
                        !matches!(e, Expr::Lit(syn::ExprLit { lit: Lit::Str(_), .. }))
                    };
                    let other = match name.as_str() {
                        "__exr_r" => Some("__exr_l"),
                        "__exr_l" => Some("__exr_r"),
                        _ => None,
                    };
                    let hint = match (&l.init, other) {
                        (Some(i), Some(o)) if ann.is_none() && hintable(&i.expr) => self.lookup(o).map(|x| x.1),
                        _ => None,
                    };
                    let Some(init) = &l.init else {
                        return err(l.span(), "`let` without initializer is not supported");
                    };
                    if init.diverge.is_some() {
                        return err(l.span(), "`let ... else` is not supported");
                    }
                    let (v, t) = self.expr(&init.expr, ann.or(hint))?;
                    if t == Ty::Never {
                        diverged = true;
                        continue;
                    }
                    // `let v: Vec<_> = ..`: 穴のある注釈は期待型としてだけ使う
                    let ann = ann.filter(|&a| !has_hole(self.prog, a));
                    let ty = ann.unwrap_or(t);
                    self.same(init.expr.span(), t, ty)?;
                    self.bind(name, ty, v);
                    self.extend_temps(t0);
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
                        self.drop_tail_temps(t0, result);
                    } else {
                        if semi.is_none() && !matches!(r.1, Ty::Unit | Ty::Never) {
                            return err(e.span(), "expected `;` after non-unit expression");
                        }
                        // 捨てる値と、文の一時値を drop する
                        if let (Some(v), t) = r {
                            if t != Ty::Never {
                                self.own_temp(v, t);
                            }
                        }
                        self.drop_temps(t0);
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
                        self.drop_tail_temps(t0, result);
                    } else {
                        self.drop_temps(t0);
                    }
                }
            }
        }
        self.pop_scope();
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
        // (書式マクロの引数の中は位置で記録されている)
        if self.in_fmt_args > 0 {
            if let Some(&t) = self.lets.get(&crate::infer::pos_key(e)) {
                return Some(t);
            }
        }
        self.peek_syn(e)
    }

    fn peek_syn(&self, e: &Expr) -> Option<Ty> {
        match e {
            // `f64::NAN` / `std::f64::consts::PI` / `PI` (use したもの)
            Expr::Path(p)
                if p.path.segments.len() >= 2
                    && p.path.segments.iter().any(|s| s.ident == "f64" || s.ident == "f32")
                    && p.path.segments.last().is_some_and(|s| s.ident.to_string().chars().all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())) =>
            {
                Some(if p.path.segments.iter().any(|s| s.ident == "f32") { Ty::F32 } else { Ty::F64 })
            }
            // `u64::MAX` / `i128::MIN` / `u32::BITS`
            Expr::Path(p)
                if p.path.segments.len() == 2
                    && matches!(p.path.segments[1].ident.to_string().as_str(), "MAX" | "MIN" | "BITS")
                    && Ty::from_suffix(&p.path.segments[0].ident.to_string()).is_some() =>
            {
                if p.path.segments[1].ident == "BITS" {
                    Some(Ty::U32)
                } else {
                    Ty::from_suffix(&p.path.segments[0].ident.to_string())
                }
            }
            Expr::Path(p) if p.path.segments.len() == 2 && self.const_of(p).is_some() => {
                Some(self.prog.consts[self.const_of(p)? as usize].ty)
            }
            Expr::Path(p) if self.const_generic(p).is_some() => Some(Ty::Usize),
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
                Lit::Int(i) if i.suffix() == "f32" => Some(Ty::F32),
                Lit::Int(i) if i.suffix() == "f64" => Some(Ty::F64),
                Lit::Float(f) => Some(if f.suffix() == "f32" { Ty::F32 } else { Ty::F64 }),
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
            // タプル構造体の構築 `V(1, 2)` (ジェネリックでないもの)
            Expr::Call(c)
                if matches!(&*c.func, Expr::Path(p) if p.path.segments.len() == 1
                    && self.lookup(&p.path.segments[0].ident.to_string()).is_none()
                    && self.prog.struct_by_path(&p.path, self.self_sid)
                        .is_some_and(|s| self.prog.structs[s as usize].generics.is_empty())) =>
            {
                let Expr::Path(p) = &*c.func else { unreachable!() };
                self.prog.struct_by_path(&p.path, self.self_sid).map(Ty::Struct)
            }
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
                .map(|(_, t)| t)
                .or_else(|| self.float_const_name(p).map(|_| Ty::F64)),
            Expr::Call(c) if self.prog.prim_method_call(c, self.self_sid).is_some() => {
                self.peek(&self.prog.prim_method_call(c, self.self_sid)?)
            }
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
                let name = mc.method.to_string();
                // (`x.to_string()` はどの型でも String)
                if name == "to_string" && mc.args.is_empty() {
                    if let Some(&s) = self.prog.struct_index.get("String") {
                        return Some(Ty::Struct(s));
                    }
                }
                // &str の組み込み / prelude の関数 (マクロの引数の中は型推論の記録がない)
                if t == Ty::Str {
                    return match name.as_str() {
                        "trim" | "as_str" => Some(Ty::Ref(false, intern(Ty::Str))),
                        "starts_with" | "ends_with" | "contains" | "is_empty" => Some(Ty::Bool),
                        "len" => Some(Ty::Usize),
                        _ => None,
                    };
                }
                // 整数・浮動小数点の組み込み (`x.pow(3).trailing_zeros()` の途中など)
                if t.is_int() || t.is_float() {
                    return match name.as_str() {
                        "count_ones" | "count_zeros" | "leading_zeros" | "trailing_zeros" | "leading_ones"
                        | "trailing_ones" => Some(Ty::U32),
                        "is_power_of_two" | "is_positive" | "is_negative" | "is_nan" | "is_finite"
                        | "is_infinite" | "is_sign_negative" | "is_sign_positive" => Some(Ty::Bool),
                        "pow" | "wrapping_add" | "wrapping_sub" | "wrapping_mul" | "wrapping_neg" | "saturating_add"
                        | "saturating_sub" | "abs" | "min" | "max" | "rem_euclid" | "div_euclid"
                        | "next_power_of_two" | "rotate_left" | "rotate_right" | "swap_bytes" | "signum"
                        | "wrapping_pow" | "saturating_pow" | "isqrt" | "sqrt" | "floor" | "ceil" | "round"
                        | "trunc" | "powi" | "powf" | "exp" | "ln" | "sin" | "cos" | "tan" | "clamp" => Some(t),
                        _ => None,
                    };
                }
                let Ty::Struct(sid) = t else { return None };
                let id = self
                    .prog
                    .method(sid, &name)
                    .or_else(|| self.prog.default_method(sid, &name))?;
                // 型パラメータは受け手から分かる分だけ (Self / impl の型パラメータ) 入れる
                let sig = &self.prog.sigs[id as usize];
                let mut sol = vec![None; sig.generics.len()];
                if sig.trait_of.is_some() {
                    if let Some(s0) = sol.first_mut() {
                        *s0 = Some(Ty::Struct(sid));
                    }
                } else if let Some(st) = sig.self_ty {
                    self.prog.match_ty(st, Ty::Struct(sid), &mut sol);
                }
                Some(self.prog.partial_subst(sig.ret, &sol)).filter(|&t| !self.prog.has_param(t))
                .filter(|&t| !self.prog.has_param(t))
            }
            Expr::Paren(p) => self.peek(&p.expr),
            Expr::Group(g) => self.peek(&g.expr),
            // `format!(..)` は String
            Expr::Macro(m) if m.mac.path.is_ident("format") => self.prog.struct_index.get("String").map(|&s| Ty::Struct(s)),
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => match self.peek(&u.expr)? {
                Ty::Ref(_, i) => Some(*i),
                _ => None,
            },
            Expr::Unary(u) => self.peek(&u.expr),
            // `&v[a..b]`: スライス (文字列なら &str)
            Expr::Reference(r) if matches!(strip_paren(&r.expr), Expr::Index(ix) if matches!(&*ix.index, Expr::Range(_))) => {
                let Expr::Index(ix) = strip_paren(&r.expr) else { unreachable!() };
                let mut t = self.peek(&ix.expr)?;
                while let Ty::Ref(_, i) = t {
                    if t.is_fat() {
                        break;
                    }
                    t = *i;
                }
                let m = r.mutability.is_some();
                match t {
                    Ty::Array(e, _) => Some(Ty::Ref(m, intern(Ty::Slice(e)))),
                    Ty::Ref(_, Ty::Slice(e)) => Some(Ty::Ref(m, intern(Ty::Slice(e)))),
                    Ty::Ref(_, Ty::Str) => Some(Ty::Ref(m, intern(Ty::Str))),
                    Ty::Struct(sid) => match self.vec_fields(sid) {
                        Some((_, _, e)) => Some(Ty::Ref(m, intern(Ty::Slice(intern(e))))),
                        None if self.prog.structs[sid as usize].name == "String" => Some(Ty::Ref(m, intern(Ty::Str))),
                        None => None,
                    },
                    _ => None,
                }
            }
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
        let by_ty = |t: Ty, m: &str| -> Option<u32> {
            match t {
                Ty::Struct(sid) => self.prog.method(sid, m).or_else(|| self.prog.default_method(sid, m)),
                t => self.prog.prim_method(t, m),
            }
        };
        if let Some(q) = &p.qself {
            let t = self.prog.parse_ty(&q.ty, &self.env).ok()?;
            let m = p.path.segments.last()?.ident.to_string();
            let i = by_ty(t, &m)?;
            return Some((i, &self.prog.sigs[i as usize]));
        }
        // `T::f(..)` (T は型引数。インスタンスでは具体的な型)
        if p.path.segments.len() == 2 {
            let g = p.path.segments[0].ident.to_string();
            if self.env.params.iter().any(|(n, _)| *n == g) {
                let ty: syn::Type = syn::Type::Path(syn::TypePath { qself: None, path: syn::Path::from(p.path.segments[0].ident.clone()) });
                if let Ok(t) = self.prog.parse_ty(&ty, &self.env) {
                    let m = p.path.segments[1].ident.to_string();
                    if let Some(i) = by_ty(t, &m) {
                        return Some((i, &self.prog.sigs[i as usize]));
                    }
                }
            }
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
            Expr::Path(p) if self.const_generic(p).is_some() => {
                let n = self.const_generic(p).unwrap();
                let v = self.b.ins().iconst(types::I64, n as i64);
                let var = self.b.declare_var(types::I64);
                self.b.def_var(var, v);
                Ok(Place::Var(var, Ty::Usize))
            }
            // const (`LIMIT` / `S::N`) を場所として使う (`println!("{}", LIMIT)` / `&LIMIT`): 一時領域に置く
            Expr::Path(p) if self.const_of(p).is_some() && p.path.get_ident().is_none_or(|id| self.lookup(&id.to_string()).is_none()) => {
                let ci = self.const_of(p).unwrap();
                let (v, t) = self.const_value(p, ci)?;
                match v {
                    Some(a) if t.is_agg() => Ok(Place::Mem(a, 0, t)),
                    Some(v) => {
                        let var = self.b.declare_var(clif_ty(t).unwrap_or(PTR));
                        self.b.def_var(var, v);
                        Ok(Place::Var(var, t))
                    }
                    None => Ok(Place::Zst(t)),
                }
            }
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
                        // unit struct の値 (`struct U;` の `U`)
                        _ => match self.prog.struct_by_path(&p.path, self.self_sid) {
                            Some(sid)
                                if self.prog.structs[sid as usize].fields.is_empty()
                                    && !self.prog.structs[sid as usize].is_enum
                                    && self.prog.structs[sid as usize].generics.is_empty() =>
                            {
                                let t = Ty::Struct(sid);
                                Ok(Place::Mem(self.temp(t), 0, t))
                            }
                            // `use std::f64::consts::PI;` してからの `PI`
                            _ => match self.float_const_name(p) {
                                Some(x) => {
                                    let v = self.b.ins().f64const(x);
                                    let var = self.b.declare_var(types::F64);
                                    self.b.def_var(var, v);
                                    Ok(Place::Var(var, Ty::F64))
                                }
                                None => err(p.span(), format!("cannot find value `{id}`")),
                            },
                        },
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
                // `*b` は b をムーブしない
                let (v, t) = self.read_no_move(&u.expr)?;
                match (v, t) {
                    (Some(_), t) if t.is_fat() => err(
                        u.span(),
                        "dereferencing a slice is not supported (index it instead)",
                    ),
                    (Some(p), Ty::Ref(_, inner) | Ty::Ptr(_, inner) | Ty::Box(inner)) => {
                        Ok(Place::Mem(p, 0, *inner))
                    }
                    _ => err(
                        u.span(),
                        format!("type `{}` cannot be dereferenced", self.prog.tn(t)),
                    ),
                }
            }
            // `map[&k]`: get(&k).unwrap() の指す先
            Expr::Index(ix)
                if matches!(self.peek(&ix.expr).map(|t| self.strip_refs(t)),
                    Some(Ty::Struct(sid)) if self.prog.structs[self.prog.base_of(sid) as usize].name == "HashMap") =>
            {
                let b = &ix.expr;
                let k = &ix.index;
                let e: Expr = syn::parse_quote!((#b).get(#k).unwrap());
                let (v, t) = self.expr(&e, None)?;
                match (v, t) {
                    (Some(p), Ty::Ref(_, inner)) => Ok(Place::Mem(p, 0, *inner)),
                    _ => err(ix.span(), "internal: bad map index"),
                }
            }
            // `d[i]` / `m[&k]` (impl Index / IndexMut、VecDeque / BTreeMap): `*x.index_mut(i)` / `*x.index(i)`
            Expr::Index(ix)
                if matches!(self.peek(&ix.expr).map(|t| self.strip_refs(t)),
                    Some(Ty::Struct(sid)) if self.vec_fields(sid).is_none()
                        && (self.prog.method(sid, "index").is_some() || self.prog.method(sid, "index_mut").is_some())) =>
            {
                let Some(Ty::Struct(sid)) = self.peek(&ix.expr).map(|t| self.strip_refs(t)) else { unreachable!() };
                let m = if self.prog.method(sid, "index_mut").is_some() { "index_mut" } else { "index" };
                let mi = syn::Ident::new(m, ix.span());
                let (b, k) = (&ix.expr, &ix.index);
                let e: Expr = syn::parse_quote!((#b).#mi(#k));
                let (v, t) = self.expr(&e, None)?;
                match (v, t) {
                    (Some(p), Ty::Ref(_, inner)) => Ok(Place::Mem(p, 0, *inner)),
                    _ => err(ix.span(), "`index` must return a reference"),
                }
            }
            Expr::Index(ix) if !matches!(&*ix.index, Expr::Range(_)) => {
                let (base, len, elem) = self.indexable(&ix.expr)?;
                let (i, it) = self.expr(&ix.index, Some(Ty::Usize))?;
                let Some(i) = i else {
                    return err(ix.span(), "index diverges");
                };
                self.same(ix.index.span(), it, Ty::Usize)?;
                // (`for k in 0..3` の k で長さ 3 以上の配列を引くなら、範囲内と分かっている)
                let in_range = match (&*ix.index, self.iconst_of(len)) {
                    (Expr::Path(p), Some(n)) => p.path.get_ident().and_then(|id| self.lookup(&id.to_string())).is_some_and(|(l, _)| {
                        self.loop_bounds.iter().any(|&(bl, h)| bl == l && h <= n as u64)
                    }),
                    _ => false,
                };
                if in_range {
                    let addr = self.elem_addr(base, i, elem);
                    return Ok(Place::Mem(addr, 0, elem));
                }
                // 範囲外なら panic (rustc と同じく境界検査する)
                let ok = self.b.ins().icmp(IntCC::UnsignedLessThan, i, len);
                // (rustc の場所: Vec など Index の impl を通るものは `[`、配列・スライスは式の先頭)
                let via_impl = matches!(self.peek(&ix.expr).map(|t| self.strip_refs(t)), Some(Ty::Struct(_)));
                let sp = if via_impl { ix.bracket_token.span.open() } else { ix.span() };
                self.index_check(ok, len, i, sp);
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
            let r = self.expr(base, None)?;
            if let (Some(v), t) = r {
                self.own_temp(v, t);
            }
            match r {
                (Some(a), t) if t.is_agg() => Place::Mem(a, 0, t),
                (Some(p), Ty::Ref(_, i) | Ty::Box(i)) => Place::Mem(p, 0, *i),
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
                Place::Mem(a, o, Ty::Ref(_, i) | Ty::Box(i)) => {
                    let ptr = self.b.ins().load(PTR, mf(), a, o);
                    Place::Mem(ptr, 0, *i)
                }
                Place::Var(v, Ty::Ref(_, i) | Ty::Box(i)) => {
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
        // `&Vec<T>` → `&[T]`、`&String` → `&str`
        if let (Some(want @ Ty::Ref(m2, Ty::Slice(_) | Ty::Str)), Some(v), Ty::Ref(m1, Ty::Struct(sid))) =
            (expected, v, t)
        {
            if m1 || !m2 {
                if let Some((fat, ft)) = self.deref_to_slice(v, *sid, m2) {
                    let ok = match (want, ft) {
                        (Ty::Ref(_, Ty::Str), Ty::Ref(_, Ty::Str)) => true,
                        (Ty::Ref(_, Ty::Slice(we)), Ty::Ref(_, Ty::Slice(fe))) => fe == we,
                        _ => false,
                    };
                    if ok {
                        return Ok((Some(fat), want));
                    }
                }
            }
        }
        // `&mut T` → `&T` (`Box<T>` も `&mut T` なのでここで `&T` になる)
        if let (Some(want @ Ty::Ref(false, wi)), Ty::Ref(true, gi)) = (expected, t) {
            if wi == gi {
                return Ok((v, want));
            }
        }
        // `Box<T>` → `Box<dyn Trait>`
        if let (Some(want @ Ty::Box(Ty::Dyn(tr))), Some(v), Ty::Box(inner)) = (expected, v, t) {
            if !matches!(inner, Ty::Dyn(_)) {
                let fat = self.to_dyn(v, *inner, tr, e.span())?;
                return Ok((Some(fat), want));
            }
        }
        // `&T` → `&dyn Trait` (データのポインタ + vtable)
        if let (Some(want @ Ty::Ref(m2, Ty::Dyn(tr))), Some(v), Ty::Ref(m1, inner)) = (expected, v, t)
        {
            if !matches!(inner, Ty::Dyn(_)) && (m1 || !m2) {
                let fat = self.to_dyn(v, *inner, tr, e.span())?;
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
                self.lets.get(&(e as *const Expr as usize)).copied().or_else(|| {
                    if self.in_fmt_args > 0 {
                        self.lets.get(&crate::infer::pos_key(e)).copied()
                    } else {
                        None
                    }
                })
            }
            None => None,
        };
        match e {
            Expr::Lit(l) => self.lit(&l.lit, expected),
            Expr::Paren(p) => self.expr(&p.expr, expected),
            Expr::Group(g) => self.expr(&g.expr, expected),
            Expr::Block(b) => {
                if let Some(l) = &b.label {
                    return self.labeled_block(l.name.ident.to_string(), &b.block, expected);
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
            // const ジェネリクスの N: インスタンスの型引数の長さ
            Expr::Path(p) if self.const_generic(p).is_some() => {
                let n = self.const_generic(p).unwrap();
                Ok((Some(self.b.ins().iconst(types::I64, n as i64)), Ty::Usize))
            }
            // const は式そのものを埋め込む
            Expr::Path(p) if self.const_of(p).is_some() => {
                let ci = self.const_of(p).unwrap();
                self.const_value(p, ci)
            }
            Expr::Path(p) if self.fn_item(p).is_some() => {
                let id = self.fn_item(p).unwrap();
                self.fn_value(p, id, expected)
            }
            Expr::Path(_) | Expr::Field(_) => {
                let p = self.place(e)?;
                let r = self.read(p);
                // drop が要る値を値として読んだらムーブ
                if self.needs_drop(r.1) {
                    let mut root = e;
                    while let Expr::Field(f) = strip_paren(root) {
                        root = &f.base;
                    }
                    if let Expr::Path(rp) = strip_paren(root) {
                        if let Some(id) = rp.path.get_ident() {
                            if self.caps.iter().any(|c| c.0 == id.to_string()) {
                                return err(
                                    e.span(),
                                    "moving a captured value out of a closure is not supported",
                                );
                            }
                        }
                    }
                    if let (Expr::Field(_), Place::Mem(a, o, t), Some(v)) = (strip_paren(e), p, r.0) {
                        if self.zeroable(t) {
                            // フィールドだけのムーブ: 値を退避して元を「空」(0) にする。
                            // 持ち主はそのまま drop される (0 の Box は解放しない、0 の Option は None)
                            let v = if t.is_agg() {
                                let tmp = self.temp(t);
                                self.copy(tmp, v, t);
                                tmp
                            } else {
                                v
                            };
                            let dst = self.addr_plus(a, o);
                            self.zero(dst, t);
                            return Ok((Some(v), t));
                        }
                    }
                    self.mark_moved(e);
                }
                Ok(r)
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
                let refill = if self.needs_drop(t) {
                    self.drop_before_assign(&a.left, p)
                } else {
                    None
                };
                self.write(p, v);
                if let Some(f) = refill {
                    let one = self.b.ins().iconst(types::I8, 1);
                    self.b.ins().stack_store(PTR, one, f, 0);
                }
                Ok((None, Ty::Unit))
            }
            Expr::If(i) => self.if_(i, expected),
            Expr::While(w) if matches!(&*w.cond, Expr::Let(_)) => self.while_let(w),
            Expr::While(w) => {
                self.pending_label = w.label.as_ref().map(|l| l.name.ident.to_string());
                let header = self.b.create_block();
                let body = self.b.create_block();
                let exit = self.b.create_block();
                self.b.ins().jump(header, &[]);
                self.b.switch_to_block(header);
                let t0 = self.temps.len();
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
                    label: self.pending_label.take(),
                    val: None,
                    is_block: false,
                    depth: self.scopes.len(),
                    temps: t0,
                });
                let (_, bt) = self.block(&w.body, Some(Ty::Unit))?;
                self.loops.pop();
                if bt != Ty::Never {
                    self.same(w.body.span(), bt, Ty::Unit)?;
                    self.drop_for_exit(usize::MAX, t0);
                    self.b.ins().jump(header, &[]);
                } else {
                    self.b.ins().trap(TrapCode::unwrap_user(1));
                }
                self.b.switch_to_block(exit);
                self.drop_temps(t0);
                Ok((None, Ty::Unit))
            }
            Expr::Loop(l) => {
                self.pending_label = l.label.as_ref().map(|l| l.name.ident.to_string());
                let body = self.b.create_block();
                let exit = self.b.create_block();
                self.b.ins().jump(body, &[]);
                self.b.switch_to_block(body);
                self.loops.push(Loop {
                    cont: body,
                    brk: exit,
                    breaks: 0,
                    label: self.pending_label.take(),
                    val: None,
                    is_block: false,
                    depth: self.scopes.len(),
                    temps: self.temps.len(),
                });
                let (_, bt) = self.block(&l.body, Some(Ty::Unit))?;
                let lp = self.loops.pop().unwrap();
                if bt != Ty::Never {
                    self.same(l.body.span(), bt, Ty::Unit)?;
                    self.drop_for_exit(usize::MAX, lp.temps);
                    self.b.ins().jump(body, &[]);
                } else {
                    self.b.ins().trap(TrapCode::unwrap_user(1));
                }
                self.b.switch_to_block(exit);
                if lp.breaks == 0 {
                    // break のない loop は値を返さない (型は !)
                    return Ok((None, Ty::Never));
                }
                // `break v` の値は exit の引数
                if let Some(t) = lp.val {
                    let v = self.b.block_params(exit).first().copied();
                    return Ok((v, t));
                }
                Ok((None, Ty::Unit))
            }
            Expr::Break(br) => {
                let label = br.label.as_ref().map(|l| l.ident.to_string());
                let Some(li) = self.loop_index(label.as_deref(), true) else {
                    return err(br.span(), "`break` outside of a loop (or unknown label)");
                };
                // `break v`: 値を評価してから抜ける (型は最初の break で決まる)
                let mut args = vec![];
                if let Some(e) = &br.expr {
                    let want = self.loops[li].val;
                    let (v, t) = self.expr(e, want)?;
                    if t == Ty::Never {
                        return Ok((None, Ty::Never));
                    }
                    match want {
                        Some(w) => self.same(e.span(), t, w)?,
                        None => {
                            let brk = self.loops[li].brk;
                            let ct = if t.is_agg() || t.is_fat() { PTR } else { clif_ty(t).unwrap_or(types::I8) };
                            if t != Ty::Unit {
                                self.b.append_block_param(brk, ct);
                            }
                            self.loops[li].val = Some(t);
                        }
                    }
                    if let Some(v) = v {
                        args.push(v);
                    }
                }
                let lp = &mut self.loops[li];
                lp.breaks += 1;
                let (t, d, tt) = (lp.brk, lp.depth, lp.temps);
                self.drop_for_exit(d, tt);
                let args: Vec<_> = args.into_iter().map(|v| v.into()).collect();
                self.b.ins().jump(t, &args);
                Ok(self.dead())
            }
            Expr::Continue(c) => {
                let label = c.label.as_ref().map(|l| l.ident.to_string());
                let Some(li) = self.loop_index(label.as_deref(), false) else {
                    return err(c.span(), "`continue` outside of a loop (or unknown label)");
                };
                let lp = &self.loops[li];
                let (t, d, tt) = (lp.cont, lp.depth, lp.temps);
                self.drop_for_exit(d, tt);
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
                self.drop_for_exit(0, 0);
                self.emit_return(v);
                Ok(self.dead())
            }
            Expr::Call(c) => self.call(c, expected),
            Expr::Closure(c) => self.closure_value(c, expected),
            Expr::Range(rg) => self.range_value(rg, expected),
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
            Expr::MethodCall(mc) => self.method_call(mc, expected),
            Expr::Cast(c) => {
                let to = self.prog.parse_ty(&c.ty, &self.env)?;
                // `&x as &dyn Trait` は期待型つきの評価 (強制変換) と同じ
                if let Ty::Ref(_, Ty::Dyn(_)) = to {
                    let (v, t) = self.expr(&c.expr, Some(to))?;
                    self.same(c.span(), t, to)?;
                    return Ok((v, t));
                }
                let hint = self.peek(&c.expr).or(match &*c.expr {
                    Expr::Lit(syn::ExprLit { lit: Lit::Float(_), .. }) => Some(Ty::F64),
                    _ => Some(Ty::I32),
                });
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
                    // 関数ポインタのアドレス (`f as usize`)
                    (Ty::Fn(_), t) if matches!(t, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
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
                // 浮動小数点が関わるキャスト (Rust の `as` と同じく、浮動小数点 → 整数は飽和し NaN は 0)
                if from.is_float() || to.is_float() {
                    let v = v.unwrap();
                    let span = c.span();
                    // 128 bit 整数 ⇔ 浮動小数点は prelude の関数 (Cranelift にない)
                    if from.bits() == 128 && to.is_float() {
                        let sg = self.b.ins().iconst(types::I8, from.signed() as i64);
                        let single = self.b.ins().iconst(types::I8, (to == Ty::F32) as i64);
                        let (r, _) = self.call_named("__exr_u128_to_f", vec![Ty::Unit], &[(v, Ty::U128), (sg, Ty::Bool), (single, Ty::Bool)], span)?;
                        let r = r.unwrap();
                        let r = if to == Ty::F32 { self.b.ins().fdemote(types::F32, r) } else { r };
                        return Ok((Some(r), to));
                    }
                    if from.is_float() && to.bits() == 128 {
                        let w = if from == Ty::F32 { self.b.ins().fpromote(types::F64, v) } else { v };
                        let sg = self.b.ins().iconst(types::I8, to.signed() as i64);
                        let (r, _) = self.call_named("__exr_f_to_u128", vec![Ty::Unit], &[(w, Ty::F64), (sg, Ty::Bool)], span)?;
                        return Ok((r, to));
                    }
                    let r = match (from.is_float(), to.is_float()) {
                        (true, true) if from == to => v,
                        (true, true) if to == Ty::F64 => self.b.ins().fpromote(types::F64, v),
                        (true, true) => self.b.ins().fdemote(types::F32, v),
                        (false, true) if from.is_int() => {
                            let w = if from.bits() < 32 { self.convert(v, from, if from.signed() { Ty::I32 } else { Ty::U32 }) } else { v };
                            let ft = clif_ty(to).unwrap();
                            if from.signed() { self.b.ins().fcvt_from_sint(ft, w) } else { self.b.ins().fcvt_from_uint(ft, w) }
                        }
                        (true, false) if to.is_int() => {
                            // 64 bit に飽和させてから、狭い型は範囲に切り詰める
                            let wide = if to.signed() {
                                self.b.ins().fcvt_to_sint_sat(types::I64, v)
                            } else {
                                self.b.ins().fcvt_to_uint_sat(types::I64, v)
                            };
                            if to.bits() == 64 {
                                wide
                            } else {
                                let (lo, hi): (i64, i64) = match to {
                                    Ty::I8 => (i8::MIN as i64, i8::MAX as i64),
                                    Ty::I16 => (i16::MIN as i64, i16::MAX as i64),
                                    Ty::I32 => (i32::MIN as i64, i32::MAX as i64),
                                    Ty::U8 => (0, u8::MAX as i64),
                                    Ty::U16 => (0, u16::MAX as i64),
                                    _ => (0, u32::MAX as i64),
                                };
                                let c = if to.signed() {
                                    let a = self.b.ins().iconst(types::I64, lo);
                                    let m = self.b.ins().smax(wide, a);
                                    let b2 = self.b.ins().iconst(types::I64, hi);
                                    self.b.ins().smin(m, b2)
                                } else {
                                    let b2 = self.b.ins().iconst(types::I64, hi);
                                    self.b.ins().umin(wide, b2)
                                };
                                self.b.ins().ireduce(clif_ty(to).unwrap(), c)
                            }
                        }
                        _ => {
                            return err(c.span(), format!("unsupported cast `{}` as `{}`", self.prog.tn(from), self.prog.tn(to)));
                        }
                    };
                    return Ok((Some(r), to));
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
        let fn_item = matches!(&*r.expr, Expr::Path(p) if self.fn_item(p).is_some());
        // (`&None` などのバリアント・定数の名前は場所でなく一時値)
        let not_var = |e: &Expr| matches!(e, Expr::Path(p) if p.path.get_ident().is_some_and(|i| {
            let n = i.to_string();
            self.lookup(&n).is_none() && !self.prog.const_index.contains_key(&n)
        }));
        let inner = match &*r.expr {
            Expr::Paren(p) => &*p.expr,
            e => e,
        };
        if is_place_expr(&r.expr) && !fn_item && !not_var(inner) {
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
        // 定数式への共有参照は静的データに昇格する (`fn f() -> &'static S { &S { x: 1 } }`)
        if !m && is_promotable(self, &r.expr) {
            let t = inner_exp
                .filter(|t| !matches!(t, Ty::Dyn(_) | Ty::Slice(_) | Ty::Str))
                .or_else(|| self.peek(&r.expr));
            if let Some(t) = t.filter(|&t| !self.needs_drop(t) && !self.prog.has_param(t)) {
                if let Ok(dv) = crate::consteval::eval(self.prog, &r.expr, t) {
                    if dv.relocs.is_empty() && !dv.bytes.is_empty() {
                        let a = self.data_addr(dv.bytes);
                        return Ok((Some(a), Ty::Ref(false, intern(t))));
                    }
                }
            }
        }
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
        // `&一時値`: 一時値は文 (let ならブロック) の終わりまで所有する
        self.own_temp_at(a, t);
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
        // `S { a: 1, ..base }`: base を一時変数に置き、書かれていないフィールドをそこからムーブする
        // (書いたフィールドの元の値は、一時変数のスコープの終わりで drop される)
        if let Some(base) = &s.rest {
            let st = match expected {
                Some(Ty::Struct(x)) if self.prog.base_of(x) == self.prog.base_of(sid) => Some(Ty::Struct(x)),
                _ if self.prog.structs[sid as usize].generics.is_empty() => Some(Ty::Struct(sid)),
                _ => None,
            };
            self.scopes.push(vec![]);
            let (bv, bt) = self.expr(base, st)?;
            if bt == Ty::Never {
                self.scopes.pop();
                return Ok((None, Ty::Never));
            }
            self.bind("__exr_base".into(), bt, bv);
            let Ty::Struct(bsid) = bt else {
                return err(base.span(), "the base of `..` must be a struct");
            };
            let all: Vec<String> = self.prog.structs[bsid as usize].fields.iter().map(|f| f.0.clone()).collect();
            let mut ns = s.clone();
            ns.rest = None;
            ns.dot2_token = None;
            for f in all {
                let given = s.fields.iter().any(|fv| match &fv.member {
                    syn::Member::Named(id) => *id == f,
                    syn::Member::Unnamed(i) => i.index.to_string() == f,
                });
                if !given {
                    let fi = syn::Ident::new(&f, base.span());
                    ns.fields.push(syn::parse_quote!(#fi: __exr_base.#fi));
                }
            }
            let r = self.struct_lit(&ns, Some(bt))?;
            self.pop_scope();
            return Ok(r);
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

    /// `|..| ..`: キャプチャを struct に詰めた値。型引数 (キャプチャ・引数・戻り値の型) をここで決める
    fn closure_value(&mut self, c: &syn::ExprClosure, expected: Option<Ty>) -> R<V> {
        let prog = self.prog;
        let hint = self.closure_hint.take();
        let Some(&k) = self.closure_ix.get(&crate::closure::closure_key(c)) else {
            return err(c.span(), "internal: unknown closure");
        };
        let Some(&cid) = prog.closure_by.get(&(self.root, k)) else {
            return err(c.span(), "internal: unregistered closure");
        };
        let cd = &prog.closures[&cid];
        let sig0 = &prog.sigs[cid as usize];
        let ng = sig0.generics.len();
        let mut sol: Vec<Option<Ty>> = vec![None; ng];
        for (i, s) in sol.iter_mut().enumerate().take(cd.n_parent) {
            *s = self.env.params.get(i).map(|p| p.1);
        }
        // 期待型がこのクロージャの型そのもの (戻り値位置の `impl Fn`) なら型引数ごと使う
        let exp_args = match expected {
            Some(Ty::Struct(sid)) if prog.base_of(sid) == cd.sid => {
                Some(prog.structs[sid as usize].args.clone())
            }
            _ => None,
        };
        // 期待型が `dyn Fn(A) -> B` なら引数と戻り値の手がかりにする
        let hint = hint.or_else(|| {
            let s = match expected {
                Some(Ty::Dyn(s)) => s,
                Some(Ty::Ref(_, Ty::Dyn(s))) => *s,
                _ => return None,
            };
            self.dyn_fn_sig(s).ok().map(|(ps, r)| (ps, Some(r)))
        });
        let mut caps = vec![];
        for (i, n) in cd.captures.iter().enumerate() {
            let Some((loc, t)) = self.lookup(n) else {
                return err(c.span(), format!("cannot capture `{n}` here"));
            };
            let ct = if cd.by_move {
                t
            } else {
                Ty::Ptr(true, intern(t))
            };
            sol[cd.n_parent + i] = Some(ct);
            caps.push((loc, t));
        }
        let mut g = cd.n_parent + cd.captures.len();
        let mut pslots = vec![];
        for (i, p) in c.inputs.iter().enumerate() {
            if crate::closure::param(p).1.is_some() {
                continue;
            }
            pslots.push((i, g));
            // 呼び先の境界 → 親の型推論 (`let f = |x| x; f(1u8)` の使われ方) の順
            sol[g] = hint
                .as_ref()
                .and_then(|h| h.0.get(i).copied())
                .filter(|&t| !prog.has_param(t))
                .or_else(|| self.lets.get(&(p as *const Pat as usize)).copied());
            g += 1;
        }
        // 戻り値: 注釈 → 手がかり → 本体の型推論
        let ri = ng - 1;
        sol[ri] = cd
            .ret_ann
            .map(|t| prog.partial_subst(t, &sol))
            .or(hint.as_ref().and_then(|h| h.1).filter(|&t| !prog.has_param(t)))
            .or_else(|| self.lets.get(&(c as *const syn::ExprClosure as usize)).copied().filter(|&t| !prog.has_param(t)))
            .or(hint.as_ref().and_then(|h| h.1));
        if let Some(a) = exp_args {
            for (s, a) in sol.iter_mut().zip(a) {
                *s = Some(a);
            }
        }
        // 一部だけ分かっている戻り値型 (`Result<?, u8>`) は型推論の手がかりにする
        let partial_ret = sol[ri].filter(|&t| prog.has_param(t));
        if partial_ret.is_some() {
            sol[ri] = None;
        }
        // 決まらない引数・戻り値は本体の型推論で (`let f = |x| x + k;`)
        if sol.iter().any(|s| s.is_none()) {
            let f = crate::closure::synth(c);
            let sig = FnSig {
                params: sig0.params.iter().map(|&t| prog.partial_subst(t, &sol)).collect(),
                ret: Ty::Unit,
                ..sig0.clone()
            };
            let env = TyEnv {
                self_sid: self.env.self_sid,
                params: sig0
                    .generics
                    .iter()
                    .zip(&sol)
                    .filter_map(|(g, s)| Some((g.0.clone(), (*s)?)))
                    .collect(),
                self_ty: self.env.self_ty,
            };
            let extra: Vec<(String, Ty)> = cd
                .captures
                .iter()
                .zip(&caps)
                .map(|(n, c)| (n.clone(), c.1))
                .collect();
            let (_, r, ps) =
                crate::infer::infer_ctx(prog, &f, &sig, &env, &extra, sol[ri].or(partial_ret), Some((self.root, k + 1)));
            if sol[ri].is_none() {
                sol[ri] = r;
            }
            for &(i, g) in &pslots {
                if sol[g].is_none() {
                    sol[g] = ps.get(i + 1).copied().flatten();
                }
            }
            if let Some(&(i, _)) = pslots.iter().find(|(_, g)| sol[*g].is_none()) {
                return err(
                    c.inputs[i].span(),
                    "cannot infer the type of this closure parameter; add a type annotation",
                );
            }
        }
        let Some(args) = sol.into_iter().collect::<Option<Vec<Ty>>>() else {
            return err(c.span(), "cannot infer the return type of this closure; annotate it (`|..| -> T { .. }`)");
        };
        let sid = prog.instantiate(cd.sid, args.clone()).or_else(|e| err(c.span(), e))?;
        // 呼ばれなくても本体を検査・コンパイルする (未使用なら --gc-sections で消える)
        {
            let mut g = self.insts.lock().unwrap();
            let iid = g.request(prog, cid, args).or_else(|e| err(c.span(), e))?;
            drop(g);
            self.requested.push(iid);
        }
        let st = Ty::Struct(sid);
        let a = self.temp(st);
        let fields = prog.structs[sid as usize].fields.clone();
        for ((loc, t), (_, ft, off)) in caps.into_iter().zip(fields) {
            if cd.by_move {
                let p = match loc {
                    Loc::Var(v) => Place::Var(v, t),
                    Loc::Zst => Place::Zst(t),
                    l => Place::Mem(self.loc_addr(l).unwrap(), 0, t),
                };
                let (v, _) = self.read(p);
                self.write(Place::Mem(a, off as i32, ft), v);
                if self.needs_drop(t) {
                    self.mark_moved_loc(loc);
                }
            } else {
                let addr = match loc {
                    Loc::Zst => self.b.ins().iconst(PTR, 0),
                    Loc::Var(_) => {
                        return err(c.span(), "internal: captured variable is not in memory");
                    }
                    l => self.loc_addr(l).unwrap(),
                };
                self.b.ins().store(mf(), addr, a, off as i32);
            }
        }
        Ok((Some(a), st))
    }

    /// `&T` (アドレス v) を `&dyn tr` の fat pointer にする
    fn to_dyn(&mut self, v: Value, t: Ty, tr: &str, span: proc_macro2::Span) -> R<Value> {
        let mut fns = vec![];
        if let (Some((_, bps, _)), Ty::Fn(f)) = (crate::front::parse_fn_bound(tr), t) {
            // 関数ポインタ: データ (関数ポインタを置いた場所) から呼ぶ prelude の中継関数
            if bps.len() != f.params.len() {
                return err(span, format!("expected a function with {} arguments", bps.len()));
            }
            let Some(&shim) = self.prog.index.get(&format!("__exr_fnptr{}", f.params.len())) else {
                return err(span, "fn pointers with more than 3 arguments cannot be used as `dyn Fn`");
            };
            let args: Vec<Ty> = f.params.iter().copied().chain([f.ret]).collect();
            let mut g = self.insts.lock().unwrap();
            let iid = g.request(self.prog, shim, args).or_else(|e| err(span, e))?;
            drop(g);
            self.requested.push(iid);
            fns.push(iid);
        } else if crate::front::parse_fn_bound(tr).is_some() {
            let cid = match t {
                Ty::Struct(sid) => self.prog.closure_of_struct.get(&self.prog.base_of(sid)).copied(),
                _ => None,
            };
            let Some(cid) = cid else {
                return err(span, format!("`{}` is not a closure", self.prog.tn(t)));
            };
            let Ty::Struct(sid) = t else { unreachable!() };
            let args = self.prog.structs[sid as usize].args.clone();
            let mut g = self.insts.lock().unwrap();
            let iid = g.request(self.prog, cid, args).or_else(|e| err(span, e))?;
            drop(g);
            self.requested.push(iid);
            fns.push(iid);
        } else {
            let Some(decls) = self.prog.traits.get(tr) else {
                return err(span, format!("cannot find trait `{tr}`"));
            };
            if !self.prog.implements(t, tr) {
                return err(
                    span,
                    format!("the trait `{tr}` is not implemented for `{}`", self.prog.tn(t)),
                );
            }
            for &d in decls {
                let ds = &self.prog.sigs[d as usize];
                let Some(Ty::Ref(m, _)) = ds.params.first() else {
                    return err(
                        span,
                        format!(
                            "`{}` cannot be called through `dyn` (it needs `&self`)",
                            self.prog.names[d as usize]
                        ),
                    );
                };
                let recv = [Ty::Ref(*m, intern(t))];
                let sol = vec![None; ds.generics.len()];
                let (mut fid, _) = self.resolve_call(d, &recv, sol, None, span)?;
                // ジェネリックな impl (`impl<T> Tr for W<T>`) のメソッドはさらにインスタンス化する
                if (fid as usize) < self.prog.sigs.len() && self.prog.sigs[fid as usize].is_generic() {
                    let g = self.prog.sigs[fid as usize].generics.len();
                    let mut sol = vec![None; g];
                    if let Some(st) = self.prog.sigs[fid as usize].self_ty {
                        self.prog.match_ty(st, t, &mut sol);
                    }
                    fid = self.resolve_call(fid, &recv, sol, None, span)?.0;
                }
                fns.push(fid);
            }
        }
        // 最後の欄は drop glue (`Box<dyn ..>` の drop で使う)
        let de = self.vtable_drop_entry(t);
        fns.push(de);
        let k = match self.vtables.iter().position(|x| *x == fns) {
            Some(k) => k,
            None => {
                self.vtables.push(fns);
                self.vtables.len() - 1
            }
        };
        let vt = self.symbol_addr(NS_VTABLE, k as u32);
        let fat = self.temp(Ty::Ref(false, intern(Ty::Dyn(crate::front::intern_str(tr)))));
        self.b.ins().store(mf(), v, fat, 0);
        self.b.ins().store(mf(), vt, fat, 8);
        Ok(fat)
    }

    /// vtable の idx 番目を呼ぶ。fat は `&dyn` の (データ, vtable) のアドレス
    fn dyn_call(
        &mut self,
        fat: Value,
        idx: usize,
        params: &[Ty],
        ret: Ty,
        args: &[&Expr],
        span: proc_macro2::Span,
    ) -> R<V> {
        self.indirect_call(Indirect::Vtable(fat, idx), params, ret, args, span)
    }

    /// 関数ポインタ / vtable 経由の呼び出し
    fn indirect_call(
        &mut self,
        callee: Indirect,
        params: &[Ty],
        ret: Ty,
        args: &[&Expr],
        span: proc_macro2::Span,
    ) -> R<V> {
        if params.len() != args.len() {
            return err(
                span,
                format!("expected {} arguments, found {}", params.len(), args.len()),
            );
        }
        let mut sig = self.prog.sigs[0].clone();
        let env = matches!(callee, Indirect::Vtable(..));
        sig.params = env
            .then_some(Ty::Ptr(false, intern(Ty::U8)))
            .into_iter()
            .chain(params.iter().copied())
            .collect();
        sig.ret = ret;
        sig.is_main = false;
        sig.is_extern = false;
        let data = match callee {
            Indirect::Vtable(fat, _) => Some(self.b.ins().load(PTR, mf(), fat, 0)),
            Indirect::Ptr(_) => None,
        };
        let mut vals = vec![];
        for (a, &pt) in args.iter().zip(params) {
            let (v, t) = self.expr(a, Some(pt))?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            self.same(a.span(), t, pt)?;
            vals.push(v);
        }
        let fp = match callee {
            Indirect::Vtable(fat, idx) => {
                let vt = self.b.ins().load(PTR, mf(), fat, 8);
                self.b.ins().load(PTR, mf(), vt, 8 * idx as i32)
            }
            Indirect::Ptr(fp) => fp,
        };
        let mut cv = vec![];
        let sret = if is_sret(&sig) {
            let a = self.temp(ret);
            cv.push(a);
            Some(a)
        } else {
            None
        };
        cv.extend(data);
        for (v, &pt) in vals.into_iter().zip(params) {
            if let Some(v) = v {
                self.push_arg(&mut cv, v, pt);
            }
        }
        let sr = self.b.import_signature(clif_sig(&sig, self.cc));
        let call = self.b.ins().call_indirect(sr, fp, &cv);
        if ret == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(self.dead());
        }
        if let Some(a) = sret {
            return Ok((Some(a), ret));
        }
        let r = self.b.inst_results(call).first().copied();
        Ok((r, ret))
    }

    /// `&dyn Fn(A) -> B` の呼び出し (vtable の 0 番目がクロージャの本体)
    fn dyn_fn_call(&mut self, fat: Value, s: &str, args: &[&Expr], span: proc_macro2::Span) -> R<V> {
        let (ps, r) = self.dyn_fn_sig(s).or_else(|e| err(span, e))?;
        self.dyn_call(fat, 0, &ps, r, args, span)
    }

    /// `x.m(..)` (x: `&dyn Trait`)
    fn dyn_method_call(&mut self, fat: Value, tr: &str, mc: &syn::ExprMethodCall) -> R<V> {
        let name = mc.method.to_string();
        let decls = self.prog.traits.get(tr).cloned().unwrap_or_default();
        let Some(idx) = decls
            .iter()
            .position(|&d| self.prog.names[d as usize].rsplit("::").next() == Some(&name))
        else {
            return err(
                mc.method.span(),
                format!("no method named `{name}` found for `dyn {tr}`"),
            );
        };
        let ds = &self.prog.sigs[decls[idx] as usize];
        // Self は引数・戻り値に出てこない (object safe) 前提。出てきたらエラー
        let ps: Vec<Ty> = ds.params[1..].to_vec();
        if ps.iter().chain([&ds.ret]).any(|&t| self.prog.has_param(t)) {
            return err(
                mc.method.span(),
                format!("`{name}` cannot be called through `dyn {tr}` (it uses `Self` or generics)"),
            );
        }
        let ret = ds.ret;
        let args: Vec<&Expr> = mc.args.iter().collect();
        self.dyn_call(fat, idx, &ps, ret, &args, mc.span())
    }

    /// 呼び先の引数の型 pt が `F` / `&F` / `&mut F` で、F に `Fn(A) -> B` の境界があれば (A, B)
    /// (呼び先のシグネチャの型パラメータを Param で含む)
    fn fn_bound_of(&self, sig: &FnSig, pt: Ty) -> Option<(Vec<Ty>, Ty)> {
        let pt = match pt {
            Ty::Ref(_, i) => *i,
            t => t,
        };
        let Ty::Param(g) = pt else { return None };
        let (_, bounds) = sig.generics.get(g as usize)?;
        let (_, ps, r) = bounds.iter().find_map(|b| crate::front::parse_fn_bound(b))?;
        let names: Vec<&str> = sig.generics.iter().map(|g| g.0.as_str()).collect();
        let ps = ps
            .iter()
            .map(|t| self.prog.resolve_g(t, &names).ok())
            .collect::<Option<Vec<Ty>>>()?;
        Some((ps, self.prog.resolve_g(&r, &names).ok()?))
    }

    /// クロージャの型 (struct のインスタンス) なら、その引数と戻り値の型
    fn closure_sig(&self, t: Ty) -> Option<(Vec<Ty>, Ty)> {
        if let Ty::Fn(f) = t {
            return Some((f.params.clone(), f.ret));
        }
        let Ty::Struct(sid) = t else { return None };
        let cid = *self.prog.closure_of_struct.get(&self.prog.base_of(sid))?;
        let args = &self.prog.structs[sid as usize].args;
        let s0 = &self.prog.sigs[cid as usize];
        Some((
            s0.params[1..].iter().map(|&p| self.prog.subst(p, args)).collect(),
            self.prog.subst(s0.ret, args),
        ))
    }

    /// 関連型の制約から型パラメータを解く (`I: Iterator<Item = T>` / `where Self: Iterator<Item = &T>`)。
    /// `collect::<Vec<_>>()` の `_` は `Self::Item` で埋める
    fn solve_assoc_bounds(&self, sig: &FnSig, sol: &mut [Option<Ty>]) {
        let names: Vec<&str> = sig.generics.iter().map(|g| g.0.as_str()).collect();
        for _ in 0..2 {
            for (g, (_, bounds)) in sig.generics.iter().enumerate() {
                let Some(gt) = sol[g] else { continue };
                if self.prog.has_param(gt) {
                    continue;
                }
                for b in bounds {
                    let Some((name, want)) = assoc_constraint(b) else { continue };
                    let Ok(want) = self.prog.resolve_g(&want, &names) else { continue };
                    if let Some(actual) = self.prog.assoc_of(gt, &name) {
                        self.prog.match_ty(want, actual, sol);
                    }
                }
            }
        }
        for g in 0..sol.len() {
            let Some(t) = sol[g] else { continue };
            if !has_hole(self.prog, t) || !sig.generics[g].1.iter().any(|b| b == "__FromIter") {
                continue;
            }
            let item = sol
                .first()
                .copied()
                .flatten()
                .filter(|&s| !self.prog.has_param(s))
                .and_then(|s| self.prog.assoc_of(s, "Item"));
            if let Some(item) = item {
                let mut args: Vec<Ty> = (0..=crate::front::HOLE).map(Ty::Param).collect();
                args[crate::front::HOLE as usize] = item;
                sol[g] = Some(self.prog.subst(t, &args));
            }
        }
    }

    /// 値としての `a..b` / `a..=b`: prelude の Range / RangeInclusive
    fn range_value(&mut self, rg: &syn::ExprRange, expected: Option<Ty>) -> R<V> {
        let incl = matches!(rg.limits, syn::RangeLimits::Closed(_));
        // `(1..)`: 終わりのない範囲
        if let (Some(lo), None, false) = (&rg.start, &rg.end, incl) {
            let Some(&base) = self.prog.struct_index.get("RangeFrom") else {
                return err(rg.span(), "ranges can only be used directly in `for` loops here");
            };
            let et = match expected {
                Some(Ty::Struct(sid)) if self.prog.base_of(sid) == base => self.prog.structs[sid as usize].args.first().copied(),
                _ => None,
            }
            .or_else(|| self.peek(lo));
            let (lv, lt) = self.expr(lo, et)?;
            let sid = self.prog.instantiate(base, vec![lt]).or_else(|e| err(rg.span(), e))?;
            let st = Ty::Struct(sid);
            let a = self.temp(st);
            let (_, ft, off) = self.prog.structs[sid as usize].fields[0];
            self.write(Place::Mem(a, off as i32, ft), lv);
            return Ok((Some(a), st));
        }
        let name = if incl { "RangeInclusive" } else { "Range" };
        let (Some(lo), Some(hi)) = (&rg.start, &rg.end) else {
            return err(rg.span(), "only `a..b` / `a..=b` / `a..` ranges can be used as values");
        };
        let Some(&base) = self.prog.struct_index.get(name) else {
            return err(rg.span(), "ranges can only be used directly in `for` loops here");
        };
        let mut et = match expected {
            Some(Ty::Struct(sid)) if self.prog.base_of(sid) == base => {
                self.prog.structs[sid as usize].args.first().copied()
            }
            _ => None,
        };
        if et.is_none() {
            et = self.peek(lo).or_else(|| self.peek(hi));
        }
        let (lv, lt) = self.expr(lo, et)?;
        let (hv, ht) = self.expr(hi, Some(lt))?;
        self.same(hi.span(), ht, lt)?;
        let sid = self.prog.instantiate(base, vec![lt]).or_else(|e| err(rg.span(), e))?;
        let st = Ty::Struct(sid);
        let a = self.temp(st);
        let fields = self.prog.structs[sid as usize].fields.clone();
        for (n, ft, off) in fields {
            let v = match n.as_str() {
                "start" => lv,
                "end" => hv,
                _ => Some(self.b.ins().iconst(types::I8, 0)),
            };
            self.write(Place::Mem(a, off as i32, ft), v);
        }
        Ok((Some(a), st))
    }

    /// `Fn(A, B) -> R` (dyn の中身) の引数と戻り値の型
    fn dyn_fn_sig(&self, s: &str) -> R<(Vec<Ty>, Ty)> {
        let Some((_, ps, r)) = crate::front::parse_fn_bound(s) else {
            return Err(format!("`dyn {s}` is not a function type"));
        };
        let ps = ps
            .iter()
            .map(|t| self.prog.resolve_env(t, &self.env))
            .collect::<R<Vec<Ty>>>()?;
        Ok((ps, self.prog.resolve_env(&r, &self.env)?))
    }

    /// 関数でない値の呼び出し `f(x)`: クロージャ (の参照) か `&dyn Fn`
    fn call_value(&mut self, c: &syn::ExprCall, expected: Option<Ty>) -> R<V> {
        // 呼び出しはムーブしない (Fn / FnMut は参照で呼ぶ)。一時値なら文の終わりまで所有する
        let (v, mut t) = if is_place_expr(&c.func)
            && !matches!(&*c.func, Expr::Path(p) if self.fn_item(p).is_some())
        {
            let p = self.place(&c.func)?;
            self.read(p)
        } else {
            let r = self.expr(&c.func, None)?;
            if let (Some(v), t) = r {
                self.own_temp(v, t);
            }
            r
        };
        let Some(mut v) = v else {
            return err(c.func.span(), "expected a function");
        };
        loop {
            match t {
                Ty::Ref(_, Ty::Dyn(_)) | Ty::Box(Ty::Dyn(_)) => break,
                Ty::Ref(_, i) | Ty::Box(i) if i.is_agg() => t = *i,
                Ty::Ref(_, i) | Ty::Box(i) => {
                    v = self.b.ins().load(PTR, mf(), v, 0);
                    t = *i;
                }
                _ => break,
            }
        }
        let args: Vec<&Expr> = c.args.iter().collect();
        match t {
            Ty::Struct(sid) => {
                let Some(&cid) = self.prog.closure_of_struct.get(&self.prog.base_of(sid)) else {
                    return err(
                        c.func.span(),
                        format!("expected a function, found `{}`", self.prog.tn(t)),
                    );
                };
                let recv = (v, Ty::Ptr(true, intern(t)));
                self.call_fn(cid, Some(recv), &args, None, expected, c.span())
            }
            Ty::Ref(_, Ty::Dyn(s)) | Ty::Box(Ty::Dyn(s)) => self.dyn_fn_call(v, s, &args, c.span()),
            Ty::Fn(f) => self.indirect_call(Indirect::Ptr(v), &f.params, f.ret, &args, c.span()),
            t => err(
                c.func.span(),
                format!("expected a function, found `{}`", self.prog.tn(t)),
            ),
        }
    }

    fn call(&mut self, c: &syn::ExprCall, expected: Option<Ty>) -> R<V> {
        if let Some(r) = self.default_call(c, expected)? {
            return Ok(r);
        }
        if let Some(e) = self.prog.prim_method_call(c, self.self_sid) {
            return self.expr(&e, expected);
        }
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
            // 値の呼び出し: クロージャ / `&dyn Fn`
            return self.call_value(c, expected);
        };
        let args: Vec<&Expr> = c.args.iter().collect();
        let tf = match &*c.func {
            Expr::Path(p) => match &p.path.segments.last().unwrap().arguments {
                syn::PathArguments::AngleBracketed(a) => Some(a),
                _ => None,
            },
            _ => None,
        };
        // `Buf::<8>::new()` / `Vec::<i32>::new()`: 型のセグメントの型引数
        if let Expr::Path(p) = &*c.func {
            let n = p.path.segments.len();
            if n >= 2 && !p.path.segments[n - 2].arguments.is_none() {
                let seg = p.path.segments[n - 2].clone();
                let ty = syn::Type::Path(syn::TypePath { qself: None, path: syn::Path::from(seg) });
                self.seg_self = Some(self.prog.parse_ty(&ty, &self.env)?);
            }
        }
        let r = self.call_fn(id, None, &args, tf, expected, c.span());
        self.seg_self = None;
        r
    }

    /// `recv.m(args)`: レシーバを自動 deref して struct を見つけ、
    /// メソッドの self 型に合わせて自動参照 (`&self` / `&mut self`) かコピー (`self`) で渡す
    fn method_call(&mut self, mc: &syn::ExprMethodCall, expected: Option<Ty>) -> R<V> {
        // `x.unwrap()` / `x.expect(..)`: panic の場所 (メソッド名の位置) を先に書いておく (#[track_caller] 相当)
        if matches!(mc.method.to_string().as_str(), "unwrap" | "expect" | "unwrap_err" | "expect_err")
            && self.prog.panic_handler.is_none()
        {
            if let Some(&ci) = self.prog.const_index.get("__EXR_CALLER") {
                let (l, c) = self.loc(mc.method.span());
                let v = self.b.ins().iconst(types::I64, ((l as i64) << 16) | c as i64);
                let a = self.symbol_addr(NS_STATIC, ci);
                self.b.ins().store(mf(), v, a, 0);
            }
        }
        // `v[a..b].iter()` / `s[1..].len()`: 範囲の添字の受け手は自動で借用する (書き換えるメソッドは &mut)
        if let Expr::Index(ix) = strip_paren(&mc.receiver) {
            if matches!(&*ix.index, Expr::Range(_))
                && !matches!(
                    mc.method.to_string().as_str(),
                    "sort" | "sort_unstable" | "sort_by" | "sort_unstable_by" | "sort_by_key" | "sort_unstable_by_key"
                        | "sort_by_cached_key"
                )
            {
                let mutating = matches!(
                    mc.method.to_string().as_str(),
                    "reverse" | "swap" | "fill" | "rotate_left" | "rotate_right" | "iter_mut" | "copy_from_slice"
                        | "clone_from_slice" | "split_at_mut"
                );
                let (r, m, args) = (&mc.receiver, &mc.method, &mc.args);
                let tf = &mc.turbofish;
                let e: Expr = if mutating {
                    syn::parse_quote!((&mut #r).#m #tf (#args))
                } else {
                    syn::parse_quote!((&#r).#m #tf (#args))
                };
                return self.expr(&e, expected);
            }
        }
        // 値としての `[a, b].into_iter()`: Vec にしてから (配列の変数で要素が Copy なら iter().copied())
        if mc.method == "into_iter" && mc.args.is_empty() {
            if let Expr::Array(a) = strip_paren(&mc.receiver) {
                let es = &a.elems;
                let e: Expr = syn::parse_quote!(vec![#es].into_iter());
                return self.expr(&e, expected);
            }
            if let Some(Ty::Array(el, _)) = self.peek(&mc.receiver) {
                if !self.needs_drop(*el) && self.prog.struct_index.contains_key("Copied") {
                    let r = &mc.receiver;
                    let e: Expr = syn::parse_quote!((#r).iter().copied());
                    return self.expr(&e, expected);
                }
            }
        }
        // ブランケット impl のメソッドをプリミティブ型・&str で呼ぶ (`5.describe()` / `"x".describe()`)
        if let Some(&bid) = self.prog.blanket.get(&mc.method.to_string()) {
            let pt = self.peek(&mc.receiver);
            let prim = |t: Ty| t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char);
            let own = match pt.map(|t| self.strip_refs(t)) {
                Some(Ty::Struct(sid)) => self.prog.method(sid, &mc.method.to_string()).is_some(),
                _ => false,
            };
            if !own && pt.is_some_and(|t| prim(t) || matches!(t, Ty::Ref(_, Ty::Str))) {
                let (v, t) = self.expr(&mc.receiver, None)?;
                let Some(v) = v else { return Ok((None, Ty::Never)) };
                // (&self に渡すのは値のアドレス。&str は fat pointer の置き場所のアドレスがそのまま &&str)
                let addr = if t.is_fat() {
                    v
                } else {
                    let ss = self.slot(t);
                    self.b.ins().stack_store(PTR, v, ss, 0);
                    self.b.ins().stack_addr(PTR, ss, 0)
                };
                let args: Vec<&Expr> = mc.args.iter().collect();
                return self.call_fn(bid, Some((addr, Ty::Ref(false, intern(t)))), &args, mc.turbofish.as_ref(), expected, mc.span());
            }
        }
        // `b.as_ref()` / `b.as_mut()` (b: Box<T> / &Box<T>): 中身への参照 (Box と参照は同じ表現)
        if matches!(mc.method.to_string().as_str(), "as_ref" | "as_mut") && mc.args.is_empty() {
            let mut t = self.peek(&mc.receiver);
            while let Some(Ty::Ref(_, i)) = t {
                t = Some(*i);
            }
            if let Some(Ty::Box(inner)) = t {
                let (v, mut vt) = self.read_no_move(&mc.receiver)?;
                let Some(mut v) = v else { return Ok((None, Ty::Never)) };
                while let Ty::Ref(_, i) = vt {
                    v = if i.is_fat() { v } else { self.b.ins().load(PTR, mf(), v, 0) };
                    vt = *i;
                }
                return Ok((Some(v), Ty::Ref(mc.method == "as_mut", inner)));
            }
        }
        // prelude の `v.__push_auto(x)` (partition など): x が `&T` で v が `Vec<T>` なら `*x` を push
        if mc.method == "__push_auto" && mc.args.len() == 1 {
            let (r, a) = (&mc.receiver, &mc.args[0]);
            let elem = match self.peek(r).map(|t| self.strip_refs(t)) {
                Some(Ty::Struct(sid)) => self.vec_fields(sid).map(|f| f.2),
                _ => None,
            };
            let e: Expr = match (self.peek(a), elem) {
                (Some(Ty::Ref(_, i)), Some(el)) if *i == el => syn::parse_quote!((#r).push(*(#a))),
                _ => syn::parse_quote!((#r).push(#a)),
            };
            return self.expr(&e, expected);
        }
        // prelude の `s.__push_piece(c)` (String への collect): 要素の型で push / push_str
        if mc.method == "__push_piece" && mc.args.len() == 1 {
            let (r, a) = (&mc.receiver, &mc.args[0]);
            let e: Expr = match self.peek(a).map(|t| self.strip_refs(t)) {
                Some(Ty::Char) if matches!(self.peek(a), Some(Ty::Ref(..))) => syn::parse_quote!((#r).push(*(#a))),
                Some(Ty::Char) | None => syn::parse_quote!((#r).push(#a)),
                Some(Ty::Str) => syn::parse_quote!((#r).push_str(#a)),
                _ => syn::parse_quote!((#r).push_str((#a).as_str())),
            };
            return self.expr(&e, expected);
        }
        // `x.into()`: 期待型の `T::from(x)` (String は String::from、整数は広げるキャスト)
        if mc.method == "into" && mc.args.is_empty() {
            let recv_own = match self.peek(&mc.receiver) {
                Some(Ty::Struct(sid)) => self.prog.method(sid, "into").is_some(),
                _ => false,
            };
            let target = expected.or_else(|| self.lets.get(&(mc as *const _ as usize)).copied());
            if !recv_own {
                match target {
                    // `"msg".into()` / `e.into()` → Box<dyn Error>
                    Some(Ty::Struct(sid)) if self.prog.structs[sid as usize].name == "__DynError" => {
                        let (v, t) = self.expr(&mc.receiver, None)?;
                        let Some(v) = v else { return Ok((None, Ty::Never)) };
                        return self.call_named("__exr_dyn_error", vec![t], &[(v, t)], mc.span());
                    }
                    Some(Ty::Struct(sid)) => {
                        if let Some(id) = self.prog.method(sid, "from") {
                            let args = [&*mc.receiver];
                            return self.call_fn(id, None, &args, None, Some(Ty::Struct(sid)), mc.span());
                        }
                    }
                    Some(t) if t.is_int() || t.is_float() => {
                        let r = &mc.receiver;
                        let ty = syn::Ident::new(&self.prog.tn(t), mc.span());
                        let e: Expr = syn::parse_quote!((#r) as #ty);
                        return self.expr(&e, Some(t));
                    }
                    _ => {}
                }
            }
        }
        // `x.to_string()` (Display を実装した型。&str は str_method が String::from にする)
        if mc.method == "to_string" && mc.args.is_empty() && self.prog.index.contains_key("__exr_display") {
            if let Some(t) = self.peek(&mc.receiver) {
                let mut b = t;
                while let Ty::Ref(_, i) = b {
                    b = *i;
                }
                let own = match b {
                    Ty::Struct(sid) => self.prog.method(sid, "to_string").is_some(),
                    _ => false,
                };
                if !own && b != Ty::Str && self.fmt_ok(t, false) {
                    return self.to_string_value(&mc.receiver, mc.span());
                }
            }
        }
        if let Some(r) = self.ord_method(mc)? {
            return Ok(r);
        }
        if let Some(r) = self.extend_call(mc, expected)? {
            return Ok(r);
        }
        if let Some(r) = self.clone_call(mc)? {
            return Ok(r);
        }
        // `x.clamp(lo, hi)` (整数も Ord な型も): prelude の __exr_clamp
        if mc.method == "clamp" && mc.args.len() == 2 && self.prog.index.contains_key("__exr_clamp") {
            let own = match self.peek(&mc.receiver).map(|t| self.strip_refs(t)) {
                Some(Ty::Struct(sid)) => self.prog.method(sid, "clamp").is_some(),
                Some(_) => false,
                // (型が分からない: 自分の型に clamp があるときだけ普通のメソッド呼び出しにする)
                None => self.prog.names.iter().any(|n| n.ends_with("::clamp")),
            };
            if !own {
                let (r, a, b) = (&mc.receiver, &mc.args[0], &mc.args[1]);
                let e: Expr = syn::parse_quote!(__exr_clamp(#r, #a, #b));
                return self.expr(&e, expected);
            }
        }
        // `a.max(b)` / `a.min(b)` (Ord を実装した struct / タプル / String): prelude の __exr_max / __exr_min
        if matches!(mc.method.to_string().as_str(), "max" | "min")
            && mc.args.len() == 1
            && self.prog.index.contains_key("__exr_max")
        {
            if let Some(Ty::Struct(sid)) = self.peek(&mc.receiver) {
                if self.prog.method(sid, &mc.method.to_string()).is_none()
                    && self.prog.default_method(sid, &mc.method.to_string()).is_none()
                {
                    let f = syn::Ident::new(
                        if mc.method == "max" { "__exr_max" } else { "__exr_min" },
                        mc.method.span(),
                    );
                    let (r, a) = (&mc.receiver, &mc.args[0]);
                    let e: Expr = syn::parse_quote!(#f(#r, #a));
                    return self.expr(&e, expected);
                }
            }
        }
        if let Some(r) = self.builtin_method(mc)? {
            return Ok(r);
        }
        if let Some(r) = self.prim_trait_method(mc)? {
            return Ok(r);
        }
        let mut recv_temp = None;
        let mut p = if is_place_expr(&mc.receiver) {
            self.place(&mc.receiver)?
        } else {
            let r = self.expr(&mc.receiver, None)?;
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
                    return err(
                        mc.method.span(),
                        format!("no method `{}` on type `{}`", mc.method, self.prog.tn(t)),
                    );
                }
            }
        };
        let name = mc.method.to_string();
        // (メソッドがなく `impl Deref` があれば、deref した先で探す: `rc.borrow_mut()`)
        let (a, o, sid, id) = loop {
            let (a, o, sid) = loop {
                p = match p {
                    Place::Mem(a, o, Ty::Struct(sid)) => break (a, o, sid),
                    Place::Mem(a, o, Ty::Ref(_, Ty::Dyn(tr)) | Ty::Box(Ty::Dyn(tr))) => {
                        let fat = self.addr_plus(a, o);
                        return self.dyn_method_call(fat, tr, mc);
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
                        return err(
                            mc.method.span(),
                            format!("no method `{}` on type `{t}`", mc.method),
                        );
                    }
                }
            };
            if let Some(id) = self
                .prog
                .method(sid, &name)
                .or_else(|| self.prog.default_method(sid, &name))
            {
                break (a, o, sid, id);
            }
            let sd = &self.prog.structs[sid as usize];
            let gens = sd.args.clone();
            let base = sd.name.split('<').next().unwrap_or("").to_string();
            if let Some(did) = self.prog.method(sid, "deref") {
                let target = match self.prog.sigs[did as usize].ret {
                    Ty::Ref(_, t) => *t,
                    _ => return err(mc.span(), "bad `deref` signature"),
                };
                let target = self.prog.partial_subst(target, &gens.iter().map(|&g| Some(g)).collect::<Vec<_>>());
                let recv = self.addr_plus(a, o);
                let _ = base;
                let (v, vt) = self.call_fn_vals(did, &[(recv, Ty::Ref(false, intern(Ty::Struct(sid))))], mc.span())?;
                let inner_t = match vt {
                    Ty::Ref(_, t) => *t,
                    _ => target,
                };
                p = Place::Mem(v.unwrap(), 0, inner_t);
                continue;
            }
            let sname = self.prog.structs[sid as usize].name.clone();
            return err(
                mc.method.span(),
                format!("no method named `{name}` found for `{sname}`"),
            );
        };
        let sname = self.prog.structs[sid as usize].name.clone();
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
        // `self` を値で受ける: 受け手からムーブ
        if !matches!(recv_ty, Ty::Ref(..)) && self.needs_drop(st) {
            match recv_temp {
                Some(f) => self.clear_flag(f),
                None => self.mark_moved(&mc.receiver),
            }
        }
        let args: Vec<&Expr> = mc.args.iter().collect();
        self.call_fn(
            id,
            Some((recv, recv_ty)),
            &args,
            mc.turbofish.as_ref(),
            expected,
            mc.span(),
        )
    }

    /// プリミティブ型への trait 実装のメソッド (`impl Tr for u32` の `x.m()`)
    fn prim_trait_method(&mut self, mc: &syn::ExprMethodCall) -> R<Option<V>> {
        // (接尾辞のない整数リテラル `5.d()` は i32)
        let lit_int = matches!(strip_paren(&mc.receiver), Expr::Lit(syn::ExprLit { lit: Lit::Int(i), .. }) if i.suffix().is_empty());
        let Some(mut t) = self.peek(&mc.receiver).or(lit_int.then_some(Ty::I32)) else {
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
        let (v, vt) = self.expr(&mc.receiver, lit_int.then_some(t))?;
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
        if let (Some(t), Some(st)) = (self.seg_self, sig.self_ty) {
            self.prog.match_ty(st, t, &mut sol);
        }
        // trait メソッドは先頭が Self なので、turbofish はその後ろの型パラメータに当たる
        let first = sig.trait_of.is_some() as usize;
        if let Some(tf) = turbofish {
            for (i, a) in tf.args.iter().enumerate() {
                let ty = match a {
                    syn::GenericArgument::Type(t) => self.prog.parse_ty(t, &self.env)?,
                    // `f::<3>()`: const ジェネリクスの引数
                    syn::GenericArgument::Const(e) => {
                        let ts = crate::front::const_arg_syn(e).or_else(|er| err(a.span(), er))?;
                        self.prog.resolve_env(&ts, &self.env).or_else(|er| err(a.span(), er))?
                    }
                    _ => return err(a.span(), "only type arguments are supported"),
                };
                if let Some(slot) = sol.get_mut(first + i) {
                    // `_` は推論に任せる。`Vec<_>` のように一部だけ `_` なら穴として持つ
                    *slot = (ty != Ty::Param(crate::front::HOLE)).then_some(ty);
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
        self.solve_assoc_bounds(&sig, &mut sol);
        // prelude の helper のダミーの型引数 `G` (呼ばれた分だけインスタンスにするためのもの) は ()
        for (i, g) in sig.generics.iter().enumerate() {
            if g.0 == "G" && sol[i].is_none() {
                sol[i] = Some(Ty::Unit);
            }
        }
        if let Some(i) = sol.iter().position(|s| s.is_some_and(|t| has_hole(self.prog, t))) {
            return err(
                span,
                format!("cannot infer type parameter `{}`; annotate the type", sig.generics[i].0),
            );
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
        // 期待型が戻り値と同じ型の (具体的な) インスタンスなら、そこから型パラメータを解いておく
        // (`let r: RefCell<Vec<String>> = RefCell::new(vec![]);` の空の vec![] の要素型)
        if let (Some(Ty::Struct(es)), Ty::Struct(rs)) = (expected, sig0.ret) {
            if !self.prog.has_param(Ty::Struct(es))
                && self.prog.has_param(sig0.ret)
                && self.prog.base_of(es) == self.prog.base_of(rs)
            {
                self.prog.match_ty(sig0.ret, Ty::Struct(es), &mut sol);
            }
        }
        let mut vals: Vec<(Value, Ty, proc_macro2::Span)> = vec![];
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
                let seq = is_vec || matches!(base, Ty::Array(..) | Ty::Ref(_, Ty::Slice(_)));
                if !seq {
                    return None;
                }
                Some(match (t, strip_paren(a)) {
                    (Ty::Ref(..), Expr::Reference(r)) => {
                        let inner = &r.expr;
                        syn::parse_quote!((#inner).iter())
                    }
                    (Ty::Ref(..), _) => syn::parse_quote!((#a).iter()),
                    _ => syn::parse_quote!((#a).into_iter()),
                })
            })
            .collect();
        let conv_refs: Vec<&Expr> = args
            .iter()
            .zip(&conv)
            .map(|(a, c)| c.as_ref().unwrap_or(*a))
            .collect();
        let args: &[&Expr] = &conv_refs;
        for (a, &pt) in args.iter().zip(&sig0.params[skip..]) {
            let mut exp = Some(self.prog.partial_subst(pt, &sol)).filter(|&t| !self.prog.has_param(t));
            // (`[i64; N]` の N がまだ分からなくても、配列のリテラルには要素の型を渡す。長さはリテラルから決まる)
            if exp.is_none() {
                if let Ty::Array(e, n) = self.prog.partial_subst(pt, &sol) {
                    if n >= crate::front::CONST_LEN_BASE && !self.prog.has_param(*e) {
                        if let Expr::Array(al) = strip_paren(a) {
                            exp = Some(Ty::Array(e, al.elems.len() as u32));
                        }
                    }
                }
            }
            // `F: Fn(A) -> B` の引数にクロージャを渡すなら、A / B をクロージャの型の手がかりにする
            let fb = self.fn_bound_of(&sig0, pt);
            // 関数アイテム (`or_insert_with(Vec::new)`) なら、境界を関数ポインタの期待型にして型引数を決める
            if let (Some((bps, bret)), None, Expr::Path(_)) = (&fb, exp, strip_paren(a)) {
                let ft = crate::front::FnTy {
                    params: bps.iter().map(|&t| self.prog.partial_subst(t, &sol)).collect(),
                    ret: self.prog.partial_subst(*bret, &sol),
                };
                if !ft.params.iter().any(|&t| self.prog.has_param(t)) && !self.prog.has_param(ft.ret) {
                    exp = Some(Ty::Fn(crate::front::intern_fn(ft)));
                }
            }
            if let (Some((bps, bret)), Expr::Closure(_)) = (&fb, strip_paren(a)) {
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
            if let (Ty::Ref(pm, Ty::Slice(_)), Ty::Ref(am, Ty::Array(e, n)), Some(val)) = (pt, t, v)
            {
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
                        let (sv, st) = self.call_named("String::from", vec![], &[(val, sref)], a.span())?;
                        if let Some(sv) = sv {
                            self.own_temp(sv, st);
                            v = Some(sv);
                            t = Ty::Ref(false, intern(st));
                        }
                    }
                }
            }
            // (`&Box<T>` も `&T` に)
            while let (Ty::Ref(_, _), Ty::Ref(m, inner @ (Ty::Ref(..) | Ty::Box(_))), Some(val)) =
                (want, t, v)
            {
                if t == want || self.prog.has_param(want) {
                    break;
                }
                // fat な参照 (`&str` / `&[T]`) はメモリ上の 16 byte のアドレスが値なのでそのまま
                v = Some(if inner.is_fat() {
                    val
                } else {
                    self.b.ins().load(PTR, mf(), val, 0)
                });
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
            vals.push((v, t, a.span()));
        }
        let arg_tys: Vec<Ty> = vals.iter().map(|v| v.1).collect();
        // 引数の型から改めて解く。引数に現れない型パラメータ (クロージャの戻り値など) は sol から
        let mut sol0 = self.turbofish_sol(&sig0, turbofish)?;
        for (a, b) in sol0.iter_mut().zip(&sol) {
            if a.is_none() {
                *a = *b;
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
            // 浮動小数点: `1.5` / `2e3` / `1.0f32` / `1f64` (と、浮動小数点が期待される整数リテラル)
            Lit::Float(_) | Lit::Int(_)
                if matches!(l, Lit::Float(_))
                    || matches!(l, Lit::Int(i) if matches!(i.suffix(), "f32" | "f64"))
                    || (matches!(l, Lit::Int(i) if i.suffix().is_empty()) && expected.is_some_and(|t| t.is_float())) =>
            {
                let (digits, suffix) = match l {
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
                let v = if ty == Ty::F32 {
                    self.b.ins().f32const(x as f32)
                } else {
                    self.b.ins().f64const(x)
                };
                Ok((Some(v), ty))
            }
            Lit::Int(i) => {
                let ty = Ty::from_suffix(i.suffix())
                    .or(expected.filter(|t| t.is_int()))
                    .unwrap_or(Ty::I32);
                // 負のリテラル (パターンの `-1` など) も来るので i128 で読む
                let n = i.base10_parse::<i128>().or_else(|_| i.base10_parse::<u128>().map(|v| v as i128)).map_err(|e| e.to_string())?;
                let v = self.int_const(ty, n);
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

    /// of なら None、そうでなければ Some(v)
    fn option_if(&mut self, v: Value, t: Ty, of: Value) -> R<V> {
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
        Ok((Some(out), ot))
    }

    /// `(v, flag)` のタプル
    fn pair_with_bool(&mut self, v: Value, t: Ty, flag: Value) -> R<V> {
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
    fn to_u32(&mut self, v: Value, ct: ir::Type) -> Value {
        match ct.bits() {
            32 => v,
            b if b > 32 => self.b.ins().ireduce(types::I32, v),
            _ => self.b.ins().uextend(types::I32, v),
        }
    }

    /// 128 bit の除算・剰余 (prelude の関数)
    fn div128(&mut self, l: Value, r: Value, t: Ty, rem: bool, span: proc_macro2::Span) -> R<Value> {
        let f = if t.signed() { "__exr_sdiv128" } else { "__exr_udiv128" };
        let rv = self.b.ins().iconst(types::I8, rem as i64);
        let (v, _) = self.call_named(f, vec![Ty::Unit], &[(l, t), (r, t), (rv, Ty::Bool)], span)?;
        Ok(v.unwrap())
    }

    /// iconst (I128 は 64 bit から符号拡張する。Cranelift の iconst は 64 bit まで)
    fn iconst_t(&mut self, ct: ir::Type, x: i64) -> Value {
        if ct == types::I128 {
            let v = self.b.ins().iconst(types::I64, x);
            self.b.ins().sextend(types::I128, v)
        } else {
            self.b.ins().iconst(ct, x)
        }
    }

    /// 整数の定数 (128 bit は 64 bit の 2 つをつなぐ)
    fn int_const(&mut self, ty: Ty, n: i128) -> Value {
        if ty.bits() == 128 {
            let lo = self.b.ins().iconst(types::I64, n as u64 as i64);
            let hi = self.b.ins().iconst(types::I64, (n >> 64) as u64 as i64);
            self.b.ins().iconcat(lo, hi)
        } else {
            self.b.ins().iconst(clif_ty(ty).unwrap(), n as u64 as i64)
        }
    }

    fn unary(&mut self, u: &syn::ExprUnary, expected: Option<Ty>) -> R<V> {
        // `-v` / `!v` (impl Neg / Not): メソッド呼び出し
        if let (UnOp::Neg(_) | UnOp::Not(_), Some(Ty::Struct(sid))) = (&u.op, self.peek(&u.expr)) {
            let m = if matches!(u.op, UnOp::Neg(_)) { "neg" } else { "not" };
            if self.prog.method(sid, m).is_some() {
                let x = &u.expr;
                let mi = syn::Ident::new(m, u.span());
                let e: Expr = syn::parse_quote!((#x).#mi());
                return self.expr(&e, expected);
            }
        }
        // 負のリテラル (`-128i8` / `-2147483648`) はその場で値にする (検査で MIN の符号反転にしない)
        if let (UnOp::Neg(_), Expr::Lit(syn::ExprLit { lit: Lit::Int(i), .. })) = (&u.op, strip_paren(&u.expr)) {
            let lit_e: &Expr = strip_paren(&u.expr);
            let ty = Ty::from_suffix(i.suffix())
                .or(expected.filter(|t| t.is_int()))
                .or_else(|| self.lets.get(&(lit_e as *const Expr as usize)).copied().filter(|t| t.is_int()))
                .or_else(|| self.lets.get(&(e_addr_of_unary(u))).copied().filter(|t| t.is_int()))
                .unwrap_or(Ty::I32);
            if ty.signed() {
                if let Ok(n) = i.base10_parse::<u128>() {
                    let v = self.int_const(ty, (n as i128).wrapping_neg());
                    return Ok((Some(v), ty));
                }
            }
        }
        let (v, t) = self.expr(&u.expr, expected)?;
        let Some(v) = v else { return Ok((None, t)) };
        match u.op {
            UnOp::Neg(_) if t.is_int() && t.signed() => {
                if self.overflow_checks {
                    let min = self.int_const(t, crate::front::prim_assoc_const(t, "MIN").unwrap());
                    let bad = self.b.ins().icmp(IntCC::Equal, v, min);
                    self.ovf_panic(bad, 6, u.span());
                }
                Ok((Some(self.b.ins().ineg(v)), t))
            }
            UnOp::Neg(_) if t.is_float() => Ok((Some(self.b.ins().fneg(v)), t)),
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

        // 演算子のオーバーロード (`impl Add for V` / String + &str): `a.add(b)` / `a.add_assign(b)`
        if let Some(Ty::Struct(sid)) = self.peek(&b.left) {
            let m = match b.op {
                Add(_) => "add",
                Sub(_) => "sub",
                Mul(_) => "mul",
                Div(_) => "div",
                Rem(_) => "rem",
                BitAnd(_) => "bitand",
                BitOr(_) => "bitor",
                BitXor(_) => "bitxor",
                Shl(_) => "shl",
                Shr(_) => "shr",
                AddAssign(_) => "add_assign",
                SubAssign(_) => "sub_assign",
                MulAssign(_) => "mul_assign",
                DivAssign(_) => "div_assign",
                RemAssign(_) => "rem_assign",
                BitAndAssign(_) => "bitand_assign",
                BitOrAssign(_) => "bitor_assign",
                BitXorAssign(_) => "bitxor_assign",
                ShlAssign(_) => "shl_assign",
                ShrAssign(_) => "shr_assign",
                _ => "",
            };
            if !m.is_empty() && self.prog.method(sid, m).is_some() {
                let (l, r) = (&b.left, &b.right);
                let mi = syn::Ident::new(m, b.span());
                let e: Expr = syn::parse_quote!((#l).#mi(#r));
                return self.expr(&e, expected);
            }
        }
        // `&a + &b` (`impl Add for &M`): `(&a).__ref_add(&b)`
        if let Some(Ty::Ref(_, Ty::Struct(sid))) = self.peek(&b.left) {
            let m = match b.op {
                Add(_) => "add",
                Sub(_) => "sub",
                Mul(_) => "mul",
                Div(_) => "div",
                Rem(_) => "rem",
                BitAnd(_) => "bitand",
                BitOr(_) => "bitor",
                BitXor(_) => "bitxor",
                _ => "",
            };
            let rm = format!("__ref_{m}");
            if !m.is_empty() && self.prog.method(*sid, &rm).is_some() {
                let (l, r) = (&b.left, &b.right);
                let mi = syn::Ident::new(&rm, b.span());
                let e: Expr = syn::parse_quote!((#l).#mi(#r));
                return self.expr(&e, expected);
            }
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
        // 比較は参照で比べるのでムーブしない (一時値は文の終わりまで所有する)
        let cmp = is_cmp(&b.op);
        // `x * 10` (x: &i64): リテラルの型は参照の中身
        let deref_num = |t: Option<Ty>| match t {
            Some(Ty::Ref(_, i)) if i.is_int() || i.is_float() => Some(*i),
            t => t,
        };
        let hint = deref_num(hint);
        let (l, lt) = self.operand(&b.left, hint, cmp)?;
        let Some(l) = l else {
            return Ok((None, Ty::Never));
        };
        let rhint = if is_shift {
            self.peek(&b.right)
        } else {
            deref_num(Some(lt))
        };
        // (`v < (1, 5)` / `v == 2`: 右辺がリテラルなら、左の型の `PartialOrd<R>` / `PartialEq<R>` の R を期待型にする)
        let rhint = match b.op {
            Lt(_) | Le(_) | Gt(_) | Ge(_) => self.rhs_hint_from_impls(&self.prog.pord.clone(), lt, &b.right).or(rhint),
            Eq(_) | Ne(_) => self.rhs_hint_from_impls(&self.prog.peq.clone(), lt, &b.right).or(rhint),
            _ => rhint,
        };
        let (r, rt) = self.operand(&b.right, rhint.or(Some(lt)), cmp)?;
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
        if !matches!(b.op, Eq(_) | Ne(_)) || !lt.is_agg() {
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
        let str_ref = Ty::Ref(false, intern(Ty::Str));
        if matches!(b.op, Eq(_) | Ne(_)) && lt != rt {
            if let (Ty::Struct(sid), true) = (lt, rt == str_ref) {
                if let Some((f, ft)) = self.deref_to_slice(l, sid, false) {
                    (l, lt) = (f, ft);
                }
            } else if let (true, Ty::Struct(sid)) = (lt == str_ref, rt) {
                if let Some((f, ft)) = self.deref_to_slice(r, sid, false) {
                    (r, rt) = (f, ft);
                }
            }
        }
        // 型の違う `<` / `<=` / `>` / `>=`: `impl PartialOrd<R> for S` の partial_cmp (None ならどれも false)
        if matches!(b.op, Lt(_) | Le(_) | Gt(_) | Ge(_)) && lt != rt {
            let cands = self.prog.pord.clone();
            if let Some((v, t)) = self.trait_bin(&cands, l, lt, r, rt, b.span())? {
                let (c, _) = self.call_named("__exr_pord_code", vec![Ty::Unit], &[(v.unwrap(), Ty::Ref(false, intern(t)))], b.span())?;
                let c = c.unwrap();
                let e = self.ord_code_test(c, &b.op);
                return Ok((Some(e), Ty::Bool));
            }
        }
        // `&str` / `String` の大小比較: prelude の __exr_str_cmp
        if matches!(b.op, Lt(_) | Le(_) | Gt(_) | Ge(_)) && self.prog.index.contains_key("__exr_str_cmp") {
            let str_ref = Ty::Ref(false, intern(Ty::Str));
            let as_str = |this: &mut Self, v: Value, t: Ty| -> Option<Value> {
                match t {
                    Ty::Ref(_, Ty::Str) => Some(v),
                    Ty::Struct(sid) => this.deref_to_slice(v, sid, false).filter(|(_, ft)| *ft == str_ref).map(|f| f.0),
                    Ty::Ref(_, Ty::Struct(sid)) => {
                        this.deref_to_slice(v, *sid, false).filter(|(_, ft)| *ft == str_ref).map(|f| f.0)
                    }
                    _ => None,
                }
            };
            if let (Some(x), Some(y)) = (as_str(self, l, lt), as_str(self, r, rt)) {
                let (c, _) = self.call_named("__exr_str_cmp", vec![Ty::Unit], &[(x, str_ref), (y, str_ref)], b.span())?;
                let cc = match b.op {
                    Lt(_) => IntCC::SignedLessThan,
                    Le(_) => IntCC::SignedLessThanOrEqual,
                    Gt(_) => IntCC::SignedGreaterThan,
                    _ => IntCC::SignedGreaterThanOrEqual,
                };
                return Ok((Some(self.b.ins().icmp_imm_s(cc, c.unwrap(), 0)), Ty::Bool));
            }
        }
        // タプル・struct・enum・配列・スライス・Vec などの大小比較: 型ごとの glue (__exr_cmp)
        if matches!(b.op, Lt(_) | Le(_) | Gt(_) | Ge(_)) && !(lt.is_int() || lt.is_float() || matches!(lt, Ty::Bool | Ty::Char)) {
            self.same(b.right.span(), rt, lt)?;
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
                    let e: Expr = syn::parse_quote!(__exr_pord_code::<()>(&__exr_ca.partial_cmp(__exr_cb)));
                    let (c, _) = self.expr(&e, Some(Ty::I32))?;
                    self.scopes.pop();
                    let e = self.ord_code_test(c.unwrap(), &b.op);
                    return Ok((Some(e), Ty::Bool));
                }
            }
            let c = self.cmp_call(la, ra, t)?;
            let cc = match b.op {
                Lt(_) => IntCC::SignedLessThan,
                Le(_) => IntCC::SignedLessThanOrEqual,
                Gt(_) => IntCC::SignedGreaterThan,
                _ => IntCC::SignedGreaterThanOrEqual,
            };
            return Ok((Some(self.b.ins().icmp_imm_s(cc, c, 0)), Ty::Bool));
        }
        // 型の違う `==` / `!=`: 右辺の型を持つ PartialEq の impl (`impl PartialEq<[T; N]> for Vec<T>` など) を探して eq を呼ぶ
        if matches!(b.op, Eq(_) | Ne(_)) && lt != rt {
            if let Some(e) = self.trait_eq(l, lt, r, rt, b.span())? {
                let e = if matches!(b.op, Ne(_)) { self.b.ins().bxor_imm_u(e, 1) } else { e };
                return Ok((Some(e), Ty::Bool));
            }
        }
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
            // 浮動小数点の比較 (NaN は == / < などで false、!= で true)
            if lt.is_float() {
                let cc = match b.op {
                    Eq(_) => FloatCC::Equal,
                    Ne(_) => FloatCC::NotEqual,
                    Lt(_) => FloatCC::LessThan,
                    Le(_) => FloatCC::LessThanOrEqual,
                    Gt(_) => FloatCC::GreaterThan,
                    _ => FloatCC::GreaterThanOrEqual,
                };
                return Ok((Some(self.b.ins().fcmp(cc, l, r)), Ty::Bool));
            }
            if !(lt.is_int() || matches!(lt, Ty::Bool | Ty::Char | Ty::Ptr(..))) {
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

    /// `l == r` (型が違う) を PartialEq<R> の impl で。self 型 S と右辺 R は、値の型そのものか、
    /// 参照の先 (`&[T]` と `impl .. for [T]`、`&[T; N]` と `PartialEq<[T; N]>`) で合わせる
    fn trait_eq(&mut self, l: Value, lt: Ty, r: Value, rt: Ty, span: proc_macro2::Span) -> R<Option<Value>> {
        let cands = self.prog.peq.clone();
        Ok(self.trait_bin(&cands, l, lt, r, rt, span)?.and_then(|v| v.0))
    }

    /// cands (`fn m(&self, other: &R)` の候補) から l と r の型に合うものを選んで呼ぶ
    fn trait_bin(&mut self, cands: &[u32], l: Value, lt: Ty, r: Value, rt: Ty, span: proc_macro2::Span) -> R<Option<V>> {
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
                    let v = self.call_named(
                        &name,
                        targs,
                        &[(la, Ty::Ref(false, intern(xl))), (ra, Ty::Ref(false, intern(xr)))],
                        span,
                    )?;
                    return Ok(Some(v));
                }
            }
        }
        Ok(None)
    }

    /// partial_cmp の結果のコード (-1 / 0 / 1、None は 2) で `<` などを決める
    fn ord_code_test(&mut self, c: Value, op: &BinOp) -> Value {
        let is = |this: &mut Self, k: i64| this.b.ins().icmp_imm_s(IntCC::Equal, c, k);
        match op {
            BinOp::Lt(_) => is(self, -1),
            BinOp::Gt(_) => is(self, 1),
            // (<= は -1 か 0、>= は 0 か 1。None (2) はどれも false)
            BinOp::Le(_) => {
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
    fn rhs_hint_from_impls(&self, cands: &[u32], lt: Ty, rhs: &Expr) -> Option<Ty> {
        if cands.is_empty() {
            return None;
        }
        let fits = |t: Ty, e: &Expr| -> bool {
            match strip_paren(e) {
                Expr::Lit(syn::ExprLit { lit: Lit::Int(_), .. }) => t.is_int(),
                Expr::Lit(syn::ExprLit { lit: Lit::Float(_), .. }) => t.is_float(),
                Expr::Unary(u) if matches!(u.op, UnOp::Neg(_)) => t.is_int() || t.is_float(),
                Expr::Tuple(tp) => matches!(t, Ty::Struct(sid) if self.prog.is_tuple(self.prog.base_of(sid))
                    && self.prog.structs[sid as usize].args.len() == tp.elems.len()),
                Expr::Array(a) => matches!(t, Ty::Array(_, n) if n as usize == a.elems.len()),
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
    fn addr_of_value(&mut self, v: Value, t: Ty) -> Value {
        if t.is_agg() {
            return v;
        }
        let a = self.temp(t);
        self.b.ins().store(mf(), v, a, 0);
        a
    }

    /// a, b (t 型の値。集約型ならアドレス) が等しいか (i8 の 0/1)
    fn agg_eq(&mut self, a: Value, b: Value, t: Ty, span: proc_macro2::Span) -> R<Value> {
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

    fn eq_at(&mut self, a: Value, b: Value, t: Ty, span: proc_macro2::Span) -> R<Value> {
        let one = |this: &mut Self| this.b.ins().iconst(types::I8, 1);
        match t {
            Ty::Unit => Ok(one(self)),
            t if t.is_int() || matches!(t, Ty::Bool | Ty::Char | Ty::Ptr(..)) => {
                Ok(self.b.ins().icmp(IntCC::Equal, a, b))
            }
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
                self.b.ins().brif(same_len, head, &[], done, &[zero8.into()]);
                self.b.switch_to_block(head);
                let iv = self.b.use_var(i);
                let more = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, la);
                let one8 = self.b.ins().iconst(types::I8, 1);
                self.b.ins().brif(more, body, &[], done, &[one8.into()]);
                self.b.switch_to_block(body);
                let off = self.b.ins().imul_imm_s(iv, es as i64);
                let xa = self.b.ins().iadd(pa, off);
                let xb = self.b.ins().iadd(pb, off);
                let (x, y) = match clif_ty(e) {
                    Some(ct) if !e.is_agg() => (
                        self.b.ins().load(ct, mf(), xa, 0),
                        self.b.ins().load(ct, mf(), xb, 0),
                    ),
                    _ => (xa, xb),
                };
                let eq = self.agg_or_scalar_eq(x, y, e, span)?;
                let next = self.b.ins().iadd_imm_s(iv, 1);
                self.b.def_var(i, next);
                let zero8 = self.b.ins().iconst(types::I8, 0);
                self.b.ins().brif(eq, head, &[], done, &[zero8.into()]);
                self.b.switch_to_block(done);
                Ok(res)
            }
            // (Box の先の struct は glue 関数で: 再帰型をその場で展開しない)
            Ty::Box(inner @ Ty::Struct(_)) if self.prog.index.contains_key("__exr_eq") => {
                let r = Ty::Ref(false, intern(*inner));
                let (v, _) = self.call_named("__exr_eq", vec![*inner], &[(a, r), (b, r)], span)?;
                Ok(v.unwrap())
            }
            // 参照と Box は指す先の値で比べる
            Ty::Ref(_, inner) | Ty::Box(inner) if !t.is_fat() => {
                let (x, y) = match clif_ty(*inner) {
                    Some(ct) if !inner.is_agg() => (
                        self.b.ins().load(ct, mf(), a, 0),
                        self.b.ins().load(ct, mf(), b, 0),
                    ),
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
                // フィールドが数値だけなら分岐せずに全バリアントを比べる (読んでも安全)
                let plain = variants.iter().all(|(_, fs)| {
                    fs.iter().all(|f| f.1.is_int() || f.1.is_float() || matches!(f.1, Ty::Bool | Ty::Char | Ty::Unit))
                });
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
                self.b.ins().brif(tag_eq, start, &[], done, &[zero.into()]);
                self.b.switch_to_block(start);
                for (discr, fields) in variants {
                    if fields.is_empty() {
                        let is_v = self.b.ins().icmp_imm_s(IntCC::Equal, ta, discr);
                        let one_v = self.b.ins().iconst(types::I8, 1);
                        let next = self.b.create_block();
                        self.b.ins().brif(is_v, done, &[one_v.into()], next, &[]);
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
                    self.b.ins().jump(done, &[acc.into()]);
                    self.b.switch_to_block(next);
                }
                let z2 = self.b.ins().iconst(types::I8, 0);
                self.b.ins().jump(done, &[z2.into()]);
                self.b.switch_to_block(done);
                Ok(res)
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
            if self.overflow_checks && !self.iconst_of(r).is_some_and(|k| (0..lt.bits() as i64).contains(&k)) {
                // 型の幅以上のシフト (負の量も符号なしで大きい)
                let bad = self.b.ins().icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, r, lt.bits() as i64);
                self.ovf_panic(bad, if matches!(op, Shl(_)) { 7 } else { 8 }, b.span());
            }
            let ins = self.b.ins();
            return Ok(match op {
                Shl(_) => ins.ishl(l, r),
                _ if lt.signed() => ins.sshr(l, r),
                _ => ins.ushr(l, r),
            });
        }
        // char の範囲 (`'a'..='e'`) の prelude が `v + 1` で次の文字にする
        if lt == Ty::Char && rt.is_int() && matches!(op, Add(_) | Sub(_)) {
            let r32 = match rt.bits() {
                32 => r,
                b if b > 32 => self.b.ins().ireduce(types::I32, r),
                _ => self.b.ins().uextend(types::I32, r),
            };
            return Ok(if matches!(op, Add(_)) { self.b.ins().iadd(l, r32) } else { self.b.ins().isub(l, r32) });
        }
        self.same(b.right.span(), rt, lt)?;
        if lt.is_float() {
            let ins = self.b.ins();
            return Ok(match op {
                Add(_) => ins.fadd(l, r),
                Sub(_) => ins.fsub(l, r),
                Mul(_) => ins.fmul(l, r),
                Div(_) => ins.fdiv(l, r),
                // (Cranelift に剰余はないので libm の fmod)
                Rem(_) => {
                    let f = if lt == Ty::F32 { "__exr_fmodf" } else { "__exr_fmod" };
                    let (v, _) = self.call_named(f, vec![], &[(l, lt), (r, lt)], b.span())?;
                    v.unwrap()
                }
                _ => return err(b.span(), format!("unsupported operator for `{}`", self.prog.tn(lt))),
            });
        }
        let bool_ok = matches!(op, BitAnd(_) | BitOr(_) | BitXor(_));
        if !(lt.is_int() || (lt == Ty::Bool && bool_ok)) {
            return err(
                b.span(),
                format!("unsupported operator for `{}`", self.prog.tn(lt)),
            );
        }
        let s = lt.signed();
        // 128 bit の除算は Cranelift にないので prelude の関数
        if lt.bits() == 128 && matches!(op, Div(_) | Rem(_)) {
            return self.div128(l, r, lt, matches!(op, Rem(_)), b.span());
        }
        // 0 除算と MIN / -1 は (rustc と同じく最適化しても) panic
        if lt.is_int() && matches!(op, Div(_) | Rem(_)) && self.user_code {
            let rem = matches!(op, Rem(_));
            let cr = self.iconst_of(r);
            if cr.is_none() || cr == Some(0) {
                let zero = self.b.ins().icmp_imm_u(IntCC::Equal, r, 0);
                self.ovf_panic(zero, if rem { 4 } else { 3 }, b.span());
            }
            if s && (cr.is_none() || cr == Some(-1)) {
                let min = self.int_const(lt, crate::front::prim_assoc_const(lt, "MIN").unwrap());
                let am = self.b.ins().icmp(IntCC::Equal, l, min);
                let bm = self.b.ins().icmp_imm_s(IntCC::Equal, r, -1);
                let both = self.b.ins().band(am, bm);
                self.ovf_panic(both, if rem { 10 } else { 9 }, b.span());
            }
        }
        // 足し算・引き算・掛け算のオーバーフロー
        if lt.is_int() && self.overflow_checks && matches!(op, Add(_) | Sub(_) | Mul(_)) {
            let kind = match op {
                Add(_) => 0,
                Sub(_) => 1,
                _ => 2,
            };
            if lt.bits() < 128 {
                let (v, of) = match (kind, s) {
                    (0, true) => self.b.ins().sadd_overflow(l, r),
                    (0, false) => self.b.ins().uadd_overflow(l, r),
                    (1, true) => self.b.ins().ssub_overflow(l, r),
                    (1, false) => self.b.ins().usub_overflow(l, r),
                    (_, true) => self.b.ins().smul_overflow(l, r),
                    (_, false) => self.b.ins().umul_overflow(l, r),
                };
                self.ovf_panic(of, kind, b.span());
                return Ok(v);
            }
            // (128 bit: 足し算・引き算は結果の符号と大小で見る。掛け算は検査しない)
            if kind < 2 {
                let v = if kind == 0 { self.b.ins().iadd(l, r) } else { self.b.ins().isub(l, r) };
                let of = if !s {
                    if kind == 0 {
                        self.b.ins().icmp(IntCC::UnsignedLessThan, v, l)
                    } else {
                        self.b.ins().icmp(IntCC::UnsignedLessThan, l, r)
                    }
                } else {
                    let x = if kind == 0 { self.b.ins().bxor(l, v) } else { self.b.ins().bxor(l, r) };
                    let y = if kind == 0 { self.b.ins().bxor(r, v) } else { self.b.ins().bxor(l, v) };
                    let m = self.b.ins().band(x, y);
                    let z = self.int_const(lt, 0);
                    self.b.ins().icmp(IntCC::SignedLessThan, m, z)
                };
                self.ovf_panic(of, kind, b.span());
                return Ok(v);
            }
        }
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
                (Some(v), Ty::Ref(_, i) | Ty::Box(i)) => Place::Mem(v, 0, *i),
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
        // `&s[a..b]` (String / &str): バイト位置で切り、文字の境界でなければ panic
        let bt = self.peek(base).map(|t| self.strip_refs(t));
        let is_string = matches!(bt, Some(Ty::Struct(sid))
            if self.prog.structs[sid as usize].base.is_none() && self.prog.structs[sid as usize].name == "String");
        let str_fat = if is_string {
            let e: Expr = syn::parse_quote!((#base).as_str());
            Some(self.expr(&e, None)?)
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
            if let Some(d) = a.elems.iter().find_map(|e| match e {
                Expr::Cast(c) => match self.prog.parse_ty(&c.ty, &self.env) {
                    Ok(t @ Ty::Ref(_, Ty::Dyn(_))) => Some(t),
                    _ => None,
                },
                _ => None,
            }) {
                elem = Some(d);
            }
        }
        let mut vals = vec![];
        for e in &a.elems {
            let (v, t) = self.expr(e, elem)?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            match elem {
                Some(et) if soft && t != et => elem = Some(t),
                Some(et) => self.same(e.span(), t, et)?,
                None => elem = Some(t),
            }
            soft = false;
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
            Expr::Path(p) if self.const_generic(p).is_some() => Ok(self.const_generic(p).unwrap()),
            Expr::Path(p) if p.path.get_ident().is_some() || self.const_of(p).is_some() => {
                // (関連定数 `S::N` は const の名前 `S::N` で引く)
                let name = match self.const_of(p) {
                    Some(ci) => self.prog.consts[ci as usize].name.clone(),
                    None => p.path.get_ident().unwrap().to_string(),
                };
                let ts = crate::front::TySyn::Array(
                    Box::new(crate::front::TySyn::Prim(Ty::U8)),
                    crate::front::ArrLen::Const(name),
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
            "split_at" => Some(("__exr_slice_split_at", false)),
            "split_at_mut" => Some(("__exr_slice_split_at_mut", true)),
            _ => None,
        }
        .filter(|(f, _)| self.prog.index.contains_key(*f));
        // `names.join(", ")` / `parts.concat()` (要素が &str / String の Vec・配列・スライス)
        if matches!(name.as_str(), "join" | "concat")
            && mc.args.len() <= 1
            && self.prog.index.contains_key("__exr_join_str")
        {
            let own = match self.peek(&mc.receiver).map(|t| self.strip_refs(t)) {
                Some(Ty::Struct(sid)) => self.vec_fields(sid).is_none() && self.prog.method(sid, &name).is_some(),
                _ => false,
            };
            if !own {
                // (一時値の受け手 `vec![..].join(..)` は `&` を付けて評価し、文の終わりに drop させる)
                let r = &mc.receiver;
                let re: Expr = if is_place_expr(r) { (**r).clone() } else { syn::parse_quote!(&(#r)) };
                let (base, len, elem) = self.indexable(&re)?;
                // Vec の Vec の concat: 要素を clone して 1 つの Vec に
                if let (Ty::Struct(es), "concat") = (elem, name.as_str()) {
                    if let Some((_, _, inner)) = self.vec_fields(es) {
                        let fat = self.make_fat_v(base, len);
                        let st = Ty::Ref(false, intern(Ty::Slice(intern(elem))));
                        let (v, t) = self.call_named("__exr_concat_vec", vec![inner], &[(fat, st)], mc.span())?;
                        return Ok(Some((v, t)));
                    }
                }
                let f = match elem {
                    Ty::Ref(_, Ty::Str) => "__exr_join_str",
                    Ty::Struct(es) if self.prog.structs[es as usize].name == "String" => "__exr_join_string",
                    t => return err(mc.span(), format!("`{name}` on elements of type `{}` is not supported", self.prog.tn(t))),
                };
                let fat = self.make_fat_v(base, len);
                let st = Ty::Ref(false, intern(Ty::Slice(intern(elem))));
                let sref = Ty::Ref(false, intern(Ty::Str));
                let sep = match mc.args.first() {
                    Some(a) if self.peek(a) == Some(Ty::Char) => {
                        let (v, _) = self.expr(a, Some(Ty::Char))?;
                        self.char_to_str(v.unwrap(), a.span())?
                    }
                    Some(a) => {
                        let (v, t) = self.expr(a, Some(sref))?;
                        self.same(a.span(), t, sref)?;
                        v.unwrap()
                    }
                    None => {
                        let e: Expr = syn::parse_quote!("");
                        self.expr(&e, Some(sref))?.0.unwrap()
                    }
                };
                return self.call_named(f, vec![Ty::Unit], &[(fat, st), (sep, sref)], mc.span()).map(Some);
            }
        }
        // `v.drain(a..b)` / `v.drain(..)`: Vec::__drain(lo, hi)
        if name == "drain" && mc.args.len() == 1 {
            if let (Expr::Range(rg), Some(Ty::Struct(sid))) = (strip_paren(&mc.args[0]), self.peek(&mc.receiver).map(|t| self.strip_refs(t))) {
                if self.vec_fields(sid).is_some() && self.prog.method(sid, "__drain").is_some() {
                    let r = &mc.receiver;
                    let lo: Expr = match &rg.start {
                        Some(e) => (**e).clone(),
                        None => syn::parse_quote!(0usize),
                    };
                    let hi: Expr = match (&rg.end, &rg.limits) {
                        (Some(e), syn::RangeLimits::Closed(_)) => syn::parse_quote!((#e) + 1),
                        (Some(e), _) => (**e).clone(),
                        (None, _) => syn::parse_quote!((#r).len()),
                    };
                    let e: Expr = syn::parse_quote!((#r).__drain(#lo, #hi));
                    return self.expr(&e, None).map(Some);
                }
            }
        }
        // `v[..n].sort()`: 範囲の添字は自動で借用する
        if let (Some((f, m)), Expr::Index(ix)) = (slice_fn, strip_paren(&mc.receiver)) {
            if matches!(&*ix.index, Expr::Range(_)) {
                let r = &mc.receiver;
                let e: Expr = if m { syn::parse_quote!(&mut #r) } else { syn::parse_quote!(&#r) };
                let (v, t) = self.expr(&e, None)?;
                let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
                let id = self.prog.index[f];
                let args: Vec<&Expr> = mc.args.iter().collect();
                return self.call_fn(id, Some((v, t)), &args, None, None, mc.span()).map(Some);
            }
        }
        let Some(mut rt) = self.peek(&mc.receiver) else {
            return Ok(None);
        };
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
                let r = &mc.receiver;
                let m = &mc.method;
                let args = &mc.args;
                let tf = &mc.turbofish;
                let e: Expr = syn::parse_quote!((#r).as_str().#m #tf (#args));
                // (型推論が元の呼び出しに記録した型を、作り直したノードにも写す: `t.parse()` の T)
                if let Some(&t) = self.lets.get(&(mc as *const syn::ExprMethodCall as usize)) {
                    self.lets.insert(&e as *const Expr as usize, t);
                    if let Expr::MethodCall(nm) = &e {
                        self.lets.insert(nm as *const syn::ExprMethodCall as usize, t);
                    }
                }
                let r = self.expr(&e, None).map(Some);
                // (e はここで捨てるので、そのアドレスの記録も消す)
                self.lets.remove(&(&e as *const Expr as usize));
                if let Expr::MethodCall(nm) = &e {
                    self.lets.remove(&(nm as *const syn::ExprMethodCall as usize));
                }
                return r;
            }
        }
        if let (Some((f, m)), Ty::Array(..) | Ty::Ref(_, Ty::Slice(_))) = (slice_fn, rt) {
            if let Some(&id) = self.prog.index.get(f) {
                // 受け手を `&[T]` / `&mut [T]` にして、引数 (クロージャも) は元の式のまま渡す
                let (base, len, elem) = self.indexable(&mc.receiver)?;
                let (fat, st) = (self.make_fat_v(base, len), Ty::Ref(m, intern(Ty::Slice(intern(elem)))));
                let args: Vec<&Expr> = mc.args.iter().collect();
                return self.call_fn(id, Some((fat, st)), &args, None, None, mc.span()).map(Some);
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
                    let (v, vt) = self.expr(&mc.receiver, None)?;
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
            // 値としての `xs.iter()` / `xs.iter_mut()`: prelude の Iter / IterMut
            "iter" | "iter_mut" if self.prog.index.contains_key("__exr_slice_iter") || name == "iter_mut" => {
                let (base, len, elem) = self.indexable(&mc.receiver)?;
                let m = name == "iter_mut";
                let fat = self.make_fat_v(base, len);
                let f = if m { "__exr_slice_iter_mut" } else { "__exr_slice_iter" };
                self.call_named(f, vec![elem], &[(fat, Ty::Ref(m, intern(Ty::Slice(intern(elem)))))], mc.span())
                    .map(Some)
            }
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
        let (v, mut t) = self.expr(&mc.receiver, None)?;
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
            // split / lines / trim / starts_with / ends_with / contains / find: prelude の関数
            "split" | "split_whitespace" | "lines" | "trim" | "starts_with" | "ends_with" | "contains" | "find"
                if !(m_name(mc) == "split" && mc.args.first().is_some_and(|a| self.peek(a) != Some(Ty::Char)))
                    && self.prog.index.contains_key(match m_name(mc).as_str() {
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
                let name = m_name(mc);
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
                for a in &mc.args {
                    // char のパターン (`s.contains('x')`) は UTF-8 にして &str で渡す
                    let char_pat = name != "split" && self.peek(a) == Some(Ty::Char);
                    let exp = if name == "split" || char_pat { Ty::Char } else { sref };
                    let (v, t) = self.expr(a, Some(exp))?;
                    if name == "split" && t != Ty::Char {
                        return err(a.span(), "only `char` separators are supported in `split`");
                    }
                    self.same(a.span(), t, exp)?;
                    if char_pat {
                        let ss = self.b.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            4,
                            0,
                        ));
                        let buf = self.b.ins().stack_addr(PTR, ss, 0);
                        let bt = Ty::Ptr(true, intern(Ty::U8));
                        let (n, _) = self.call_named(
                            "__exr_char_utf8",
                            vec![Ty::Unit],
                            &[(v.unwrap(), Ty::Char), (buf, bt)],
                            a.span(),
                        )?;
                        let fat = self.make_fat_v(buf, n.unwrap());
                        args.push((fat, sref));
                        continue;
                    }
                    args.push((v.unwrap(), t));
                }
                let r = self.call_named(f, vec![Ty::Unit], &args, mc.span())?;
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
                    .call_named("__exr_str_chars", vec![Ty::Unit], &[(fat, Ty::Ref(false, intern(Ty::Str)))], mc.span())
                    .map(Some);
            }
            // `"..".to_string()` / `to_owned()` は prelude の String::from
            "to_string" | "to_owned" => {
                return self
                    .call_named("String::from", vec![], &[(fat, Ty::Ref(false, intern(Ty::Str)))], mc.span())
                    .map(Some);
            }
            m => {
                // prelude の `__exr_str_<m>` (`s.split("ab")` は __exr_str_split_str)
                // (パターンがクロージャなら `__exr_str_<m>_fn`)
                let fname = format!("{m}_fn");
                let h = if m == "split" {
                    "split_str"
                } else if matches!(mc.args.first(), Some(Expr::Closure(_)))
                    && self.prog.index.contains_key(&format!("__exr_str_{fname}"))
                {
                    fname.as_str()
                } else {
                    m
                };
                if let Some(r) = self.helper_method("__exr_str_", h, mc, (fat, Ty::Ref(false, intern(Ty::Str))))? {
                    return Ok(Some(r));
                }
                return err(mc.method.span(), format!("no method `{m}` on `&str`"));
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
            "checked_pow",
            "wrapping_pow",
            "saturating_pow",
            "overflowing_pow",
            "overflowing_add",
            "overflowing_sub",
            "overflowing_mul",
            "rem_euclid",
            "div_euclid",
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
        ];
        if !known.contains(&name.as_str()) {
            return Ok(None);
        }
        // 受け手が `&u64` / `&mut u64` なら指す先の値 (自動 deref)
        let (a, a_ty) = if matches!(self.peek(&mc.receiver), Some(Ty::Ref(..))) {
            self.expr(&mc.receiver, None)?
        } else {
            self.expr(&mc.receiver, Some(t))?
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
            "abs" => {
                // (debug と同じく MIN の abs は panic)
                if self.overflow_checks && s {
                    let min = self.int_const(t, crate::front::prim_assoc_const(t, "MIN").unwrap());
                    let bad = self.b.ins().icmp(IntCC::Equal, a, min);
                    self.ovf_panic(bad, 6, mc.span());
                }
                (Some(self.b.ins().iabs(a)), t)
            }
            "checked_abs" | "checked_neg" => {
                let v = if name == "checked_abs" { self.b.ins().iabs(a) } else { self.b.ins().ineg(a) };
                let bad = if s {
                    let min = self.int_const(t, crate::front::prim_assoc_const(t, "MIN").unwrap());
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
                        self.ovf_panic(of, 2, mc.span());
                        (Some(v), t)
                    }
                    "saturating_pow" => {
                        // 負の底の奇数乗なら MIN、それ以外は MAX
                        let max = self.int_const(t, crate::front::prim_assoc_const(t, "MAX").unwrap());
                        let sat = if s {
                            let min = self.int_const(t, crate::front::prim_assoc_const(t, "MIN").unwrap());
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
                let max = self.int_const(t, crate::front::prim_assoc_const(t, "MAX").unwrap());
                let min = self.int_const(t, crate::front::prim_assoc_const(t, "MIN").unwrap());
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
                            let min = self.int_const(t, crate::front::prim_assoc_const(t, "MIN").unwrap());
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
                            let span = mc.span();
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
                    "overflowing_add" if s => self.b.ins().sadd_overflow(a, b),
                    "overflowing_add" => self.b.ins().uadd_overflow(a, b),
                    "overflowing_sub" if s => self.b.ins().ssub_overflow(a, b),
                    "overflowing_sub" => self.b.ins().usub_overflow(a, b),
                    _ if s => self.b.ins().smul_overflow(a, b),
                    _ => self.b.ins().umul_overflow(a, b),
                };
                self.pair_with_bool(v, t, of)?
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
                let (v, _) = self.call_named("__exr_isqrt", vec![Ty::Unit], &[(w, Ty::U64)], mc.span())?;
                let v = v.unwrap();
                let v = if ct == types::I64 { v } else { self.b.ins().ireduce(ct, v) };
                (Some(v), t)
            }
            "try_into" => {
                // 型は型推論の記録 (Result<T, TryFromIntError>) から
                let rt = self.lets.get(&(mc as *const syn::ExprMethodCall as usize)).copied().or_else(|| {
                    if self.in_fmt_args > 0 {
                        self.lets.get(&crate::infer::pos_key(&Expr::MethodCall(mc.clone()))).copied()
                    } else {
                        None
                    }
                });
                let Some(Ty::Struct(rs)) = rt else {
                    return err(mc.span(), "cannot infer the target type of `try_into`");
                };
                let Some(&dt) = self.prog.structs[rs as usize].args.first() else {
                    return err(mc.span(), "cannot infer the target type of `try_into`");
                };
                if !dt.is_int() {
                    return err(mc.span(), "`try_into` is only supported between integer types");
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
                    let lo = self.int_const(Ty::I128, crate::front::prim_assoc_const(dt, "MIN").unwrap());
                    self.b.ins().icmp(IntCC::SignedGreaterThanOrEqual, w, lo)
                } else {
                    self.b.ins().iconst(types::I8, 1)
                };
                let hi_ok = if dt == Ty::U128 {
                    self.b.ins().iconst(types::I8, 1)
                } else {
                    let hi = self.int_const(Ty::I128, crate::front::prim_assoc_const(dt, "MAX").unwrap());
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
        self.pending_label = f.label.as_ref().map(|l| l.name.ident.to_string());
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
        // 配列・スライス・Vec でなければ Iterator: `while let Some(pat) = it.next() { .. }`
        let base_ty = self.peek(match e {
            Expr::Reference(r) => &r.expr,
            e => e,
        });
        let mut bt = base_ty;
        while let Some(Ty::Ref(_, i)) = bt {
            if bt.is_some_and(|t| t.is_fat()) {
                break;
            }
            bt = Some(*i);
        }
        // (型が分からないメソッド・関数呼び出し (`s.split(' ')` など) もイテレータとみなす)
        let iterator = match bt {
            Some(Ty::Struct(sid)) => self.vec_fields(sid).is_none(),
            None => matches!(e, Expr::MethodCall(_) | Expr::Call(_)),
            _ => false,
        };
        if iterator {
            // `for (k, v) in &map`: next がなければ iter() / iter_mut() / into_iter() を通す
            let struct_has_next = |cx: &Self, t: Option<Ty>| match t {
                Some(Ty::Struct(s)) => cx.prog.method(s, "next").is_some(),
                _ => true,
            };
            // (`m.iter_mut()` は剥がした後の m ではなく、式全体の型で見る)
            let has_next = struct_has_next(self, bt)
                || matches!(strip_paren(&f.expr), Expr::MethodCall(_))
                    && struct_has_next(self, self.peek(&f.expr));
            // (ユーザの `impl IntoIterator for &Coll` / `for Coll`)
            let user_into = match bt {
                Some(Ty::Struct(s)) => {
                    let by_ref = matches!(strip_paren(&f.expr), Expr::Reference(_));
                    if by_ref && self.prog.method(s, "__ref_into_iter").is_some() {
                        Some(true)
                    } else if !by_ref && self.prog.method(s, "into_iter").is_some() {
                        Some(false)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            let via: Option<Expr> = if has_next {
                None
            } else if let Some(by_ref) = user_into {
                let fe = &f.expr;
                Some(if by_ref {
                    syn::parse_quote!((#fe).__ref_into_iter())
                } else {
                    syn::parse_quote!((#fe).into_iter())
                })
            } else {
                let fe = &f.expr;
                Some(match strip_paren(fe) {
                    Expr::Reference(r) if r.mutability.is_some() => {
                        let inner = &r.expr;
                        syn::parse_quote!((#inner).iter_mut())
                    }
                    Expr::Reference(r) => {
                        let inner = &r.expr;
                        syn::parse_quote!((#inner).iter())
                    }
                    e => syn::parse_quote!((#e).into_iter()),
                })
            };
            let src_e: &Expr = via.as_ref().unwrap_or(&f.expr);
            self.scopes.push(vec![]);
            let (v, t) = self.expr(src_e, None)?;
            if t == Ty::Never {
                self.pop_scope();
                return Ok((None, Ty::Never));
            }
            self.bind("__exr_it".into(), t, v);
            let call: Expr = syn::parse_quote!(__exr_it.next());
            let fp = &f.pat;
            let pat: Pat = syn::parse_quote!(Some(#fp));
            let r = self.while_let_parts(&pat, &call, &f.body);
            self.pop_scope();
            r?;
            return Ok((None, Ty::Unit));
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
        // 値で回す (`for x in arr`): 要素は x にムーブするので、配列は drop しない。
        // Vec なら長さを 0 にしておき、バッファの解放だけを Vec の drop に任せる
        if mode.is_none() {
            let vec = match self.peek(base_e) {
                Some(Ty::Struct(sid)) => self.vec_fields(sid),
                _ => None,
            };
            if let (Some((_, lo, _)), true) = (vec, is_place_expr(base_e)) {
                if let Place::Mem(a, o, _) = self.place(base_e)? {
                    let z = self.b.ins().iconst(types::I64, 0);
                    self.b.ins().store(mf(), z, a, o + lo as i32);
                }
            } else if self.needs_drop(elem) {
                self.mark_moved(base_e);
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
        let bound = self.pending_bound.take();
        let nb = self.loop_bounds.len();
        if let (Some(h), Pat::Ident(pi)) = (bound, &*f.pat) {
            if pi.mutability.is_none() && pi.by_ref.is_none() {
                if let Some((l, _)) = self.lookup(&pi.ident.to_string()) {
                    self.loop_bounds.push((l, h));
                }
            }
        }
        self.loops.push(Loop {
            cont: step,
            brk: exit,
            breaks: 0,
            label: self.pending_label.take(),
            val: None,
            is_block: false,
            // パターンの束縛のスコープも break で drop する
            depth: self.scopes.len() - 1,
            temps: self.temps.len(),
        });
        let (_, bt) = self.block(&f.body, Some(Ty::Unit))?;
        self.loops.pop();
        self.loop_bounds.truncate(nb);
        self.pop_scope();
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
        let Some(lo_e) = &rg.start else {
            return err(rg.span(), "for loops need a range with a start");
        };
        // `for i in 0..`: 型の最大値までの閉区間 (事実上終わらない)
        if rg.end.is_none() && rev {
            return err(rg.span(), "cannot reverse an unbounded range");
        }
        let hint = self.peek(lo_e).or_else(|| rg.end.as_ref().and_then(|h| self.peek(h)));
        let (lo, lt) = self.expr(lo_e, hint)?;
        if !lt.is_int() {
            return err(rg.span(), "range bounds must be integers");
        }
        let lo = lo.unwrap();
        let (hi, inclusive) = match &rg.end {
            Some(hi_e) => {
                let (hi, ht) = self.expr(hi_e, Some(lt))?;
                self.same(hi_e.span(), ht, lt)?;
                (hi.unwrap(), matches!(rg.limits, syn::RangeLimits::Closed(_)))
            }
            None => (self.int_const(lt, crate::front::prim_assoc_const(lt, "MAX").unwrap()), true),
        };
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
        // (`for k in 0..3`: 下限が 0 以上・上限が定数なら、k はその上限より小さい)
        self.pending_bound = match (self.iconst_of(lo), self.iconst_of(hi)) {
            (Some(l), Some(h)) if l >= 0 && h >= 0 => Some(h as u64 + inclusive as u64),
            _ => None,
        };
        // -O: 回数が定数で小さい (4 回まで) ループは展開する (`for k in 0..3`: k が定数になり、添字がアドレスの定数に畳まれる)
        if self.prog.opt && !enumerate {
            if let (Some(l), Some(h)) = (self.iconst_of(lo), self.iconst_of(hi)) {
                let h = h + inclusive as i64;
                if l <= h && h - l <= 4 {
                    let bound = self.pending_bound;
                    let exit = self.b.create_block();
                    let vals: Vec<i64> = if rev { (l..h).rev().collect() } else { (l..h).collect() };
                    for v in vals {
                        let step = self.b.create_block();
                        let iv = self.b.ins().iconst(ct, v);
                        self.pending_bound = bound;
                        self.for_body(f, Some(iv), lt, step, exit)?;
                        self.b.switch_to_block(step);
                    }
                    self.b.ins().jump(exit, &[]);
                    self.b.switch_to_block(exit);
                    return Ok((None, Ty::Unit));
                }
            }
        }
        let i = self.b.declare_var(ct);
        // (enumerate の番号。enumerate でなければ数えない)
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
        if enumerate {
            let nv = self.b.use_var(n);
            let nn = self.b.ins().iadd_imm_s(nv, 1);
            self.b.def_var(n, nn);
        }
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

    /// cond が立っていれば、算術の検査の panic (kind: 0 足し算 1 引き算 2 掛け算 3 0 除算 4 0 の剰余
    /// 5 (未使用) 6 符号反転 7 左シフト 8 右シフト 9 割り算のあふれ 10 剰余のあふれ)
    fn ovf_panic(&mut self, cond: Value, kind: u8, span: proc_macro2::Span) {
        let _ = (kind, span);
        // std のプログラム: 条件つき trap (main が登録したハンドラが panic として 101 で終わらせる)。
        // 分岐とブロックを増やさないので、検査のコンパイルの重さは命令 1 つ分
        if self.prog.panic_handler.is_none() {
            let code = self.trap_site(kind, span, None);
            self.b.ins().trapnz(cond, code);
            return;
        }
        // (`#[panic_handler]` があればそれを呼ぶ)
        let cont = self.b.create_block();
        let pb = self.b.create_block();
        self.b.set_cold_block(pb);
        self.b.ins().brif(cond, pb, &[], cont, &[]);
        self.b.switch_to_block(pb);
        self.panic_call();
        self.b.switch_to_block(cont);
    }

    /// panic の trap のコードを 1 つ取り、場所 (種類・行・列) とメッセージを覚える (表は .o に置き、
    /// main が登録したハンドラが発生したアドレスで引く)。数が多すぎれば場所なしのコード 4
    /// span の (ファイルの中の行, 1 始まりの列)
    fn loc(&mut self, span: proc_macro2::Span) -> (u32, u32) {
        self.loc_used = true;
        let st = span.start();
        (st.line as u32 + self.line_base, st.column as u32 + 1)
    }

    fn trap_site(&mut self, kind: u8, span: proc_macro2::Span, msg: Option<String>) -> TrapCode {
        let k = self.traps.len();
        if k >= TRAP_SITE_MAX {
            return TrapCode::unwrap_user(4);
        }
        let info = if self.caller_loc {
            (kind as u32) << 27
        } else {
            let (l, c) = self.loc(span);
            ((kind as u32) << 27) | (l.min(0xFFFF) << 11) | c.min(0x7FF)
        };
        let m = match msg {
            Some(m) => {
                let bytes = m.into_bytes();
                match self.datas.iter().position(|d| *d == bytes) {
                    Some(i) => i as u32,
                    None => {
                        self.datas.push(bytes);
                        self.datas.len() as u32 - 1
                    }
                }
            }
            None => u32::MAX,
        };
        self.traps.push((info, m));
        TrapCode::unwrap_user(TRAP_SITE_BASE + k as u8)
    }

    /// 値が整数の定数ならその値 (`x % 4` の 0 除算の検査を省く)
    fn iconst_of(&self, v: Value) -> Option<i64> {
        let ir::ValueDef::Result(inst, _) = self.b.func.dfg.value_def(v) else { return None };
        match self.b.func.dfg.insts[inst] {
            ir::InstructionData::UnaryImm { opcode: ir::Opcode::Iconst, imm } => Some(imm.bits()),
            _ => None,
        }
    }

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

    /// `vec![a, b, c]` / `vec![x; n]`: Vec::with_capacity と push に展開する
    fn vec_macro(&mut self, mac: &syn::Macro, expected: Option<Ty>) -> R<V> {
        use syn::parse::Parser;
        let span = mac.span();
        let mut elem = match expected {
            Some(Ty::Struct(sid)) => self.vec_fields(sid).map(|f| f.2),
            _ => None,
        };
        let repeat = (|input: syn::parse::ParseStream| {
            let x: Expr = input.parse()?;
            let _: syn::Token![;] = input.parse()?;
            let n: Expr = input.parse()?;
            Ok((x, n))
        })
        .parse2(mac.tokens.clone());
        if let Ok((x, n)) = repeat {
            let (xv, xt) = self.expr(&x, elem)?;
            let (nv, nt) = self.expr(&n, Some(Ty::Usize))?;
            self.same(n.span(), nt, Ty::Usize)?;
            let nv = nv.unwrap();
            // (drop の要る要素は clone して並べる: `Vec::new()` + `resize(n, x)`)
            if self.needs_drop(xt) {
                let Some(xv) = xv else { return Ok((None, Ty::Never)) };
                if !self.prog.index.contains_key("Vec::resize") {
                    return err(x.span(), "`vec![x; n]` needs a `Copy` element");
                }
                let (v, vt) = self.call_named("Vec::new", vec![xt], &[], span)?;
                let v = v.unwrap();
                let vr = if vt.is_agg() { v } else { unreachable!() };
                self.call_named("Vec::resize", vec![xt], &[(vr, Ty::Ref(true, intern(vt))), (nv, Ty::Usize), (xv, xt)], span)?;
                return Ok((Some(v), vt));
            }
            let (v, vt) = self.call_named("Vec::with_capacity", vec![xt], &[(nv, Ty::Usize)], span)?;
            let v = v.unwrap();
            // for i in 0..n { v.push(x) }
            let i = self.b.declare_var(types::I64);
            let z = self.b.ins().iconst(types::I64, 0);
            self.b.def_var(i, z);
            let head = self.b.create_block();
            let body = self.b.create_block();
            let done = self.b.create_block();
            self.b.ins().jump(head, &[]);
            self.b.switch_to_block(head);
            let iv = self.b.use_var(i);
            let more = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, nv);
            self.b.ins().brif(more, body, &[], done, &[]);
            self.b.switch_to_block(body);
            if let Some(xv) = xv {
                self.call_named("Vec::push", vec![xt], &[(v, Ty::Ref(true, intern(vt))), (xv, xt)], span)?;
            }
            let next = self.b.ins().iadd_imm_s(iv, 1);
            self.b.def_var(i, next);
            self.b.ins().jump(head, &[]);
            self.b.switch_to_block(done);
            return Ok((Some(v), vt));
        }
        let args = self.macro_args(mac)?;
        let mut vals = vec![];
        for a in &args {
            let (v, t) = self.expr(a, elem)?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            match elem {
                Some(e) => self.same(a.span(), t, e)?,
                None => elem = Some(t),
            }
            vals.push(v);
        }
        let Some(e) = elem else {
            return err(span, "type annotations needed for an empty `vec![]`");
        };
        let n = self.b.ins().iconst(types::I64, vals.len() as i64);
        let (v, vt) = self.call_named("Vec::with_capacity", vec![e], &[(n, Ty::Usize)], span)?;
        let v = v.unwrap();
        for x in vals.into_iter().flatten() {
            self.call_named("Vec::push", vec![e], &[(v, Ty::Ref(true, intern(vt))), (x, e)], span)?;
        }
        Ok((Some(v), vt))
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
            "vec" => self.vec_macro(mac, expected),
            "format" | "print" | "println" | "eprint" | "eprintln" | "write" | "writeln" => {
                self.fmt_macro(mac, &name)
            }
            // `dbg!(e)`: `[file:line:col] e = {:#?}` を stderr に出して e を返す
            "dbg" => {
                let args = self.macro_args(mac)?;
                let Some(e) = args.first() else {
                    return err(mac.span(), "`dbg!()` without arguments is not supported");
                };
                let (sl, sc) = self.loc(e.span());
                let text = {
                    use quote::ToTokens;
                    e.to_token_stream().to_string()
                };
                let head = format!("[src/main.rs:{sl}:{sc}] {} = ", text);
                let ex: Expr = syn::parse_quote!({
                    let __exr_dbg = #e;
                    eprintln!("{}{:#?}", #head, __exr_dbg);
                    __exr_dbg
                });
                self.expr(&ex, expected)
            }
            "panic" | "unreachable" | "todo" | "unimplemented" => {
                if self.prog.panic_handler.is_none() {
                    // メッセージ: 文字列だけならそのまま、書式つきなら書式化してから prelude の関数で panic する
                    let args = self.macro_args(mac).unwrap_or_default();
                    let head = match name.as_str() {
                        "unreachable" => "internal error: entered unreachable code",
                        "todo" => "not yet implemented",
                        "unimplemented" => "not implemented",
                        _ => "",
                    };
                    let lit = match args.first() {
                        Some(Expr::Lit(syn::ExprLit { lit: Lit::Str(s), .. })) if args.len() == 1 && !s.value().contains('{') => Some(s.value()),
                        _ => None,
                    };
                    // (書式化の部品がない (書式化を使わない) プログラムでは、書式の文字列の固定の部分だけを出す)
                    let lit = if lit.is_none() && !self.prog.index.contains_key("__exr_panic_fmt") {
                        match args.first() {
                            Some(Expr::Lit(syn::ExprLit { lit: Lit::Str(s), .. })) => {
                                let v = s.value();
                                let fixed = v.split('{').next().unwrap_or("").trim_end_matches(": ").to_string();
                                if fixed.is_empty() { None } else { Some(fixed) }
                            }
                            _ => None,
                        }
                    } else {
                        lit
                    };
                    if !args.is_empty() && lit.is_none() {
                        if self.prog.index.contains_key("__exr_panic_fmt") {
                            let toks = &mac.tokens;
                            let e: Expr = syn::parse_quote!(format!(#toks));
                            let (s, st) = self.expr(&e, None)?;
                            if let Some(s) = s {
                                let pre = if head.is_empty() { String::new() } else { format!("{head}: ") };
                                let pv = self.str_const(&pre);
                                let (sl, sc) = if self.caller_loc { (0, 0) } else { let (l, c) = self.loc(mac.span()); (l as i64, c as i64) };
                                let l = self.b.ins().iconst(types::I32, sl);
                                let c = self.b.ins().iconst(types::I32, sc);
                                let sref = Ty::Ref(false, intern(Ty::Str));
                                self.call_named("__exr_panic_fmt", vec![Ty::Unit], &[(pv, sref), (s, Ty::Ref(false, intern(st))), (l, Ty::U32), (c, Ty::U32)], mac.span())?;
                            }
                            self.b.ins().trap(TrapCode::unwrap_user(3));
                            return Ok(self.dead());
                        }
                    }
                    let msg = match (lit, head) {
                        (Some(m), "") => m,
                        (Some(m), h) => format!("{h}: {m}"),
                        (None, "") => "explicit panic".to_string(),
                        (None, h) => h.to_string(),
                    };
                    let code = self.trap_site(12, mac.span(), Some(msg));
                    self.b.ins().trap(code);
                    return Ok(self.dead());
                }
                Ok(self.panic_call())
            }
            "assert" | "debug_assert" => {
                let args = self.macro_args(mac)?;
                let Some(c) = args.first() else {
                    return err(mac.span(), "assert! needs a condition");
                };
                let (cv, ct) = self.expr(c, Some(Ty::Bool))?;
                self.same(c.span(), ct, Ty::Bool)?;
                if let Some(cv) = cv {
                    let text = {
                        use quote::ToTokens;
                        c.to_token_stream().to_string()
                    };
                    let msg = match args.get(1) {
                        Some(Expr::Lit(syn::ExprLit { lit: Lit::Str(s), .. })) if args.len() == 2 => s.value(),
                        _ => format!("assertion failed: {text}"),
                    };
                    self.panic_unless_at(cv, 12, mac.span(), Some(msg));
                }
                Ok((None, Ty::Unit))
            }
            "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne" => {
                let args = self.macro_args(mac)?;
                if args.len() < 2 {
                    return err(mac.span(), format!("{name}! needs two arguments"));
                }
                // `a == b` / `a != b` として比べる (構造的な比較・手書きの eq・String と &str も)
                let (a, b) = (&args[0], &args[1]);
                // 書式化が使えれば、rustc と同じく left / right の値 ({:?}) も出す (a と b は 1 回だけ評価する)
                if self.prog.panic_handler.is_none() && self.prog.index.contains_key("__exr_panic_fmt") {
                    let op = if name.ends_with("_eq") { "==" } else { "!=" };
                    let (l, c) = if self.caller_loc { (0u32, 0u32) } else { self.loc(mac.span()) };
                    let cmp: Expr = if name.ends_with("_eq") {
                        syn::parse_quote!(*__exr_l == *__exr_r)
                    } else {
                        syn::parse_quote!(*__exr_l != *__exr_r)
                    };
                    let fail: Expr = if args.len() > 2 {
                        let fmt = format!("assertion `left {op} right` failed: {{}}\n  left: {{:?}}\n right: {{:?}}");
                        let rest = &args[2..];
                        syn::parse_quote!({
                            let __exr_m = format!(#(#rest),*);
                            __exr_panic_fmt::<()>("", &format!(#fmt, __exr_m, __exr_l, __exr_r), #l, #c);
                        })
                    } else {
                        let fmt = format!("assertion `left {op} right` failed\n  left: {{:?}}\n right: {{:?}}");
                        syn::parse_quote!({
                            __exr_panic_fmt::<()>("", &format!(#fmt, __exr_l, __exr_r), #l, #c);
                        })
                    };
                    // (左が数のリテラルで右がそうでなければ、右を先に評価して左の型を右から決める (`assert_eq!(2, n)`))
                    let num_lit = |e: &Expr| matches!(e, Expr::Lit(syn::ExprLit { lit: Lit::Int(_) | Lit::Float(_), .. }));
                    let e: Expr = if num_lit(a) && !num_lit(b) {
                        syn::parse_quote!({
                            let __exr_r = &(#b);
                            let __exr_l = &(#a);
                            if !(#cmp) #fail
                        })
                    } else {
                        syn::parse_quote!({
                            let __exr_l = &(#a);
                            let __exr_r = &(#b);
                            if !(#cmp) #fail
                        })
                    };
                    self.expr(&e, Some(Ty::Unit))?;
                    return Ok((None, Ty::Unit));
                }
                let e: Expr = if name.ends_with("_eq") {
                    syn::parse_quote!((#a) == (#b))
                } else {
                    syn::parse_quote!((#a) != (#b))
                };
                let (c, ct) = self.expr(&e, Some(Ty::Bool))?;
                self.same(mac.span(), ct, Ty::Bool)?;
                if let Some(c) = c {
                    let op = if name.ends_with("_eq") { "==" } else { "!=" };
                    self.panic_unless_at(c, 12, mac.span(), Some(format!("assertion `left {op} right` failed")));
                }
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
                self.pop_scope();
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

    /// 添字の検査: 範囲外なら `index out of bounds: the len is L but the index is I` で panic する。
    /// 値をメッセージに入れるので trap でなく分岐して prelude の関数を呼ぶ (呼ぶブロックは関数で 1 つを共有し、
    /// 長さ・添字・場所はブロック引数で渡す)
    fn index_check(&mut self, ok: Value, len: Value, i: Value, span: proc_macro2::Span) {
        if self.prog.panic_handler.is_some() || !self.prog.index.contains_key("__exr_panic_index") {
            self.panic_unless_at(ok, 12, span, Some("index out of bounds".into()));
            return;
        }
        // (同じブロックの中で、同じ添字と長さの検査はもう通っている)
        let key = match self.iconst_of(len) {
            Some(n) => LenKey::Const(n),
            None => LenKey::Val(len),
        };
        let cur = self.b.current_block();
        if self.checked.0 != cur {
            self.checked = (cur, vec![]);
        }
        if self.checked.1.contains(&(i, key)) {
            return;
        }
        // -O: 場所ごとに冷たいブロックを作り、場所の定数はそこで作る (熱い道で定数をレジスタに置かない)
        if self.prog.opt {
            let pb = self.b.create_block();
            self.b.set_cold_block(pb);
            let cont = self.b.create_block();
            self.b.ins().brif(ok, cont, &[], pb, &[]);
            self.b.switch_to_block(pb);
            let (sl, sc) = self.loc(span);
            let ln = self.b.ins().iconst(types::I32, sl as i64);
            let cl = self.b.ins().iconst(types::I32, sc as i64);
            let _ = self.call_named(
                "__exr_panic_index",
                vec![Ty::Unit],
                &[(len, Ty::Usize), (i, Ty::Usize), (ln, Ty::U32), (cl, Ty::U32)],
                span,
            );
            self.b.ins().trap(TrapCode::unwrap_user(3));
            self.b.switch_to_block(cont);
            let prev = std::mem::take(&mut self.checked.1);
            self.checked = (Some(cont), prev);
            self.checked.1.push((i, key));
            return;
        }
        let pb = match self.index_panic_block {
            Some(b) => b,
            None => {
                let pb = self.b.create_block();
                self.b.set_cold_block(pb);
                let l = self.b.append_block_param(pb, types::I64);
                let x = self.b.append_block_param(pb, types::I64);
                let ln = self.b.append_block_param(pb, types::I32);
                let cl = self.b.append_block_param(pb, types::I32);
                let cur = self.b.current_block().unwrap();
                self.b.switch_to_block(pb);
                let _ = self.call_named(
                    "__exr_panic_index",
                    vec![Ty::Unit],
                    &[(l, Ty::Usize), (x, Ty::Usize), (ln, Ty::U32), (cl, Ty::U32)],
                    span,
                );
                self.b.ins().trap(TrapCode::unwrap_user(3));
                self.b.switch_to_block(cur);
                self.index_panic_block = Some(pb);
                pb
            }
        };
        let (sl, sc) = self.loc(span);
        let ln = self.b.ins().iconst(types::I32, sl as i64);
        let cl = self.b.ins().iconst(types::I32, sc as i64);
        let cont = self.b.create_block();
        self.b.ins().brif(ok, cont, &[], pb, &[len.into(), i.into(), ln.into(), cl.into()]);
        self.b.switch_to_block(cont);
        let prev = std::mem::take(&mut self.checked.1);
        self.checked = (Some(cont), prev);
        self.checked.1.push((i, key));
    }

    /// cond が偽なら panic する (kind と msg は std のプログラムでの panic のメッセージ)
    fn panic_unless_at(&mut self, cond: Value, kind: u8, span: proc_macro2::Span, msg: Option<String>) {
        if self.prog.panic_handler.is_none() {
            let code = self.trap_site(kind, span, msg);
            self.b.ins().trapz(cond, code);
            return;
        }
        self.panic_unless(cond);
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
        // `?` は中身 (Ok / Some の値か、Err の値) をムーブするので、一時値 (や変数) はもう drop しない
        let sflag = self.last_scrut;
        self.release(&t.expr, sflag);
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
            let (ev, _) = self.read(Self::field_place(place, ety, eoff));
            // エラーの型が違えば `From::from` で変換する (`impl From<ParseIntError> for AppError`)
            let ev = if ety != rety {
                let conv = match rety {
                    Ty::Struct(rs) => self.prog.method(rs, "from").filter(|&id| {
                        let sg = &self.prog.sigs[id as usize];
                        !sg.has_self && sg.params.len() == 1 && (sg.params[0] == ety || self.prog.has_param(sg.params[0]))
                    }),
                    _ => None,
                };
                let dyn_err = matches!(rety, Ty::Struct(rs) if self.prog.structs[rs as usize].name == "__DynError");
                match (conv, ev) {
                    (Some(id), Some(v)) => self.call_fn_vals(id, &[(v, ety)], t.span())?.0,
                    // `Box<dyn Error>` へは表示の文字列にして入れる
                    (None, Some(v)) if dyn_err => self.call_named("__exr_dyn_error", vec![ety], &[(v, ety)], t.span())?.0,
                    _ => {
                        self.same(t.span(), ety, rety)?;
                        ev
                    }
                }
            } else {
                ev
            };
            self.write(Place::Mem(a, reoff as i32, rety), ev);
        }
        self.drop_for_exit(0, 0);
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
        // `Box::new(x)`: ヒープに置いて、所有する `&mut T` として返す (解放はしない)
        let n = p.path.segments.len();
        if name == "new"
            && n >= 2
            && p.path.segments[n - 2].ident == "Box"
            && !self.prog.struct_index.contains_key("Box")
        {
            if c.args.len() != 1 {
                return err(c.span(), "`Box::new` takes 1 argument");
            }
            // `Box::new(MyErr(..))` を `Box<dyn Error>` として: 表示の文字列にする
            if let Some(Ty::Struct(sid)) = expected {
                if self.prog.structs[sid as usize].name == "__DynError" {
                    let (v, t) = self.expr(&c.args[0], None)?;
                    let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
                    return self.call_named("__exr_dyn_error", vec![t], &[(v, t)], c.span()).map(Some);
                }
            }
            let inner = match expected {
                Some(Ty::Box(t)) if !matches!(t, Ty::Dyn(_) | Ty::Slice(_) | Ty::Str) => Some(*t),
                _ => None,
            };
            let (v, t) = self.expr(&c.args[0], inner)?;
            if t == Ty::Never {
                return Ok(Some((None, Ty::Never)));
            }
            let Some(&alloc) = self.prog.index.get(crate::prelude::BOX_ALLOC) else {
                return err(c.span(), "internal: the allocator is missing from the prelude");
            };
            let (size, _) = self.prog.size_align(t);
            let asig = self.prog.sigs[alloc as usize].clone();
            let fr = self.fref(alloc, &asig);
            let sz = self.b.ins().iconst(types::I64, size.max(1) as i64);
            let call = self.b.ins().call(fr, &[sz]);
            let ptr = self.b.inst_results(call)[0];
            self.write(Place::Mem(ptr, 0, t), v);
            return Ok(Some((Some(ptr), Ty::Box(intern(t)))));
        }
        // prelude の Chars が使う u32 → char (検査しない)
        if name == "__exr_char_from_u32" {
            let (v, t) = self.expr(&c.args[0], Some(Ty::U32))?;
            self.same(c.args[0].span(), t, Ty::U32)?;
            return Ok(Some((v, Ty::Char)));
        }
        // `core::slice::from_raw_parts(p, n)` / `_mut` と `core::str::from_utf8_unchecked(b)`
        if matches!(name.as_str(), "from_raw_parts" | "from_raw_parts_mut" | "from_utf8_unchecked")
            && !(p.path.segments.len() == 1 && self.prog.index.contains_key(&name))
        {
            if name == "from_utf8_unchecked" {
                let u8s = Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8))));
                let (v, t) = self.expr(&c.args[0], Some(u8s))?;
                self.same(c.args[0].span(), t, u8s)?;
                return Ok(Some((v, Ty::Ref(false, intern(Ty::Str)))));
            }
            if c.args.len() != 2 {
                return err(c.span(), format!("`{name}` takes 2 arguments"));
            }
            let (pv, pt) = self.expr(&c.args[0], None)?;
            let Ty::Ptr(_, e) = pt else {
                return err(c.args[0].span(), "expected a raw pointer");
            };
            let (n, nt) = self.expr(&c.args[1], Some(Ty::Usize))?;
            self.same(c.args[1].span(), nt, Ty::Usize)?;
            let fat = self.make_fat_v(pv.unwrap(), n.unwrap());
            let m = name == "from_raw_parts_mut";
            return Ok(Some((Some(fat), Ty::Ref(m, intern(Ty::Slice(e))))));
        }
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
    /// 値として使われた関数のパス (`apply(double, 3)` の `double`、`S::f`) ならその関数
    fn fn_item(&self, p: &syn::ExprPath) -> Option<u32> {
        if p.qself.is_some() {
            return None;
        }
        if let Some(id) = p.path.get_ident() {
            let n = id.to_string();
            if self.lookup(&n).is_some() || self.prog.const_index.contains_key(&n) {
                return None;
            }
        }
        let id = self.prog.fn_by_path(&p.path, self.self_sid)?;
        let s = &self.prog.sigs[id as usize];
        (!s.is_main && s.has_body || s.is_extern).then_some(id)
    }

    /// 関数アイテムを関数ポインタの値にする。ジェネリックなら turbofish か期待型 (`fn(i64) -> i64`) で型引数を決める
    fn fn_value(&mut self, p: &syn::ExprPath, id: u32, expected: Option<Ty>) -> R<V> {
        let sig0 = self.prog.sigs[id as usize].clone();
        let tf = match &p.path.segments.last().unwrap().arguments {
            syn::PathArguments::AngleBracketed(a) => Some(a),
            _ => None,
        };
        let mut sol = self.turbofish_sol(&sig0, tf)?;
        if let Some(Ty::Fn(f)) = expected {
            for (&a, &b) in sig0.params.iter().zip(&f.params) {
                self.prog.match_ty(a, b, &mut sol);
            }
            self.prog.match_ty(sig0.ret, f.ret, &mut sol);
        }
        for (i, g) in sig0.generics.iter().enumerate() {
            if g.0 == "G" && sol[i].is_none() {
                sol[i] = Some(Ty::Unit);
            }
        }
        let (target, sig) = if sig0.is_generic() {
            if let Some(i) = sol.iter().position(|s| s.is_none()) {
                return err(
                    p.span(),
                    format!(
                        "cannot infer type parameter `{}`; use turbofish (`f::<T>`) or annotate the fn pointer type",
                        sig0.generics[i].0
                    ),
                );
            }
            let ats: Vec<Ty> = sig0.params.iter().map(|&t| self.prog.partial_subst(t, &sol)).collect();
            self.resolve_call(id, &ats, sol, None, p.span())?
        } else {
            (id, sig0)
        };
        let f = self.fref(target, &sig);
        let v = self.b.ins().func_addr(PTR, f);
        let t = Ty::Fn(crate::front::intern_fn(crate::front::FnTy {
            params: sig.params.clone(),
            ret: sig.ret,
        }));
        Ok((Some(v), t))
    }

    fn path_value(&mut self, p: &syn::ExprPath, _expected: Option<Ty>) -> R<V> {
        if let Some(ci) = self.const_of(p) {
            return self.const_value(p, ci);
        }
        if let Some(id) = self.fn_item(p) {
            return self.fn_value(p, id, _expected);
        }
        let segs: Vec<String> = p
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        // `f64::MAX` / `f64::consts::PI` / `std::f64::consts::E` など
        let fl = segs.iter().position(|s| s == "f64" || s == "f32");
        if let Some(i) = fl {
            let t = if segs[i] == "f32" { Ty::F32 } else { Ty::F64 };
            let name = segs.last().unwrap().as_str();
            let x: Option<f64> = match name {
                "MAX" => Some(if t == Ty::F32 { f32::MAX as f64 } else { f64::MAX }),
                "MIN" => Some(if t == Ty::F32 { f32::MIN as f64 } else { f64::MIN }),
                "MIN_POSITIVE" => Some(if t == Ty::F32 { f32::MIN_POSITIVE as f64 } else { f64::MIN_POSITIVE }),
                "EPSILON" => Some(if t == Ty::F32 { f32::EPSILON as f64 } else { f64::EPSILON }),
                "INFINITY" => Some(f64::INFINITY),
                "NEG_INFINITY" => Some(f64::NEG_INFINITY),
                "NAN" => Some(f64::NAN),
                "PI" => Some(std::f64::consts::PI),
                "E" => Some(std::f64::consts::E),
                "TAU" => Some(std::f64::consts::TAU),
                "SQRT_2" => Some(std::f64::consts::SQRT_2),
                "LN_2" => Some(std::f64::consts::LN_2),
                "LN_10" => Some(std::f64::consts::LN_10),
                "FRAC_PI_2" => Some(std::f64::consts::FRAC_PI_2),
                "FRAC_PI_4" => Some(std::f64::consts::FRAC_PI_4),
                "FRAC_1_SQRT_2" => Some(std::f64::consts::FRAC_1_SQRT_2),
                _ => None,
            };
            if let Some(x) = x {
                let v = if t == Ty::F32 { self.b.ins().f32const(x as f32) } else { self.b.ins().f64const(x) };
                return Ok((Some(v), t));
            }
        }
        if segs.len() == 2 {
            if let Some(t) = crate::front::Ty::from_suffix(&segs[0]).or(if segs[0] == "char" {
                Some(Ty::Char)
            } else {
                None
            }) {
                if let Some(v) = crate::front::prim_assoc_const(t, &segs[1]) {
                    let t = if segs[1] == "BITS" { Ty::U32 } else { t };
                    let c = self.int_const(t, v);
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
    /// `N` (const ジェネリクス) の値。ローカル変数が同じ名前なら変数
    fn const_generic(&self, p: &syn::ExprPath) -> Option<u32> {
        let id = p.path.get_ident()?.to_string();
        if self.lookup(&id).is_some() {
            return None;
        }
        crate::front::const_generic_value(&self.env, &id)
    }

    fn const_of(&self, p: &syn::ExprPath) -> Option<u32> {
        // (速い判定: 複数セグメントのパスで、最後の名前の const がなければ const ではない)
        if p.path.segments.len() >= 2 {
            let last = p.path.segments.last()?.ident.to_string();
            if !self.prog.const_by_last.contains_key(&last) && !self.prog.const_index.contains_key(&last) {
                return None;
            }
        }
        // `geometry::UNIT` / `crate::m::N`: モジュールの部分を外して引く
        if p.qself.is_none() && p.path.segments.len() >= 2 {
            if let Some(q) = self.prog.strip_mods(&p.path) {
                let qe = syn::ExprPath { attrs: vec![], qself: None, path: q };
                if let Some(ci) = self.const_of_inner(&qe) {
                    return Some(ci);
                }
            }
        }
        self.const_of_inner(p)
    }

    fn const_of_inner(&self, p: &syn::ExprPath) -> Option<u32> {
        // 関連定数 `S::N` / `Self::N` / `T::N` (T は型引数) / trait の既定値 `Tr::N`
        if p.qself.is_none() && p.path.segments.len() == 2 {
            let (a, n) = (p.path.segments[0].ident.to_string(), p.path.segments[1].ident.to_string());
            let ty = if a == "Self" {
                self.env.self_ty.or(self.self_sid.map(Ty::Struct))
            } else if self.env.params.iter().any(|(g, _)| *g == a) {
                let ty: syn::Type = syn::Type::Path(syn::TypePath { qself: None, path: syn::Path::from(p.path.segments[0].ident.clone()) });
                self.prog.parse_ty(&ty, &self.env).ok()
            } else {
                None
            };
            let base = match ty {
                Some(Ty::Struct(sid)) => Some(self.prog.structs[self.prog.base_of(sid) as usize].name.clone()),
                Some(t) => Some(self.prog.tn(t)),
                None => Some(a.clone()),
            }?;
            if let Some(&ci) = self.prog.const_index.get(&format!("{base}::{n}")) {
                return Some(ci);
            }
            // trait の既定値: 型が実装している trait の `Tr::N`
            if let Some(Ty::Struct(sid)) = ty {
                let b = self.prog.base_of(sid);
                for (s2, tr) in &self.prog.impls {
                    if *s2 == b {
                        if let Some(&ci) = self.prog.const_index.get(&format!("{tr}::{n}")) {
                            return Some(ci);
                        }
                    }
                }
            }
            return None;
        }
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
        self.scrutinee_as(e, None)
    }

    /// `let (a, b): (Vec<i32>, Vec<i32>) = ..`: 型注釈を期待型として渡す
    fn scrutinee_as(&mut self, e: &Expr, expected: Option<Ty>) -> R<Option<Place>> {
        self.last_scrut = None;
        // `v[..]` / `a[1..]` (サイズ不定の [T] の場所): fat pointer の置き場所を `[T]` 型の place として渡す
        // (スライスパターンだけが受け付け、要素は値で束縛する)
        if let Expr::Index(ix) = strip_paren(e) {
            if matches!(&*ix.index, Expr::Range(_)) {
                let r: Expr = syn::parse_quote!(&#e);
                let (v, t) = self.expr(&r, None)?;
                let (Some(v), Ty::Ref(_, Ty::Slice(el))) = (v, t) else {
                    return err(e.span(), "unsupported range scrutinee");
                };
                return Ok(Some(Place::Mem(v, 0, Ty::Slice(el))));
            }
        }
        if is_place_expr(e) {
            return Ok(Some(self.place(e)?));
        }
        let (v, t) = self.expr(e, expected)?;
        // 一時値の match 対象は文の終わりまで所有する (パターンがムーブしたらフラグを下ろす)
        if let Some(v) = v {
            if t != Ty::Never {
                self.last_scrut = self.own_temp(v, t);
            }
        }
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
        let sflag = self.last_scrut;
        let merge = self.b.create_block();
        let mut result_ty: Option<Ty> = None;
        let mut param: Option<Value> = None;
        // 各 arm を上から順に試す (判定 → 束縛 → ガード → 本体)
        for arm in &m.arms {
            let next = self.b.create_block();
            self.pat_test(&arm.pat, place, next)?;
            self.scopes.push(vec![]);
            let ob = self.owned_binds;
            self.pat_bind(&arm.pat, place, BindMode::Move)?;
            if self.owned_binds > ob {
                self.release(&m.expr, sflag);
            }
            if let Some((_, g)) = &arm.guard {
                let (c, ct) = self.expr(g, Some(Ty::Bool))?;
                self.same(g.span(), ct, Ty::Bool)?;
                if let Some(c) = c {
                    self.require(c, next);
                }
            }
            let t0 = self.temps.len();
            let (v, t) = self.expr(&arm.body, result_ty.or(expected))?;
            self.drop_tail_temps(t0, (v, t));
            self.pop_scope();
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
        let sflag = self.last_scrut;
        let else_blk = self.b.create_block();
        let merge = self.b.create_block();
        let mut result_ty: Option<Ty> = None;
        let mut param: Option<Value> = None;
        self.pat_test(&l.pat, place, else_blk)?;
        self.scopes.push(vec![]);
        let ob = self.owned_binds;
        self.pat_bind(&l.pat, place, BindMode::Move)?;
        if self.owned_binds > ob {
            self.release(&l.expr, sflag);
        }
        let (tv, tt) = self.block(&i.then_branch, expected)?;
        self.pop_scope();
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
        self.pending_label = w.label.as_ref().map(|l| l.name.ident.to_string());
        let Expr::Let(l) = &*w.cond else {
            unreachable!()
        };
        self.while_let_parts(&l.pat, &l.expr, &w.body)
    }

    /// `while let pat = e { body }` (for ループのイテレータ版からも使う)
    fn while_let_parts(&mut self, pat: &Pat, e: &Expr, body: &syn::Block) -> R<V> {
        let header = self.b.create_block();
        let exit = self.b.create_block();
        self.b.ins().jump(header, &[]);
        self.b.switch_to_block(header);
        // 毎回 match 対象を評価し直す (一時値は 1 回ごとに drop する)
        let t0 = self.temps.len();
        let Some(place) = self.scrutinee(e)? else {
            return Ok((None, Ty::Never));
        };
        let sflag = self.last_scrut;
        self.pat_test(pat, place, exit)?;
        self.scopes.push(vec![]);
        let ob = self.owned_binds;
        self.pat_bind(pat, place, BindMode::Move)?;
        if self.owned_binds > ob {
            self.release(e, sflag);
        }
        self.loops.push(Loop {
            cont: header,
            brk: exit,
            breaks: 0,
            label: self.pending_label.take(),
            val: None,
            is_block: false,
            depth: self.scopes.len() - 1,
            temps: t0,
        });
        let (_, bt) = self.block(body, Some(Ty::Unit))?;
        self.loops.pop();
        self.pop_scope();
        if bt != Ty::Never {
            self.same(body.span(), bt, Ty::Unit)?;
            self.drop_for_exit(usize::MAX, t0);
            self.b.ins().jump(header, &[]);
        } else {
            self.b.ins().trap(TrapCode::unwrap_user(1));
        }
        self.b.switch_to_block(exit);
        self.drop_temps(t0);
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
        // (`i32::MIN..=-1` や const の名前は式として評価する)
        let Expr::Lit(l) = e else {
            let (v, vt) = self.expr(e, Some(t))?;
            self.same(e.span(), vt, t)?;
            return v.ok_or_else(|| "range pattern bound has no value".to_string());
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
            Pat::Slice(sp) => {
                let (base, len, elem, _) = self.slice_scrutinee(p, sp.span())?;
                let (pre, rest, post) = slice_parts(sp);
                let fixed = (pre.len() + post.len()) as i64;
                let c = if rest.is_some() {
                    self.b.ins().icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, len, fixed)
                } else {
                    self.b.ins().icmp_imm_u(IntCC::Equal, len, fixed)
                };
                self.require(c, fail);
                for (q, sub) in self.slice_elems(base, len, elem, &pre, &post) {
                    self.pat_test(sub, q, fail)?;
                }
                Ok(())
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
            // 文字列のパターン (`match s { "add" => .. }`): &str (と &&str) の中身で比べる
            Pat::Lit(l) if matches!(l.lit, Lit::Str(_)) => {
                let sref = Ty::Ref(false, intern(Ty::Str));
                let mut p = p;
                while let Ty::Ref(_, i) = p.ty() {
                    if p.ty() == sref || *i == Ty::Str {
                        break;
                    }
                    let (v, _) = self.read(p);
                    p = Place::Mem(v.unwrap(), 0, *i);
                }
                if !matches!(p.ty(), Ty::Ref(_, Ty::Str)) {
                    return err(l.span(), format!("mismatched types: expected `{}`, found `&str`", self.prog.tn(p.ty())));
                }
                let (v, _) = self.read(p);
                let (c, _) = self.lit(&l.lit, Some(sref))?;
                let cond = self.eq_at(v.unwrap(), c.unwrap(), sref, l.span())?;
                self.require(cond, fail);
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
                        if !(t.is_int() || t == Ty::Bool || t == Ty::Char) {
                            return err(l.span(), "literal patterns need an integer, char, bool or string");
                        }
                        let cond = self.b.ins().icmp(IntCC::Equal, v.unwrap(), c.unwrap());
                        self.require(cond, fail);
                        Ok(())
                    }
                    Pat::Range(r) => {
                        if !(t.is_int() || t == Ty::Char) {
                            return err(r.span(), "range patterns need an integer or char");
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
            // `()` のパターン (`Ok(())`)
            Ty::Unit if n_pat == 0 => Ok(vec![]),
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
                if !o.cases.iter().any(has_bindings) {
                    return Ok(());
                }
                // どの選択肢に合ったかを判定し直し、その選択肢で束縛した値を merge の引数で渡す
                let mut names = vec![];
                crate::closure::pat_idents(&o.cases[0], &mut names);
                let merge = self.b.create_block();
                let mut tys: Option<Vec<(String, Ty)>> = None;
                for case in &o.cases {
                    let next = self.b.create_block();
                    self.pat_test(case, p, next)?;
                    let o0 = self.owned.len();
                    let ob = self.owned_binds;
                    self.scopes.push(vec![]);
                    self.pat_bind(case, p, mode)?;
                    let mut vals = vec![];
                    let mut tl = vec![];
                    for n in &names {
                        if let Some((loc, t)) = self.lookup(n) {
                            if let Some(v) = self.loc_arg(loc, t) {
                                vals.push(v);
                            }
                            tl.push((n.clone(), t));
                        }
                    }
                    self.scopes.pop();
                    self.owned.truncate(o0);
                    self.owned_binds = ob;
                    match &tys {
                        None => {
                            for (_, t) in &tl {
                                if let Some(ct) = self.param_ty(*t) {
                                    self.b.append_block_param(merge, ct);
                                }
                            }
                            tys = Some(tl);
                        }
                        Some(t0) => {
                            if t0.iter().map(|x| x.1).ne(tl.iter().map(|x| x.1)) {
                                return err(case.span(), "`|` alternatives bind different types");
                            }
                        }
                    }
                    let args: Vec<_> = vals.into_iter().map(|v| v.into()).collect();
                    self.b.ins().jump(merge, &args);
                    self.b.switch_to_block(next);
                }
                self.b.ins().trap(TrapCode::unwrap_user(2));
                self.b.switch_to_block(merge);
                let params = self.b.block_params(merge).to_vec();
                let mut pi = params.into_iter();
                for (n, t) in tys.unwrap_or_default() {
                    let v = if self.param_ty(t).is_some() { pi.next() } else { None };
                    self.bind(n, t, v);
                }
                Ok(())
            }
            Pat::Slice(sp) => {
                let (base, len, elem, rmode) = self.slice_scrutinee(p, sp.span())?;
                let mode = rmode.map(BindMode::Ref).unwrap_or(mode);
                let (pre, rest, post) = slice_parts(sp);
                for (q, sub) in self.slice_elems(base, len, elem, &pre, &post) {
                    self.pat_bind(sub, q, mode)?;
                }
                // `rest @ ..`: 残りの部分スライス (`&[T]`)
                if let Some(Pat::Ident(pi)) = rest {
                    let (es, _) = self.prog.size_align(elem);
                    let start = self.b.ins().iadd_imm_s(base, pre.len() as i64 * es as i64);
                    let n = self.b.ins().iadd_imm_s(len, -((pre.len() + post.len()) as i64));
                    let fat = self.make_fat_v(start, n);
                    let m = matches!(mode, BindMode::Ref(true));
                    self.bind(pi.ident.to_string(), Ty::Ref(m, intern(Ty::Slice(intern(elem)))), Some(fat));
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

// ---------------------------------------------------------------------------
// Drop
// ---------------------------------------------------------------------------
//
// - drop が要る型 ([`Program::needs_drop`]) の値を持つローカル変数・一時値は、所有者として
//   `owned` / `temps` に drop フラグ (スタック上の 1 byte) と一緒に登録する。
// - ムーブ (値としての読み出し・値で受ける self・move キャプチャ・値で束縛するパターン) でフラグを 0 にする。
//   フィールドだけのムーブ (部分ムーブ) も全体のフラグを 0 にする (残りはリークする)。
// - スコープの終わり・return / break / continue / `?` で、フラグが 1 のものを登録と逆順に drop する。
// - drop 本体は `__exr_drop_in_place::<T>` のインスタンス (drop glue)。codegen が型ごとに
//   1 段だけ展開した本体を作り、入れ子の型は呼び出しにする (再帰的な型でも展開が止まる)。
// - panic では drop しない (unwind しない)。

impl Cx<'_, '_> {
    fn needs_drop(&self, t: Ty) -> bool {
        self.prog.has_drop && self.prog.needs_drop(t)
    }

    fn new_flag(&mut self) -> StackSlot {
        let ss = self
            .b
            .create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 1, 0));
        let one = self.b.ins().iconst(types::I8, 1);
        self.b.ins().stack_store(PTR, one, ss, 0);
        ss
    }

    fn clear_flag(&mut self, flag: StackSlot) {
        let z = self.b.ins().iconst(types::I8, 0);
        self.b.ins().stack_store(PTR, z, flag, 0);
    }

    /// 今のスコープのローカル変数として所有する
    fn own_local(&mut self, loc: Loc, ty: Ty) {
        if !self.needs_drop(ty) {
            return;
        }
        let flag = self.new_flag();
        let depth = self.scopes.len() - 1;
        self.owned.push(Owned {
            loc,
            ty,
            flag,
            depth,
        });
    }

    /// 一時値 v (集約型ならアドレス) を文の終わりまで所有する。drop が要らなければ None
    fn own_temp(&mut self, v: Value, ty: Ty) -> Option<StackSlot> {
        if !self.needs_drop(ty) {
            return None;
        }
        let loc = if ty.is_agg() {
            Loc::Ptr(v)
        } else {
            let ss = self.slot(ty);
            self.b.ins().stack_store(PTR, v, ss, 0);
            Loc::Slot(ss)
        };
        let flag = self.new_flag();
        self.temps.push(Owned {
            loc,
            ty,
            flag,
            depth: 0,
        });
        Some(flag)
    }

    /// 0 で埋めた状態が「drop しても何もしない値」になる型か (Box は null、enum は判別値 0 のバリアント)。
    /// `impl Drop` を含む型は 0 にすると drop が走ってしまうので不可
    fn zeroable(&self, t: Ty) -> bool {
        match t {
            Ty::Box(_) => true,
            Ty::Array(e, _) => !self.needs_drop(*e) || self.zeroable(*e),
            Ty::Struct(sid) => {
                if self.prog.drop_impls.contains(&self.prog.base_of(sid)) {
                    return false;
                }
                let sd = &self.prog.structs[sid as usize];
                let ok = |fs: &[(String, Ty, u32)]| {
                    fs.iter().all(|f| !self.needs_drop(f.1) || self.zeroable(f.1))
                };
                if sd.is_enum {
                    sd.variants.iter().find(|v| v.discr == 0).is_some_and(|v| ok(&v.fields))
                } else {
                    ok(&sd.fields)
                }
            }
            _ => !self.needs_drop(t),
        }
    }

    fn zero(&mut self, dst: Value, t: Ty) {
        let (size, _) = self.prog.size_align(t);
        let z = self.b.ins().iconst(types::I8, 0);
        for i in 0..size {
            self.b.ins().store(mf(), z, dst, i as i32);
        }
    }

    /// prelude の Vec<T> のインスタンスなら (ptr のオフセット, len のオフセット, T)
    fn vec_fields(&self, sid: u32) -> Option<(u32, u32, Ty)> {
        let sd = &self.prog.structs[sid as usize];
        let b = sd.base?;
        if self.prog.structs[b as usize].name != "Vec" {
            return None;
        }
        let po = sd.fields.iter().find(|f| f.0 == "ptr")?.2;
        let lo = sd.fields.iter().find(|f| f.0 == "len")?.2;
        Some((po, lo, *sd.args.first()?))
    }

    /// `&Vec<T>` (アドレス a) → `&[T]`、`&String` → `&str` の fat pointer
    fn deref_to_slice(&mut self, a: Value, sid: u32, m: bool) -> Option<(Value, Ty)> {
        if let Some((po, lo, e)) = self.vec_fields(sid) {
            let p = self.b.ins().load(PTR, mf(), a, po as i32);
            let n = self.b.ins().load(types::I64, mf(), a, lo as i32);
            return Some((self.make_fat_v(p, n), Ty::Ref(m, intern(Ty::Slice(intern(e))))));
        }
        let sd = &self.prog.structs[sid as usize];
        if sd.base.is_none() && sd.name == "String" {
            let (_, vt, vo) = sd.fields.iter().find(|f| f.0 == "vec")?.clone();
            let Ty::Struct(vs) = vt else { return None };
            let va = self.addr_plus_v(a, vo);
            let (fat, _) = self.deref_to_slice(va, vs, m)?;
            return Some((fat, Ty::Ref(m, intern(Ty::Str))));
        }
        None
    }

    fn addr_plus_v(&mut self, a: Value, off: u32) -> Value {
        self.addr_plus(a, off as i32)
    }

    /// prelude の関数 (ジェネリックなら型引数 targs のインスタンス) を、評価済みの引数で呼ぶ
    /// 受け手の struct の型引数で impl のメソッド (`Rc::<T>::deref`) を値の引数で呼ぶ
    fn call_fn_vals(&mut self, id: u32, args: &[(Value, Ty)], span: proc_macro2::Span) -> R<V> {
        let targs = match args.first().map(|a| self.strip_refs(a.1)) {
            Some(Ty::Struct(sid)) if !self.prog.sigs[id as usize].generics.is_empty() => {
                self.prog.structs[sid as usize].args.clone()
            }
            _ => vec![],
        };
        let name = self.prog.names[id as usize].clone();
        self.call_named(&name, targs, args, span)
    }

    fn call_named(
        &mut self,
        name: &str,
        targs: Vec<Ty>,
        args: &[(Value, Ty)],
        span: proc_macro2::Span,
    ) -> R<V> {
        let Some(&id) = self.prog.index.get(name) else {
            return err(span, format!("internal: `{name}` is missing from the prelude"));
        };
        let iid = if targs.is_empty() {
            id
        } else {
            let mut g = self.insts.lock().unwrap();
            let i = g.request(self.prog, id, targs).or_else(|e| err(span, e))?;
            drop(g);
            self.requested.push(i);
            i
        };
        let sig = self.insts.lock().unwrap().sig(self.prog, iid).clone();
        let mut cv = vec![];
        let sret = if is_sret(&sig) {
            let a = self.temp(sig.ret);
            cv.push(a);
            Some(a)
        } else {
            None
        };
        for (&(v, t), &pt) in args.iter().zip(&sig.params) {
            self.same(span, t, pt)?;
            self.push_arg(&mut cv, v, pt);
        }
        let f = self.fref(iid, &sig);
        let call = self.b.ins().call(f, &cv);
        if let Some(a) = sret {
            return Ok((Some(a), sig.ret));
        }
        let r = self.b.inst_results(call).first().copied();
        Ok((r, sig.ret))
    }

    /// 二項演算の被演算子。比較 (cmp) ならムーブせずに読み、一時値は所有する
    fn operand(&mut self, e: &Expr, hint: Option<Ty>, cmp: bool) -> R<V> {
        if !cmp {
            return self.expr(e, hint);
        }
        // `a[..2] == [1, 2]`: サイズ不定のスライスは参照で比べる (`[T]` の PartialEq の &self)
        if let Expr::Index(ix) = strip_paren(e) {
            if matches!(&*ix.index, Expr::Range(_)) {
                let r: Expr = syn::parse_quote!(&#e);
                return self.expr(&r, None);
            }
        }
        // (`None` などローカル変数でない名前は値として評価する)
        let non_local = matches!(strip_paren(e), Expr::Path(p)
            if p.path.get_ident().is_some_and(|id| self.lookup(&id.to_string()).is_none()));
        if is_place_expr(e) && !non_local {
            let r = self.read_no_move(e)?;
            if self.needs_drop(r.1) || !matches!(hint, Some(_)) {
                return Ok(r);
            }
            // (整数リテラルの型合わせなど、期待型の変換が要る場合は通常の評価)
            if hint == Some(r.1) {
                return Ok(r);
            }
            return self.expr(e, hint);
        }
        let r = self.expr(e, hint)?;
        if let (Some(v), t) = r {
            if t != Ty::Never {
                self.own_temp(v, t);
            }
        }
        Ok(r)
    }

    fn strip_refs(&self, mut t: Ty) -> Ty {
        while let Ty::Ref(_, i) = t {
            t = *i;
        }
        t
    }

    /// 値を読むがムーブはしない (deref・呼び出しの対象など)
    fn read_no_move(&mut self, e: &Expr) -> R<V> {
        if is_place_expr(e) && !matches!(strip_paren(e), Expr::Path(p) if self.fn_item(p).is_some() || self.const_of(p).is_some()) {
            let p = self.place(e)?;
            Ok(self.read(p))
        } else {
            self.expr(e, None)
        }
    }

    /// アドレス a にある一時値を所有する
    fn own_temp_at(&mut self, a: Value, ty: Ty) {
        if !self.needs_drop(ty) {
            return;
        }
        let flag = self.new_flag();
        self.temps.push(Owned {
            loc: Loc::Ptr(a),
            ty,
            flag,
            depth: 0,
        });
    }

    /// ローカル変数 (の場所) の drop フラグ
    fn flag_of_loc(&self, loc: Loc) -> Option<StackSlot> {
        self.owned.iter().rev().find(|o| o.loc == loc).map(|o| o.flag)
    }

    /// 式 e (ローカル変数か、そのフィールド) から値をムーブした: 変数のフラグを下ろす
    fn mark_moved(&mut self, e: &Expr) {
        let mut e = e;
        let mut via_field = false;
        loop {
            match e {
                Expr::Paren(p) => e = &p.expr,
                Expr::Group(g) => e = &g.expr,
                // 参照越しのフィールドからはムーブできない (Copy 以外は rustc が弾く)
                Expr::Field(f) => {
                    via_field = true;
                    e = &f.base
                }
                Expr::Path(p) => {
                    let Some(id) = p.path.get_ident() else { return };
                    if let Some((loc, t)) = self.lookup(&id.to_string()) {
                        if matches!(t, Ty::Ref(..) | Ty::Ptr(..)) {
                            return;
                        }
                        // (Box の中身のフィールドをムーブしたときも Box ごとムーブ済みにする: 領域はリークする)
                        let _ = via_field;
                        if let Some(f) = self.flag_of_loc(loc) {
                            self.clear_flag(f);
                        }
                    }
                    return;
                }
                _ => return,
            }
        }
    }

    /// パターンが match 対象 e から値でムーブした (flag は e が一時値のときのフラグ)
    fn release(&mut self, e: &Expr, flag: Option<StackSlot>) {
        if is_place_expr(e) {
            self.mark_moved(e);
        } else if let Some(f) = flag {
            self.clear_flag(f);
        }
    }

    /// 代入で上書きする前に古い値を drop する。ローカル変数ならフラグを見て、代入後に立て直すフラグを返す
    fn drop_before_assign(&mut self, left: &Expr, p: Place) -> Option<StackSlot> {
        if let Expr::Path(pp) = strip_paren(left) {
            if let Some(id) = pp.path.get_ident() {
                if let Some((loc, ty)) = self.lookup(&id.to_string()) {
                    if let Some(flag) = self.flag_of_loc(loc) {
                        self.emit_owned_drop(Owned {
                            loc,
                            ty,
                            flag,
                            depth: 0,
                        });
                        return Some(flag);
                    }
                }
            }
        }
        // フィールド・`*p`・`a[i]`: 初期化済みなので必ず drop する
        let addr = match p {
            Place::Mem(a, o, _) => self.addr_plus(a, o),
            Place::Var(v, t) => {
                let x = self.b.use_var(v);
                let ss = self.slot(t);
                self.b.ins().stack_store(PTR, x, ss, 0);
                self.b.ins().stack_addr(PTR, ss, 0)
            }
            Place::Zst(_) => return None,
        };
        self.drop_in_place(addr, p.ty());
        None
    }

    fn mark_moved_loc(&mut self, loc: Loc) {
        if let Some(f) = self.flag_of_loc(loc) {
            self.clear_flag(f);
        }
    }

    /// フラグが立っていれば drop する
    fn emit_owned_drop(&mut self, o: Owned) {
        let f = self.b.ins().stack_load(PTR, types::I8, o.flag, 0);
        let yes = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().brif(f, yes, &[], done, &[]);
        self.b.switch_to_block(yes);
        self.clear_flag(o.flag);
        let addr = match o.loc {
            Loc::Slot(ss) => self.b.ins().stack_addr(PTR, ss, 0),
            Loc::Ptr(a) => a,
            Loc::Var(v) => {
                let x = self.b.use_var(v);
                let ss = self.slot(o.ty);
                self.b.ins().stack_store(PTR, x, ss, 0);
                self.b.ins().stack_addr(PTR, ss, 0)
            }
            Loc::Zst => self.b.ins().iconst(PTR, 0),
        };
        self.drop_in_place(addr, o.ty);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
    }

    /// addr にある t の値を drop する (drop glue の呼び出し)
    fn drop_in_place(&mut self, addr: Value, t: Ty) {
        if !self.needs_drop(t) {
            return;
        }
        let Some(&g) = self.prog.index.get(crate::prelude::DROP_GLUE) else {
            return;
        };
        let iid = {
            let mut ins = self.insts.lock().unwrap();
            match ins.request(self.prog, g, vec![t]) {
                Ok(i) => i,
                Err(_) => return,
            }
        };
        self.requested.push(iid);
        let sig = self.insts.lock().unwrap().sig(self.prog, iid).clone();
        let f = self.fref(iid, &sig);
        self.b.ins().call(f, &[addr]);
    }

    /// スコープを閉じる: そのスコープのローカル変数を逆順に drop する
    fn pop_scope(&mut self) {
        let depth = self.scopes.len() - 1;
        while self.owned.last().is_some_and(|o| o.depth >= depth) {
            let o = self.owned.pop().unwrap();
            self.emit_owned_drop(o);
        }
        self.scopes.pop();
    }

    /// 早期脱出 (return / break / continue): 深さ depth 以上のローカル変数と temps[t..] を drop する
    fn drop_for_exit(&mut self, depth: usize, t: usize) {
        let temps: Vec<Owned> = self.temps[t.min(self.temps.len())..].to_vec();
        for o in temps.into_iter().rev() {
            self.emit_owned_drop(o);
        }
        let locals: Vec<Owned> = self.owned.iter().filter(|o| o.depth >= depth).copied().collect();
        for o in locals.into_iter().rev() {
            self.emit_owned_drop(o);
        }
    }

    /// 文の終わり: temps[t..] を drop して捨てる
    fn drop_temps(&mut self, t: usize) {
        while self.temps.len() > t {
            let o = self.temps.pop().unwrap();
            self.emit_owned_drop(o);
        }
    }

    /// ブロックの末尾の式・match の arm の一時値は、その場で drop する (Rust 2024 と同じ)。
    /// 分岐の中で作った値を文の終わりで触ると支配関係が崩れるため。結果そのものの一時値は残す
    fn drop_tail_temps(&mut self, t: usize, result: V) {
        if result.1 == Ty::Never {
            self.temps.truncate(t.min(self.temps.len()));
            return;
        }
        let keep: Vec<Owned> = self.temps[t.min(self.temps.len())..]
            .iter()
            .filter(|o| result.0.is_some_and(|v| o.loc == Loc::Ptr(v)))
            .copied()
            .collect();
        let rest: Vec<Owned> = self.temps.drain(t.min(self.temps.len())..).collect();
        for o in rest.into_iter().rev() {
            if !keep.iter().any(|k| k.loc == o.loc) {
                self.emit_owned_drop(o);
            }
        }
        self.temps.extend(keep);
    }

    /// let の初期化式の一時値は、ブロックの終わりまで延ばす (rustc の一時値の寿命延長の近似)
    fn extend_temps(&mut self, t: usize) {
        let depth = self.scopes.len() - 1;
        let ts: Vec<Owned> = self.temps.drain(t.min(self.temps.len())..).collect();
        for mut o in ts {
            o.depth = depth;
            self.owned.push(o);
        }
    }

    fn free(&mut self, p: Value) {
        if let Some(&fr) = self.prog.index.get(crate::prelude::BOX_FREE) {
            let sig = self.prog.sigs[fr as usize].clone();
            let f = self.fref(fr, &sig);
            self.b.ins().call(f, &[p]);
        }
    }

    /// vtable の中で drop glue が入っている位置 (メソッドの後ろ)
    fn vtable_drop_index(&self, tr: &str) -> usize {
        if crate::front::parse_fn_bound(tr).is_some() {
            1
        } else {
            self.prog.traits.get(tr).map(|v| v.len()).unwrap_or(0)
        }
    }

    /// vtable の drop の欄: t の drop glue (要らなければ u32::MAX = null)
    fn vtable_drop_entry(&mut self, t: Ty) -> u32 {
        if !self.needs_drop(t) {
            return u32::MAX;
        }
        let Some(&g) = self.prog.index.get(crate::prelude::DROP_GLUE) else {
            return u32::MAX;
        };
        let mut ins = self.insts.lock().unwrap();
        match ins.request(self.prog, g, vec![t]) {
            Ok(i) => {
                drop(ins);
                self.requested.push(i);
                i
            }
            Err(_) => u32::MAX,
        }
    }

    /// `__exr_drop_in_place::<T>(p: *mut T)` の本体
    fn drop_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let p = self.b.block_params(entry)[0];
        let Some(Ty::Ptr(_, t)) = sig.params.first() else {
            return Err("internal: bad drop glue signature".into());
        };
        let t = **t;
        match t {
            Ty::Box(Ty::Dyn(tr)) => {
                // 中身の型は vtable の drop 欄が知っている。data が 0 ならムーブ済み
                let data = self.b.ins().load(PTR, mf(), p, 0);
                let live = self.b.create_block();
                let out = self.b.create_block();
                self.b.ins().brif(data, live, &[], out, &[]);
                self.b.switch_to_block(live);
                let vt = self.b.ins().load(PTR, mf(), p, 8);
                let idx = self.vtable_drop_index(tr);
                let dfn = self.b.ins().load(PTR, mf(), vt, 8 * idx as i32);
                let call = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().brif(dfn, call, &[], done, &[]);
                self.b.switch_to_block(call);
                let mut s = Signature::new(self.cc);
                s.params.push(AbiParam::new(PTR));
                let sr = self.b.import_signature(s);
                self.b.ins().call_indirect(sr, dfn, &[data]);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                self.free(data);
                self.b.ins().jump(out, &[]);
                self.b.switch_to_block(out);
            }
            Ty::Box(inner) => {
                let ptr = self.b.ins().load(PTR, mf(), p, 0);
                let live = self.b.create_block();
                let out = self.b.create_block();
                self.b.ins().brif(ptr, live, &[], out, &[]);
                self.b.switch_to_block(live);
                self.drop_in_place(ptr, *inner);
                self.free(ptr);
                self.b.ins().jump(out, &[]);
                self.b.switch_to_block(out);
            }
            Ty::Array(e, n) => {
                let (size, _) = self.prog.size_align(*e);
                // 要素ごとに drop (長さは静的に分かるので展開しないループ)
                let i = self.b.declare_var(types::I64);
                let z = self.b.ins().iconst(types::I64, 0);
                self.b.def_var(i, z);
                let head = self.b.create_block();
                let body = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(head);
                let iv = self.b.use_var(i);
                let more = self.b.ins().icmp_imm_u(IntCC::UnsignedLessThan, iv, n as i64);
                self.b.ins().brif(more, body, &[], done, &[]);
                self.b.switch_to_block(body);
                let off = self.b.ins().imul_imm_s(iv, size as i64);
                let a = self.b.ins().iadd(p, off);
                self.drop_in_place(a, *e);
                let next = self.b.ins().iadd_imm_s(iv, 1);
                self.b.def_var(i, next);
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(done);
            }
            Ty::Struct(sid) => {
                // 先に `impl Drop` の drop(&mut self)、それからフィールド
                if self.prog.drop_impls.contains(&self.prog.base_of(sid)) {
                    if let Some(m) = self.prog.method(sid, "drop") {
                        let recv = [Ty::Ref(true, intern(t))];
                        let mut fid = m;
                        let ms = &self.prog.sigs[m as usize];
                        if ms.is_generic() {
                            let mut sol = vec![None; ms.generics.len()];
                            if let Some(st) = ms.self_ty {
                                self.prog.match_ty(st, t, &mut sol);
                            }
                            fid = self
                                .resolve_call(m, &recv, sol, None, proc_macro2::Span::call_site())?
                                .0;
                        }
                        let fs = self.insts.lock().unwrap().sig(self.prog, fid).clone();
                        let f = self.fref(fid, &fs);
                        self.b.ins().call(f, &[p]);
                    }
                }
                let sd = &self.prog.structs[sid as usize];
                if sd.is_enum {
                    let tag_ty = sd.tag;
                    let vs: Vec<(i64, Vec<(Ty, u32)>)> = sd
                        .variants
                        .iter()
                        .map(|v| {
                            (
                                v.discr,
                                v.fields
                                    .iter()
                                    .filter(|f| self.needs_drop(f.1))
                                    .map(|f| (f.1, f.2))
                                    .collect(),
                            )
                        })
                        .collect();
                    let tag = self.b.ins().load(clif_ty(tag_ty).unwrap(), mf(), p, 0);
                    for (d, fs) in vs {
                        if fs.is_empty() {
                            continue;
                        }
                        let yes = self.b.create_block();
                        let next = self.b.create_block();
                        let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, d);
                        self.b.ins().brif(c, yes, &[], next, &[]);
                        self.b.switch_to_block(yes);
                        for (ft, off) in fs {
                            let a = self.addr_plus(p, off as i32);
                            self.drop_in_place(a, ft);
                        }
                        self.b.ins().jump(next, &[]);
                        self.b.switch_to_block(next);
                    }
                } else {
                    let fs: Vec<(Ty, u32)> = sd
                        .fields
                        .iter()
                        .filter(|f| self.needs_drop(f.1))
                        .map(|f| (f.1, f.2))
                        .collect();
                    for (ft, off) in fs {
                        let a = self.addr_plus(p, off as i32);
                        self.drop_in_place(a, ft);
                    }
                }
            }
            _ => {}
        }
        self.b.ins().return_(&[]);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 書式化 (format! / print! / println! / eprint! / eprintln! / write! / writeln!)
// ---------------------------------------------------------------------------
//
// 書式文字列はコンパイル時に分解し、prelude の Formatter に「文字列片」と「値」を順に書く。
// 値は `__exr_display::<T>` / `__exr_debug::<T>` (drop glue と同じく型ごとに codegen が本体を作る) で書く。
// 数値・文字列の書き出し・パディング・Debug の整形は prelude の Rust の関数。

/// 書式指定 1 個分
#[derive(Clone, Default, PartialEq)]
struct FmtSpec {
    width: usize,
    /// `{:w$}` / `{:1$}`: 幅を引数から取る (引数の番号)
    width_arg: Option<usize>,
    /// `{:.p$}` / `{:.1$}` / `{:.*}`: 精度を引数から取る
    prec_arg: Option<usize>,
    fill: char,
    align: u8,
    flags: u8,
    prec: Option<usize>,
    radix: u8,
}

enum FmtPiece {
    Lit(String),
    Arg {
        /// 引数の番号 (位置か名前から決めたもの)
        arg: usize,
        debug: bool,
        spec: FmtSpec,
    },
}

/// 書式文字列を分解する。名前つきの `{x}` は named (名前つき引数) か、なければその名前の変数を引数に足す
fn parse_fmt(
    s: &str,
    n_pos: usize,
    named: &mut Vec<String>,
    span: proc_macro2::Span,
) -> R<Vec<FmtPiece>> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = vec![];
    let mut lit = String::new();
    let mut next = 0;
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == '{' && cs.get(i + 1) == Some(&'{') {
            lit.push('{');
            i += 2;
            continue;
        }
        if c == '}' && cs.get(i + 1) == Some(&'}') {
            lit.push('}');
            i += 2;
            continue;
        }
        if c == '}' {
            return err(span, "invalid format string: unmatched `}`");
        }
        if c != '{' {
            lit.push(c);
            i += 1;
            continue;
        }
        let Some(end) = cs[i..].iter().position(|&x| x == '}').map(|p| p + i) else {
            return err(span, "invalid format string: unmatched `{`");
        };
        let inner: String = cs[i + 1..end].iter().collect();
        i = end + 1;
        if !lit.is_empty() {
            out.push(FmtPiece::Lit(std::mem::take(&mut lit)));
        }
        let (a, sp) = match inner.split_once(':') {
            Some((a, s)) => (a.trim().to_string(), s.to_string()),
            None => (inner.trim().to_string(), String::new()),
        };
        let implicit = a.is_empty();
        let mut arg = if a.is_empty() {
            next += 1;
            next - 1
        } else if let Ok(k) = a.parse::<usize>() {
            k
        } else {
            match named.iter().position(|n| *n == a) {
                Some(k) => n_pos + k,
                None => {
                    named.push(a);
                    n_pos + named.len() - 1
                }
            }
        };
        // [[fill]align][+][#][0][width][.prec][type]
        let sc: Vec<char> = sp.chars().collect();
        let mut j = 0;
        let mut spec = FmtSpec {
            fill: ' ',
            ..Default::default()
        };
        let al = |c: char| match c {
            '<' => Some(1),
            '>' => Some(2),
            '^' => Some(3),
            _ => None,
        };
        if sc.len() >= 2 && al(sc[1]).is_some() {
            spec.fill = sc[0];
            spec.align = al(sc[1]).unwrap();
            j = 2;
        } else if !sc.is_empty() && al(sc[0]).is_some() {
            spec.align = al(sc[0]).unwrap();
            j = 1;
        }
        if sc.get(j) == Some(&'+') {
            spec.flags |= 2;
            j += 1;
        }
        if sc.get(j) == Some(&'#') {
            spec.flags |= 4;
            j += 1;
        }
        if sc.get(j) == Some(&'0') {
            spec.flags |= 1;
            j += 1;
        }
        // 幅: 数字、または `名前$` / `番号$`
        let ws = j;
        while sc.get(j).is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_') {
            j += 1;
        }
        let wtok: String = sc[ws..j].iter().collect();
        if sc.get(j) == Some(&'$') {
            j += 1;
            spec.width_arg = Some(match wtok.parse::<usize>() {
                Ok(k) => k,
                Err(_) => match named.iter().position(|n| *n == wtok) {
                    Some(k) => n_pos + k,
                    None => {
                        named.push(wtok);
                        n_pos + named.len() - 1
                    }
                },
            });
        } else {
            // 先頭の数字だけが幅で、残りは型の指定 (`12e` / `x` / `b` など)
            j = ws;
            let mut w = 0;
            let mut any = false;
            while let Some(d) = sc.get(j).and_then(|c| c.to_digit(10)) {
                w = w * 10 + d as usize;
                j += 1;
                any = true;
            }
            if any {
                spec.width = w;
            }
        }
        if sc.get(j) == Some(&'.') {
            j += 1;
            let ps = j;
            while sc.get(j).is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_') {
                j += 1;
            }
            let ptok: String = sc[ps..j].iter().collect();
            if sc.get(ps) == Some(&'*') {
                // `.*`: 次の位置引数が精度で、その次が値
                j = ps + 1;
                spec.prec = Some(0);
                if implicit {
                    spec.prec_arg = Some(arg);
                    arg = next;
                } else {
                    spec.prec_arg = Some(next);
                }
                next += 1;
            } else if sc.get(j) == Some(&'$') {
                j += 1;
                spec.prec = Some(0);
                spec.prec_arg = Some(match ptok.parse::<usize>() {
                    Ok(k) => k,
                    Err(_) => match named.iter().position(|n| *n == ptok) {
                        Some(k) => n_pos + k,
                        None => {
                            named.push(ptok);
                            n_pos + named.len() - 1
                        }
                    },
                });
            } else {
                // 数字だけ取り、残りは型の指定
                j = ps;
                let mut p = 0;
                while let Some(d) = sc.get(j).and_then(|c| c.to_digit(10)) {
                    p = p * 10 + d as usize;
                    j += 1;
                }
                spec.prec = Some(p);
            }
        }
        let ty: String = sc[j..].iter().collect();
        let (debug, radix) = match ty.as_str() {
            "" => (false, 0),
            "?" => (true, 0),
            "x" => (false, 1),
            "X" => (false, 2),
            "b" => (false, 3),
            "o" => (false, 4),
            "x?" => (true, 1),
            "X?" => (true, 2),
            "e" => (false, 5),
            "E" => (false, 6),
            t => return err(span, format!("unsupported format specifier `{t}`")),
        };
        spec.radix = radix;
        out.push(FmtPiece::Arg { arg, debug, spec });
    }
    if !lit.is_empty() {
        out.push(FmtPiece::Lit(lit));
    }
    Ok(out)
}

impl Cx<'_, '_> {
    /// &str の定数 (fat pointer のアドレス)
    fn str_const(&mut self, s: &str) -> Value {
        let n = s.len();
        let p = self.data_addr(s.as_bytes().to_vec());
        self.make_fat(p, n as i64)
    }

    fn formatter_ty(&self) -> R<Ty> {
        let Some(&sid) = self.prog.struct_index.get("Formatter") else {
            return Err("internal: Formatter is missing from the prelude".into());
        };
        Ok(Ty::Struct(sid))
    }

    fn string_ty(&self) -> R<Ty> {
        let Some(&sid) = self.prog.struct_index.get("String") else {
            return Err("internal: String is missing from the prelude".into());
        };
        Ok(Ty::Struct(sid))
    }

    /// 値として書けるか (`{}` なら Display、`{:?}` なら Debug を実装しているか)
    fn fmt_ok(&self, t: Ty, debug: bool) -> bool {
        match t {
            t if t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char | Ty::Str) => true,
            Ty::Unit => debug,
            Ty::Ref(_, i) | Ty::Box(i) => self.fmt_ok(*i, debug),
            Ty::Array(e, _) | Ty::Slice(e) => debug && self.fmt_ok(*e, debug),
            Ty::Struct(sid) => {
                let b = self.prog.base_of(sid);
                let tr = if debug { "Debug" } else { "Display" };
                if self.prog.impls.contains(&(b, tr.to_string())) {
                    return true;
                }
                debug && self.prog.is_tuple(b)
                    && self.prog.structs[sid as usize].args.iter().all(|&a| self.fmt_ok(a, true))
            }
            _ => false,
        }
    }

    /// 書式化マクロの本体。dest が Some なら write! (その Formatter / String に書く)
    fn fmt_macro(&mut self, mac: &syn::Macro, name: &str) -> R<V> {
        use syn::parse::Parser;
        let span = mac.span();
        let p = syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated;
        let mut args: Vec<Expr> = p
            .parse2(mac.tokens.clone())
            .map(|v| v.into_iter().collect())
            .or_else(|e| err(span, e))?;
        let dest = if matches!(name, "write" | "writeln") {
            if args.is_empty() {
                return err(span, format!("`{name}!` needs a destination"));
            }
            Some(args.remove(0))
        } else {
            None
        };
        let newline = matches!(name, "println" | "eprintln" | "writeln");
        let fmt_s = match args.first() {
            Some(Expr::Lit(syn::ExprLit {
                lit: Lit::Str(s), ..
            })) => s.value(),
            None if newline || dest.is_some() => String::new(),
            Some(e) => return err(e.span(), "format argument must be a string literal"),
            None => return err(span, format!("`{name}!` needs a format string")),
        };
        let rest: Vec<Expr> = args.into_iter().skip(1).collect();
        // 位置引数と `name = expr`
        let mut pos: Vec<Expr> = vec![];
        let mut named: Vec<String> = vec![];
        let mut named_e: Vec<Expr> = vec![];
        for a in rest {
            match a {
                Expr::Assign(asg) => {
                    let Expr::Path(p) = &*asg.left else {
                        return err(asg.span(), "expected `name = value`");
                    };
                    named.push(p.path.get_ident().map(|i| i.to_string()).unwrap_or_default());
                    named_e.push(*asg.right);
                }
                e => pos.push(e),
            }
        }
        let mut names = named.clone();
        let mut pieces = parse_fmt(&fmt_s, pos.len(), &mut names, span)?;
        if newline {
            pieces.push(FmtPiece::Lit("\n".into()));
        }
        // 書式文字列の `{x}` (引数にない名前) はその名前の変数
        for n in &names[named.len()..] {
            let id = syn::Ident::new(n, span);
            named_e.push(syn::parse_quote!(#id));
        }
        let all: Vec<Expr> = pos.into_iter().chain(named_e).collect();
        // 書き込み先の Formatter
        let fty = self.formatter_ty().or_else(|e| err(span, e))?;
        let fref = Ty::Ref(true, intern(fty));
        let sty = self.string_ty().or_else(|e| err(span, e))?;
        let sref = Ty::Ref(true, intern(sty));
        let mut owned_string = None;
        let mut writer: Option<(Value, Ty, u32)> = None;
        let f = match &dest {
            Some(d) => {
                let (v, t) = self.read_no_move(d)?;
                let v = v.ok_or("expected a destination")?;
                let mut t = t;
                let mut v = v;
                while let Ty::Ref(_, i) = t {
                    if matches!(i, Ty::Struct(_)) {
                        t = *i;
                        break;
                    }
                    v = self.b.ins().load(PTR, mf(), v, 0);
                    t = *i;
                }
                if t == fty {
                    v
                } else if t == sty {
                    self.call_named("__exr_formatter", vec![Ty::Unit], &[(v, sref)], span)?.0.unwrap()
                } else if let Some(id) = match t {
                    Ty::Struct(sid) => self.prog.method(sid, "write_str"),
                    _ => None,
                } {
                    // `write_str` を持つ書き込み先 (Stdout / BufWriter / fmt::Write を実装した型):
                    // いったん String に書いてから渡す
                    writer = Some((v, t, id));
                    let (s, _) = self.call_named("String::new", vec![], &[], span)?;
                    let s = s.unwrap();
                    owned_string = Some(s);
                    self.call_named("__exr_formatter", vec![Ty::Unit], &[(s, sref)], span)?.0.unwrap()
                } else {
                    return err(
                        d.span(),
                        format!("cannot write into `{}`", self.prog.tn(t)),
                    );
                }
            }
            None => {
                let (s, _) = self.call_named("String::new", vec![], &[], span)?;
                let s = s.unwrap();
                owned_string = Some(s);
                self.call_named("__exr_formatter", vec![Ty::Unit], &[(s, sref)], span)?.0.unwrap()
            }
        };
        // 引数は (ムーブせずに) 1 回ずつ評価してアドレスにする
        let mut vals: Vec<(Value, Ty, proc_macro2::Span)> = vec![];
        // 幅に使う引数 (`{:w$}`) は usize
        let width_args: Vec<usize> = pieces
            .iter()
            .flat_map(|p| match p {
                FmtPiece::Arg { spec, .. } => vec![spec.width_arg, spec.prec_arg],
                _ => vec![],
            })
            .flatten()
            .collect();
        self.in_fmt_args += 1;
        let r = self.fmt_eval_args(&all, &width_args, &mut vals);
        self.in_fmt_args -= 1;
        r?;
        if vals.len() < all.len() {
            return Ok((None, Ty::Never));
        }
        let mut cur = FmtSpec {
            fill: ' ',
            ..Default::default()
        };
        let str_ref = Ty::Ref(false, intern(Ty::Str));
        for pc in &pieces {
            match pc {
                FmtPiece::Lit(s) => {
                    let l = self.str_const(s);
                    self.call_named("__exr_fmt_lit", vec![Ty::Unit], &[(f, fref), (l, str_ref)], span)?;
                }
                FmtPiece::Arg { arg, debug, spec } => {
                    let Some(&(v, t, asp)) = vals.get(*arg) else {
                        return err(span, format!("format string refers to argument {arg}, but there are {}", vals.len()));
                    };
                    if !self.fmt_ok(t, *debug) {
                        return err(
                            asp,
                            format!(
                                "`{}` doesn't implement `{}`",
                                self.prog.tn(t),
                                if *debug { "Debug" } else { "Display" }
                            ),
                        );
                    }
                    if *spec != cur || spec.width_arg.is_some() || spec.prec_arg.is_some() {
                        let c = |this: &mut Self, ty: ir::Type, x: i64| this.b.ins().iconst(ty, x);
                        let w = match spec.width_arg {
                            Some(k) => {
                                let Some(&(wv, wt, wsp)) = vals.get(k) else {
                                    return err(span, "missing width argument");
                                };
                                self.same(wsp, wt, Ty::Usize)?;
                                self.b.ins().load(types::I64, mf(), wv, 0)
                            }
                            None => c(self, types::I64, spec.width as i64),
                        };
                        let fill = c(self, types::I32, spec.fill as i64);
                        let al = c(self, types::I8, spec.align as i64);
                        let fl = c(self, types::I8, spec.flags as i64);
                        let pr = match spec.prec_arg {
                            Some(k) => {
                                let Some(&(pv, pt, psp)) = vals.get(k) else {
                                    return err(span, "missing precision argument");
                                };
                                self.same(psp, pt, Ty::Usize)?;
                                self.b.ins().load(types::I64, mf(), pv, 0)
                            }
                            None => c(self, types::I64, spec.prec.unwrap_or(0) as i64),
                        };
                        let hp = c(self, types::I8, spec.prec.is_some() as i64);
                        let rx = c(self, types::I8, spec.radix as i64);
                        self.call_named(
                            "__exr_fmt_spec",
                            vec![Ty::Unit],
                            &[
                                (f, fref),
                                (w, Ty::Usize),
                                (fill, Ty::Char),
                                (al, Ty::U8),
                                (fl, Ty::U8),
                                (pr, Ty::Usize),
                                (hp, Ty::Bool),
                                (rx, Ty::U8),
                            ],
                            span,
                        )?;
                        cur = spec.clone();
                    }
                    // `{:e}` / `{:E}`: 数値を指数表記で (prelude の __exr_fmt_exp)
                    if spec.radix >= 5 && (t.is_int() || t.is_float()) {
                        let ct = clif_ty(t).unwrap();
                        let x = self.b.ins().load(ct, mf(), v, 0);
                        let up = self.b.ins().iconst(types::I8, (spec.radix == 6) as i64);
                        if t.is_float() {
                            let x = if t == Ty::F32 { self.b.ins().fpromote(types::F64, x) } else { x };
                            let is32 = self.b.ins().iconst(types::I8, (t == Ty::F32) as i64);
                            self.call_named("__exr_fmt_exp", vec![Ty::Unit], &[(f, fref), (x, Ty::F64), (is32, Ty::Bool), (up, Ty::Bool)], span)?;
                        } else {
                            // (絶対値を u128 にして渡す)
                            let w = if ct == types::I128 {
                                x
                            } else if t.signed() {
                                self.b.ins().sextend(types::I128, x)
                            } else {
                                self.b.ins().uextend(types::I128, x)
                            };
                            let z = self.int_const(Ty::I128, 0);
                            let neg = if t.signed() { self.b.ins().icmp(IntCC::SignedLessThan, w, z) } else { self.b.ins().iconst(types::I8, 0) };
                            let nw = self.b.ins().ineg(w);
                            let mag = self.b.ins().select(neg, nw, w);
                            self.call_named("__exr_fmt_exp_int", vec![Ty::Unit], &[(f, fref), (mag, Ty::U128), (neg, Ty::Bool), (up, Ty::Bool)], span)?;
                        }
                        continue;
                    }
                    let g = if *debug { "__exr_debug" } else { "__exr_display" };
                    self.call_named(g, vec![t], &[(v, Ty::Ref(false, intern(t))), (f, fref)], span)?;
                }
            }
        }
        if let Some((wv, wt, id)) = writer {
            let s = owned_string.unwrap();
            let str_ref = Ty::Ref(false, intern(Ty::Str));
            let (sv, _) = self.call_named("String::as_str", vec![], &[(s, Ty::Ref(false, intern(sty)))], span)?;
            let r = self.call_fn_vals(id, &[(wv, Ty::Ref(true, intern(wt))), (sv.unwrap(), str_ref)], span)?;
            self.own_temp_at(s, sty);
            return Ok(r);
        }
        match name {
            "format" => Ok((owned_string, sty)),
            "print" | "println" | "eprint" | "eprintln" => {
                let s = owned_string.unwrap();
                let pf = if name.starts_with('e') { "__exr_eprint" } else { "__exr_print" };
                self.call_named(pf, vec![Ty::Unit], &[(s, Ty::Ref(false, intern(sty)))], span)?;
                self.own_temp_at(s, sty);
                Ok((None, Ty::Unit))
            }
            _ => self.call_named("__exr_fmt_ok", vec![Ty::Unit], &[], span),
        }
    }

    /// 書式マクロの引数を 1 回ずつ評価してアドレスにする (発散したらそこで止める: vals が足りなくなる)
    fn fmt_eval_args(
        &mut self,
        all: &[Expr],
        width_args: &[usize],
        vals: &mut Vec<(Value, Ty, proc_macro2::Span)>,
    ) -> R<()> {
        for (ai, a) in all.iter().enumerate() {
            let (v, t) = if is_place_expr(a) {
                match self.place(a)? {
                    Place::Mem(p, o, t) => (self.addr_plus(p, o), t),
                    Place::Var(var, t) => {
                        let x = self.b.use_var(var);
                        let ss = self.slot(t);
                        self.b.ins().stack_store(PTR, x, ss, 0);
                        (self.b.ins().stack_addr(PTR, ss, 0), t)
                    }
                    Place::Zst(t) => (self.b.ins().iconst(PTR, 0), t),
                }
            } else {
                let exp = width_args.contains(&ai).then_some(Ty::Usize);
                let (v, t) = self.expr(a, exp)?;
                if t == Ty::Never {
                    return Ok(());
                }
                let v = match v {
                    Some(v) if t.is_agg() => {
                        self.own_temp_at(v, t);
                        v
                    }
                    Some(v) => {
                        let ss = self.slot(t);
                        self.b.ins().stack_store(PTR, v, ss, 0);
                        let a = self.b.ins().stack_addr(PTR, ss, 0);
                        if self.needs_drop(t) {
                            self.own_temp_at(a, t);
                        }
                        a
                    }
                    None => self.b.ins().iconst(PTR, 0),
                };
                (v, t)
            };
            vals.push((v, t, a.span()));
        }
        Ok(())
    }

    /// `x.to_string()` (Display を実装した型): `format!("{}", x)` と同じ
    fn to_string_value(&mut self, recv: &Expr, span: proc_macro2::Span) -> R<V> {
        let mac: syn::Macro = syn::parse_quote!(format!("{}", #recv));
        let _ = span;
        self.fmt_macro(&mac, "format")
    }

    /// `__exr_display::<T>` / `__exr_debug::<T>` の本体
    fn fmt_glue_body(&mut self, sig: &FnSig, debug: bool) -> R<()> {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let ps = self.b.block_params(entry).to_vec();
        let (x, f) = (ps[0], ps[1]);
        let Some(Ty::Ref(_, t)) = sig.params.first() else {
            return Err("internal: bad fmt glue signature".into());
        };
        let t = **t;
        let fty = self.formatter_ty()?;
        let fref = Ty::Ref(true, intern(fty));
        let span = proc_macro2::Span::call_site();
        let str_ref = Ty::Ref(false, intern(Ty::Str));
        let me = if debug { "__exr_debug" } else { "__exr_display" };
        match t {
            t if t.bits() == 128 => {
                let v = self.b.ins().load(types::I128, mf(), x, 0);
                let (mag, neg) = if t.signed() {
                    let z = self.int_const(t, 0);
                    let neg = self.b.ins().icmp(IntCC::SignedLessThan, v, z);
                    let n = self.b.ins().ineg(v);
                    (self.b.ins().select(neg, n, v), neg)
                } else {
                    (v, self.b.ins().iconst(types::I8, 0))
                };
                self.call_named("__exr_fmt_int128", vec![Ty::Unit], &[(f, fref), (mag, Ty::U128), (neg, Ty::Bool)], span)?;
            }
            t if t.is_int() => {
                let ct = clif_ty(t).unwrap();
                let v = self.b.ins().load(ct, mf(), x, 0);
                let v64 = if ct == types::I64 {
                    v
                } else if t.signed() {
                    self.b.ins().sextend(types::I64, v)
                } else {
                    self.b.ins().uextend(types::I64, v)
                };
                let (mag, neg) = if t.signed() {
                    let neg = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, v64, 0);
                    let n = self.b.ins().ineg(v64);
                    (self.b.ins().select(neg, n, v64), neg)
                } else {
                    (v64, self.b.ins().iconst(types::I8, 0))
                };
                self.call_named("__exr_fmt_int", vec![Ty::Unit], &[(f, fref), (mag, Ty::U64), (neg, Ty::Bool)], span)?;
            }
            t if t.is_float() => {
                let v = self.b.ins().load(clif_ty(t).unwrap(), mf(), x, 0);
                let v = if t == Ty::F32 { self.b.ins().fpromote(types::F64, v) } else { v };
                let is32 = self.b.ins().iconst(types::I8, (t == Ty::F32) as i64);
                let dbg = self.b.ins().iconst(types::I8, debug as i64);
                self.call_named("__exr_fmt_float", vec![Ty::Unit], &[(f, fref), (v, Ty::F64), (is32, Ty::Bool), (dbg, Ty::Bool)], span)?;
            }
            Ty::Bool => {
                let v = self.b.ins().load(types::I8, mf(), x, 0);
                let h = if debug { "__exr_fmt_bool" } else { "__exr_fmt_bool" };
                self.call_named(h, vec![Ty::Unit], &[(f, fref), (v, Ty::Bool)], span)?;
            }
            Ty::Char => {
                let v = self.b.ins().load(types::I32, mf(), x, 0);
                let h = if debug { "__exr_fmt_char_debug" } else { "__exr_fmt_char" };
                self.call_named(h, vec![Ty::Unit], &[(f, fref), (v, Ty::Char)], span)?;
            }
            Ty::Unit => {
                let l = self.str_const("()");
                self.call_named("__exr_fmt_lit", vec![Ty::Unit], &[(f, fref), (l, str_ref)], span)?;
            }
            // x: &str (fat のアドレス)
            Ty::Str => {
                let h = if debug { "__exr_fmt_str_debug" } else { "__exr_fmt_str" };
                self.call_named(h, vec![Ty::Unit], &[(f, fref), (x, str_ref)], span)?;
            }
            Ty::Ref(_, Ty::Str) => {
                let h = if debug { "__exr_fmt_str_debug" } else { "__exr_fmt_str" };
                self.call_named(h, vec![Ty::Unit], &[(f, fref), (x, str_ref)], span)?;
            }
            Ty::Ref(_, i) | Ty::Box(i) if !t.is_fat() => {
                let p = self.b.ins().load(PTR, mf(), x, 0);
                self.call_named(me, vec![*i], &[(p, Ty::Ref(false, intern(*i))), (f, fref)], span)?;
            }
            // &[T] / &mut [T]: 要素の列
            Ty::Ref(_, Ty::Slice(e)) | Ty::Box(Ty::Slice(e)) => {
                let p = self.b.ins().load(PTR, mf(), x, 0);
                let n = self.b.ins().load(types::I64, mf(), x, 8);
                self.fmt_list(f, p, n, **e, me)?;
            }
            Ty::Array(e, n) => {
                let n = self.b.ins().iconst(types::I64, n as i64);
                self.fmt_list(f, x, n, *e, me)?;
            }
            Ty::Struct(sid) => {
                let b = self.prog.base_of(sid);
                let tr = if debug { "Debug" } else { "Display" };
                let bname = self.prog.structs[b as usize].name.clone();
                let mname = format!("{bname}::<{tr}>::fmt");
                if let Some(&m) = self.prog.index.get(&mname) {
                    // 手書き (と prelude) の impl
                    let ms = &self.prog.sigs[m as usize];
                    let mut fid = m;
                    if ms.is_generic() {
                        let mut sol = vec![None; ms.generics.len()];
                        if let Some(st) = ms.self_ty {
                            self.prog.match_ty(st, t, &mut sol);
                        }
                        let recv = [Ty::Ref(false, intern(t)), fref];
                        fid = self.resolve_call(m, &recv, sol, None, span)?.0;
                    }
                    let fs = self.insts.lock().unwrap().sig(self.prog, fid).clone();
                    let fr = self.fref(fid, &fs);
                    let mut cv = vec![];
                    if is_sret(&fs) {
                        cv.push(self.temp(fs.ret));
                    }
                    cv.push(x);
                    cv.push(f);
                    self.b.ins().call(fr, &cv);
                } else if debug {
                    self.fmt_derived(sid, x, f)?;
                } else {
                    return Err(format!("`{bname}` doesn't implement `Display`"));
                }
            }
            t => return Err(format!("`{}` cannot be formatted", self.prog.tn(t))),
        }
        self.b.ins().return_(&[]);
        Ok(())
    }

    /// `[a, b, ..]`
    fn fmt_list(&mut self, f: Value, p: Value, n: Value, e: Ty, me: &str) -> R<()> {
        let fty = self.formatter_ty()?;
        let fref = Ty::Ref(true, intern(fty));
        let span = proc_macro2::Span::call_site();
        let str_ref = Ty::Ref(false, intern(Ty::Str));
        let empty = self.str_const("");
        let kind = self.b.ins().iconst(types::I8, 2);
        self.call_named("__exr_dbg_begin", vec![Ty::Unit], &[(f, fref), (empty, str_ref), (kind, Ty::U8)], span)?;
        let (es, _) = self.prog.size_align(e);
        let i = self.b.declare_var(types::I64);
        let z = self.b.ins().iconst(types::I64, 0);
        self.b.def_var(i, z);
        let head = self.b.create_block();
        let body = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().jump(head, &[]);
        self.b.switch_to_block(head);
        let iv = self.b.use_var(i);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, n);
        self.b.ins().brif(more, body, &[], done, &[]);
        self.b.switch_to_block(body);
        let iv = self.b.use_var(i);
        let empty = self.str_const("");
        self.call_named("__exr_dbg_item", vec![Ty::Unit], &[(f, fref), (iv, Ty::Usize), (empty, str_ref)], span)?;
        let off = self.b.ins().imul_imm_s(iv, es as i64);
        let a = self.b.ins().iadd(p, off);
        self.call_named(me, vec![e], &[(a, Ty::Ref(false, intern(e))), (f, fref)], span)?;
        self.call_named("__exr_dbg_item_end", vec![Ty::Unit], &[(f, fref)], span)?;
        let iv = self.b.use_var(i);
        let next = self.b.ins().iadd_imm_s(iv, 1);
        self.b.def_var(i, next);
        self.b.ins().jump(head, &[]);
        self.b.switch_to_block(done);
        let fl = self.b.ins().iconst(types::I8, 0);
        self.call_named("__exr_dbg_end", vec![Ty::Unit], &[(f, fref), (n, Ty::Usize), (kind, Ty::U8), (fl, Ty::Bool)], span)?;
        Ok(())
    }

    /// `#[derive(Debug)]` の struct / enum とタプル
    fn fmt_derived(&mut self, sid: u32, x: Value, f: Value) -> R<()> {
        let fty = self.formatter_ty()?;
        let fref = Ty::Ref(true, intern(fty));
        let span = proc_macro2::Span::call_site();
        let b = self.prog.base_of(sid);
        let sd = &self.prog.structs[sid as usize];
        let tuple = self.prog.is_tuple(b);
        let bname = self.prog.structs[b as usize].name.clone();
        // (名前, フィールド (名前, 型, オフセット), 名前つきか) の候補。enum なら判別値ごと
        let shapes: Vec<(Option<i64>, String, Vec<(String, Ty, u32)>, bool)> = if sd.is_enum {
            sd.variants
                .iter()
                .map(|v| {
                    (
                        Some(v.discr),
                        v.name.clone(),
                        v.fields.clone(),
                        v.kind == crate::front::VariantKind::Struct,
                    )
                })
                .collect()
        } else {
            let named = sd.fields.first().is_some_and(|f| f.0.parse::<usize>().is_err());
            vec![(None, if tuple { String::new() } else { bname }, sd.fields.clone(), named)]
        };
        let tag = if sd.is_enum {
            Some(self.b.ins().load(clif_ty(sd.tag).unwrap(), mf(), x, 0))
        } else {
            None
        };
        let done = self.b.create_block();
        for (discr, name, fields, named) in shapes {
            let next = self.b.create_block();
            if let (Some(d), Some(tag)) = (discr, tag) {
                let yes = self.b.create_block();
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, d);
                self.b.ins().brif(c, yes, &[], next, &[]);
                self.b.switch_to_block(yes);
            }
            let nm = self.str_const(&name);
            let str_ref = Ty::Ref(false, intern(Ty::Str));
            if fields.is_empty() && !tuple {
                self.call_named("__exr_fmt_lit", vec![Ty::Unit], &[(f, fref), (nm, str_ref)], span)?;
            } else {
                let kind = self.b.ins().iconst(types::I8, if named { 0 } else { 1 });
                self.call_named("__exr_dbg_begin", vec![Ty::Unit], &[(f, fref), (nm, str_ref), (kind, Ty::U8)], span)?;
                for (i, (fname, ft, off)) in fields.iter().enumerate() {
                    let iv = self.b.ins().iconst(types::I64, i as i64);
                    let key = self.str_const(if named { fname } else { "" });
                    self.call_named("__exr_dbg_item", vec![Ty::Unit], &[(f, fref), (iv, Ty::Usize), (key, str_ref)], span)?;
                    let a = self.addr_plus(x, *off as i32);
                    self.call_named("__exr_debug", vec![*ft], &[(a, Ty::Ref(false, intern(*ft))), (f, fref)], span)?;
                    self.call_named("__exr_dbg_item_end", vec![Ty::Unit], &[(f, fref)], span)?;
                }
                let n = self.b.ins().iconst(types::I64, fields.len() as i64);
                let tf = self.b.ins().iconst(types::I8, tuple as i64);
                self.call_named("__exr_dbg_end", vec![Ty::Unit], &[(f, fref), (n, Ty::Usize), (kind, Ty::U8), (tf, Ty::Bool)], span)?;
            }
            self.b.ins().jump(done, &[]);
            self.b.switch_to_block(next);
            if discr.is_none() {
                self.b.ins().jump(done, &[]);
                let dead = self.b.create_block();
                self.b.switch_to_block(dead);
            }
        }
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        Ok(())
    }
}

impl Cx<'_, '_> {
    /// `__exr_hash::<T>(x: &T) -> u64` の本体 (整数は掛け算で混ぜる、文字列はバイト列の FNV、
    /// struct / enum / タプル / 配列は要素のハッシュを組み合わせる)
    fn hash_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let x = self.b.block_params(entry)[0];
        let Some(Ty::Ref(_, t)) = sig.params.first() else {
            return Err("internal: bad hash glue signature".into());
        };
        let h = self.hash_at(x, **t)?;
        self.b.ins().return_(&[h]);
        Ok(())
    }

    fn mix(&mut self, h: Value, v: Value) -> Value {
        let a = self.b.ins().imul_imm_s(h, 31);
        let s = self.b.ins().iadd(a, v);
        let m = self.b.ins().imul_imm_s(s, 0x9E3779B97F4A7C15u64 as i64);
        let sh = self.b.ins().ushr_imm_u(m, 29);
        self.b.ins().bxor(m, sh)
    }

    fn hash_call(&mut self, a: Value, t: Ty) -> R<Value> {
        let span = proc_macro2::Span::call_site();
        let (v, _) = self.call_named("__exr_hash", vec![t], &[(a, Ty::Ref(false, intern(t)))], span)?;
        Ok(v.unwrap())
    }

    fn hash_at(&mut self, x: Value, t: Ty) -> R<Value> {
        let span = proc_macro2::Span::call_site();
        let u8s = Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8))));
        Ok(match t {
            t if t.is_int() || matches!(t, Ty::Bool | Ty::Char) => {
                let ct = clif_ty(t).unwrap();
                let v = self.b.ins().load(ct, mf(), x, 0);
                let z = self.b.ins().iconst(types::I64, 0);
                if ct == types::I128 {
                    let (lo, hi) = self.b.ins().isplit(v);
                    let h = self.mix(z, lo);
                    self.mix(h, hi)
                } else {
                    let v = if ct == types::I64 { v } else { self.b.ins().uextend(types::I64, v) };
                    self.mix(z, v)
                }
            }
            Ty::Str => self.call_named("__exr_hash_bytes", vec![Ty::Unit], &[(x, u8s)], span)?.0.unwrap(),
            Ty::Ref(_, Ty::Str) => self.call_named("__exr_hash_bytes", vec![Ty::Unit], &[(x, u8s)], span)?.0.unwrap(),
            Ty::Ref(_, i) | Ty::Box(i) if !t.is_fat() => {
                let p = self.b.ins().load(PTR, mf(), x, 0);
                self.hash_call(p, *i)?
            }
            Ty::Array(e, n) => {
                let (es, _) = self.prog.size_align(*e);
                let mut h = self.b.ins().iconst(types::I64, 0);
                for i in 0..n {
                    let a = self.addr_plus(x, (i * es) as i32);
                    let v = self.hash_call(a, *e)?;
                    h = self.mix(h, v);
                }
                h
            }
            Ty::Struct(sid) => {
                let sd = &self.prog.structs[sid as usize];
                let bname = self.prog.structs[self.prog.base_of(sid) as usize].name.clone();
                // String は中身のバイト列
                if bname == "String" {
                    let (fat, _) = self
                        .deref_to_slice(x, sid, false)
                        .ok_or("internal: bad String")?;
                    return Ok(self.call_named("__exr_hash_bytes", vec![Ty::Unit], &[(fat, u8s)], span)?.0.unwrap());
                }
                let is_enum = sd.is_enum;
                let tag_ty = sd.tag;
                let fields = sd.fields.clone();
                let variants: Vec<(i64, Vec<(String, Ty, u32)>)> =
                    sd.variants.iter().map(|v| (v.discr, v.fields.clone())).collect();
                if is_enum {
                    let tag = self.b.ins().load(clif_ty(tag_ty).unwrap(), mf(), x, 0);
                    let t64 = if clif_ty(tag_ty) == Some(types::I64) {
                        tag
                    } else {
                        self.b.ins().uextend(types::I64, tag)
                    };
                    let z = self.b.ins().iconst(types::I64, 0);
                    let h0 = self.mix(z, t64);
                    let done = self.b.create_block();
                    let res = self.b.append_block_param(done, types::I64);
                    for (d, fs) in variants {
                        let yes = self.b.create_block();
                        let next = self.b.create_block();
                        let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, d);
                        self.b.ins().brif(c, yes, &[], next, &[]);
                        self.b.switch_to_block(yes);
                        let mut h = h0;
                        for (_, ft, off) in fs {
                            let a = self.addr_plus(x, off as i32);
                            let v = self.hash_call(a, ft)?;
                            h = self.mix(h, v);
                        }
                        self.b.ins().jump(done, &[h.into()]);
                        self.b.switch_to_block(next);
                    }
                    self.b.ins().jump(done, &[h0.into()]);
                    self.b.switch_to_block(done);
                    res
                } else {
                    let mut h = self.b.ins().iconst(types::I64, 0);
                    for (_, ft, off) in fields {
                        let a = self.addr_plus(x, off as i32);
                        let v = self.hash_call(a, ft)?;
                        h = self.mix(h, v);
                    }
                    h
                }
            }
            t => return Err(format!("`{}` cannot be used as a hash key", self.prog.tn(t))),
        })
    }
}

// --- 大小比較 (PartialOrd / Ord) ---------------------------------------------

impl Cx<'_, '_> {
    /// 値 v (型 t) を比較の glue に渡せる (アドレス, 指す先の型) にする。
    /// 細い参照は指す先まで辿り、スカラーの値は一時領域に置く
    fn ord_addr(&mut self, v: Value, t: Ty) -> (Value, Ty) {
        let (mut v, mut t) = (v, t);
        loop {
            match t {
                Ty::Ref(_, i) if !t.is_fat() => {
                    if matches!(i, Ty::Ref(..)) && !i.is_fat() {
                        v = self.b.ins().load(PTR, mf(), v, 0);
                        t = *i;
                        continue;
                    }
                    return (v, *i);
                }
                t if t.is_agg() || t.is_fat() => return (v, t),
                t => {
                    let a = self.temp(t);
                    self.b.ins().store(mf(), v, a, 0);
                    return (a, t);
                }
            }
        }
    }

    /// `__exr_cmp::<t>(a, b)` (a, b は t の値のアドレス) → -1 / 0 / 1 の i32
    fn cmp_call(&mut self, a: Value, b: Value, t: Ty) -> R<Value> {
        if t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char | Ty::Unit | Ty::Ptr(..)) {
            return self.cmp_at(a, b, t);
        }
        // glue がなければその場で展開する
        if !self.prog.index.contains_key("__exr_cmp") {
            if self.cmp_depth > 16 {
                return Err(format!("comparison of the recursive type `{}` is not supported", self.prog.tn(t)));
            }
            self.cmp_depth += 1;
            let r = self.cmp_at(a, b, t);
            self.cmp_depth -= 1;
            return r;
        }
        let span = proc_macro2::Span::call_site();
        let r = Ty::Ref(false, intern(t));
        let (v, _) = self.call_named("__exr_cmp", vec![t], &[(a, r), (b, r)], span)?;
        Ok(v.unwrap())
    }

    /// `__exr_cmp::<T>(a: &T, b: &T) -> i32` の本体
    fn cmp_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let a = self.b.block_params(entry)[0];
        let b = self.b.block_params(entry)[1];
        let Some(Ty::Ref(_, t)) = sig.params.first() else {
            return Err("internal: bad cmp glue signature".into());
        };
        let c = self.cmp_at(a, b, **t)?;
        self.b.ins().return_(&[c]);
        Ok(())
    }

    /// `__exr_eq::<T>(a: &T, b: &T) -> bool` の本体
    fn eq_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let a = self.b.block_params(entry)[0];
        let b = self.b.block_params(entry)[1];
        let Some(Ty::Ref(_, t)) = sig.params.first() else {
            return Err("internal: bad eq glue signature".into());
        };
        let span = proc_macro2::Span::call_site();
        let c = self.agg_or_scalar_eq(a, b, **t, span)?;
        self.b.ins().return_(&[c]);
        Ok(())
    }

    /// 2 つのスカラーの大小 (x > y) - (x < y)
    fn cmp_scalar(&mut self, x: Value, y: Value, signed: bool) -> Value {
        let (lt, gt) = if signed {
            (IntCC::SignedLessThan, IntCC::SignedGreaterThan)
        } else {
            (IntCC::UnsignedLessThan, IntCC::UnsignedGreaterThan)
        };
        let l = self.b.ins().icmp(lt, x, y);
        let g = self.b.ins().icmp(gt, x, y);
        let l = self.b.ins().uextend(types::I32, l);
        let g = self.b.ins().uextend(types::I32, g);
        self.b.ins().isub(g, l)
    }

    /// 0 でなければ done へ (その値で)、0 なら次のブロックへ進む
    fn cmp_chain(&mut self, c: Value, done: Block) {
        let next = self.b.create_block();
        self.b.ins().brif(c, done, &[c.into()], next, &[]);
        self.b.switch_to_block(next);
    }

    /// 要素型 e の列 (pa, la) と (pb, lb) を辞書順に比べる
    fn cmp_seq(&mut self, pa: Value, la: Value, pb: Value, lb: Value, e: Ty) -> R<Value> {
        let (es, _) = self.prog.size_align(e);
        let done = self.b.create_block();
        let res = self.b.append_block_param(done, types::I32);
        let head = self.b.create_block();
        let body = self.b.create_block();
        let tail = self.b.create_block();
        let i = self.b.declare_var(types::I64);
        let z = self.b.ins().iconst(types::I64, 0);
        self.b.def_var(i, z);
        let n = self.b.ins().umin(la, lb);
        self.b.ins().jump(head, &[]);
        self.b.switch_to_block(head);
        let iv = self.b.use_var(i);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, n);
        self.b.ins().brif(more, body, &[], tail, &[]);
        self.b.switch_to_block(body);
        let off = self.b.ins().imul_imm_s(iv, es as i64);
        let xa = self.b.ins().iadd(pa, off);
        let xb = self.b.ins().iadd(pb, off);
        let c = self.cmp_call(xa, xb, e)?;
        let nx = self.b.ins().iadd_imm_s(iv, 1);
        self.b.def_var(i, nx);
        self.b.ins().brif(c, done, &[c.into()], head, &[]);
        self.b.switch_to_block(tail);
        let c = self.cmp_scalar(la, lb, false);
        self.b.ins().jump(done, &[c.into()]);
        self.b.switch_to_block(done);
        Ok(res)
    }

    /// a, b (t の値のアドレス) の大小。derive(PartialOrd) と同じく、フィールド・要素を前から辞書順に比べる。
    /// 手書きの partial_cmp / cmp があればそれを呼ぶ
    fn cmp_at(&mut self, a: Value, b: Value, t: Ty) -> R<Value> {
        Ok(match t {
            Ty::Unit => self.b.ins().iconst(types::I32, 0),
            t if t.is_int() || matches!(t, Ty::Bool | Ty::Char | Ty::Ptr(..)) => {
                let ct = clif_ty(t).unwrap();
                let x = self.b.ins().load(ct, mf(), a, 0);
                let y = self.b.ins().load(ct, mf(), b, 0);
                self.cmp_scalar(x, y, t.signed())
            }
            // (NaN はどちらとも等しい扱い: partial_cmp の None の代わり)
            t if t.is_float() => {
                let ct = clif_ty(t).unwrap();
                let x = self.b.ins().load(ct, mf(), a, 0);
                let y = self.b.ins().load(ct, mf(), b, 0);
                let l = self.b.ins().fcmp(FloatCC::LessThan, x, y);
                let g = self.b.ins().fcmp(FloatCC::GreaterThan, x, y);
                let l = self.b.ins().uextend(types::I32, l);
                let g = self.b.ins().uextend(types::I32, g);
                self.b.ins().isub(g, l)
            }
            Ty::Ref(_, Ty::Str | Ty::Slice(_)) => {
                let e = match t {
                    Ty::Ref(_, Ty::Slice(e)) => **e,
                    _ => Ty::U8,
                };
                let pa = self.b.ins().load(PTR, mf(), a, 0);
                let la = self.b.ins().load(types::I64, mf(), a, 8);
                let pb = self.b.ins().load(PTR, mf(), b, 0);
                let lb = self.b.ins().load(types::I64, mf(), b, 8);
                self.cmp_seq(pa, la, pb, lb, e)?
            }
            Ty::Ref(_, i) | Ty::Box(i) if !t.is_fat() => {
                let pa = self.b.ins().load(PTR, mf(), a, 0);
                let pb = self.b.ins().load(PTR, mf(), b, 0);
                self.cmp_call(pa, pb, *i)?
            }
            Ty::Array(e, n) => {
                let l = self.b.ins().iconst(types::I64, n as i64);
                self.cmp_seq(a, l, b, l, *e)?
            }
            Ty::Struct(sid) => {
                // Vec / String は中身の列
                if let Some((fa, ft)) = self.deref_to_slice(a, sid, false) {
                    let (fb, _) = self.deref_to_slice(b, sid, false).unwrap();
                    return self.cmp_at(fa, fb, ft);
                }
                let user = ["partial_cmp", "cmp"]
                    .into_iter()
                    .find(|m| !self.prog.is_tuple(sid) && self.prog.method(sid, m).is_some());
                if let Some(m) = user {
                    // 手書きの impl: `__exr_pord_i32(&__a.partial_cmp(__b))`
                    let r = Ty::Ref(false, intern(t));
                    self.scopes.push(vec![]);
                    self.bind("__exr_ca".into(), r, Some(a));
                    self.bind("__exr_cb".into(), r, Some(b));
                    let e: Expr = if m == "partial_cmp" {
                        syn::parse_quote!(__exr_pord_i32::<()>(&__exr_ca.partial_cmp(__exr_cb)))
                    } else {
                        syn::parse_quote!(__exr_ord_i32::<()>(&__exr_ca.cmp(__exr_cb)))
                    };
                    let (v, _) = self.expr(&e, Some(Ty::I32))?;
                    self.scopes.pop();
                    return Ok(v.unwrap());
                }
                let sd = &self.prog.structs[sid as usize];
                let is_enum = sd.is_enum;
                let tag_ty = sd.tag;
                let fields = sd.fields.clone();
                let variants: Vec<(i64, Vec<(String, Ty, u32)>)> =
                    sd.variants.iter().map(|v| (v.discr, v.fields.clone())).collect();
                let done = self.b.create_block();
                let res = self.b.append_block_param(done, types::I32);
                if is_enum {
                    // バリアントの判別値の順、同じバリアントならフィールドの辞書順
                    let ct = clif_ty(tag_ty).unwrap();
                    let ta = self.b.ins().load(ct, mf(), a, 0);
                    let tb = self.b.ins().load(ct, mf(), b, 0);
                    let c = self.cmp_scalar(ta, tb, true);
                    self.cmp_chain(c, done);
                    for (d, fs) in variants {
                        if fs.is_empty() {
                            continue;
                        }
                        let yes = self.b.create_block();
                        let next = self.b.create_block();
                        let is_v = self.b.ins().icmp_imm_s(IntCC::Equal, ta, d);
                        self.b.ins().brif(is_v, yes, &[], next, &[]);
                        self.b.switch_to_block(yes);
                        for (_, ft, off) in fs {
                            let (fa, fb) = (self.addr_plus(a, off as i32), self.addr_plus(b, off as i32));
                            let c = self.cmp_call(fa, fb, ft)?;
                            self.cmp_chain(c, done);
                        }
                        let z = self.b.ins().iconst(types::I32, 0);
                        self.b.ins().jump(done, &[z.into()]);
                        self.b.switch_to_block(next);
                    }
                } else {
                    for (_, ft, off) in fields {
                        let (fa, fb) = (self.addr_plus(a, off as i32), self.addr_plus(b, off as i32));
                        let c = self.cmp_call(fa, fb, ft)?;
                        self.cmp_chain(c, done);
                    }
                }
                let z = self.b.ins().iconst(types::I32, 0);
                self.b.ins().jump(done, &[z.into()]);
                self.b.switch_to_block(done);
                res
            }
            t => return Err(format!("`{}` cannot be compared with `<`", self.prog.tn(t))),
        })
    }

    /// `a.cmp(&b)` / `a.partial_cmp(&b)` (手書きの impl がない型: 整数・タプル・derive した型など)
    fn ord_method(&mut self, mc: &syn::ExprMethodCall) -> R<Option<V>> {
        let name = mc.method.to_string();
        if !matches!(name.as_str(), "cmp" | "partial_cmp") || mc.args.len() != 1 {
            return Ok(None);
        }
        let helper = if name == "cmp" { "__exr_cmp_m" } else { "__exr_pcmp_m" };
        if !self.prog.index.contains_key(helper) {
            return Ok(None);
        }
        // 手書きの cmp があれば普通のメソッド呼び出し
        if let Some(mut t) = self.peek(&mc.receiver) {
            while let Ty::Ref(_, i) = t {
                t = *i;
            }
            if let Ty::Struct(sid) = t {
                if self.prog.method(sid, &name).is_some() {
                    return Ok(None);
                }
            }
        }
        let (v, t) = self.operand(&mc.receiver, None, true)?;
        let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
        let (ra, rt) = self.ord_addr(v, t);
        // (`Abs(-3).cmp(&b)`: 受け手の型が評価してから分かった手書きの impl は、評価した値でそのメソッドを呼ぶ)
        if let Ty::Struct(sid) = rt {
            if let Some(id) = self.prog.method(sid, &name) {
                let args: Vec<&Expr> = mc.args.iter().collect();
                return self.call_fn(id, Some((ra, Ty::Ref(false, intern(rt)))), &args, None, None, mc.span()).map(Some);
            }
        }
        let arg = &mc.args[0];
        let (av, at) = self.operand(arg, Some(Ty::Ref(false, intern(rt))), true)?;
        let Some(av) = av else { return Ok(Some((None, Ty::Never))) };
        let (aa, at2) = self.ord_addr(av, at);
        if !matches!(at, Ty::Ref(..)) || at2 != rt {
            return err(
                arg.span(),
                format!("mismatched types: expected `&{}`, found `{}`", self.prog.tn(rt), self.prog.tn(at)),
            );
        }
        let r = Ty::Ref(false, intern(rt));
        // (浮動小数点の partial_cmp は NaN で None)
        if name == "partial_cmp" && rt.is_float() && self.prog.index.contains_key("__exr_pcmp_fl") {
            let ct = clif_ty(rt).unwrap();
            let x = self.b.ins().load(ct, mf(), ra, 0);
            let y = self.b.ins().load(ct, mf(), aa, 0);
            return self.call_named("__exr_pcmp_fl", vec![rt], &[(x, rt), (y, rt)], mc.span()).map(Some);
        }
        self.call_named(helper, vec![rt], &[(ra, r), (aa, r)], mc.span()).map(Some)
    }
}

impl Cx<'_, '_> {
    /// 型 t がイテレータ (next を持つ struct) か
    fn is_iterator_ty(&self, t: Ty) -> bool {
        match t {
            Ty::Struct(sid) => self.prog.method(sid, "next").is_some() || self.prog.default_method(sid, "next").is_some(),
            _ => false,
        }
    }

    /// `v.extend(arg)` (Vec / BinaryHeap): IntoIterator の代わりに、イテレータでない arg を
    /// `arg.into_iter()` (値) / `arg.iter()` (参照・配列) にする。要素が `&T` なら __extend_ref (T: Copy)
    fn extend_call(&mut self, mc: &syn::ExprMethodCall, expected: Option<Ty>) -> R<Option<V>> {
        if mc.method != "extend" || mc.args.len() != 1 {
            return Ok(None);
        }
        // (下で作り直した `recv.extend(__exr_ext)` は普通のメソッド呼び出し)
        if matches!(&mc.args[0], Expr::Path(p) if p.path.is_ident("__exr_ext")) {
            return Ok(None);
        }
        let Some(rt) = self.peek(&mc.receiver).map(|t| self.strip_refs(t)) else {
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
        let arg = &mc.args[0];
        let at = self.peek(arg);
        let iter_e: Option<Expr> = match at {
            None => None,
            Some(t) if self.is_iterator_ty(t) => None,
            Some(Ty::Ref(_, Ty::Array(..))) if matches!(strip_paren(arg), Expr::Reference(_)) => {
                Some(syn::parse_quote!(__exr_arr.iter()))
            }
            Some(Ty::Ref(_, _)) => match strip_paren(arg) {
                Expr::Reference(r) => {
                    let inner = &r.expr;
                    Some(syn::parse_quote!((#inner).iter()))
                }
                _ => Some(syn::parse_quote!((#arg).iter())),
            },
            Some(Ty::Array(e, _)) => {
                if self.needs_drop(*e) {
                    return err(arg.span(), "`extend` with an array of values that need drop is not supported");
                }
                Some(syn::parse_quote!(__exr_arr.iter()))
            }
            Some(_) => Some(syn::parse_quote!((#arg).into_iter())),
        };
        self.scopes.push(vec![]);
        // 配列は一度ローカルに置いてから iter() する (リテラルの `[1, 2].iter()` は場所でないため)
        match (at, strip_paren(arg)) {
            (Some(Ty::Array(..)), _) => {
                let (v, t) = self.expr(arg, None)?;
                self.bind("__exr_arr".into(), t, v);
            }
            (Some(Ty::Ref(_, Ty::Array(e, _))), Expr::Reference(r)) => {
                if self.needs_drop(**e) {
                    return err(arg.span(), "`extend` with an array of values that need drop is not supported");
                }
                let (v, t) = self.expr(&r.expr, None)?;
                self.bind("__exr_arr".into(), t, v);
            }
            _ => {}
        }
        let (v, t) = match &iter_e {
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
        let r = &mc.receiver;
        let e: Expr = if by_ref {
            syn::parse_quote!((#r).__extend_ref(__exr_ext))
        } else {
            syn::parse_quote!((#r).extend(__exr_ext))
        };
        let res = self.expr(&e, expected)?;
        self.pop_scope();
        Ok(Some(res))
    }
}

impl Cx<'_, '_> {
    /// break / continue の行き先のループ (ラベルがなければいちばん内側のループ。ラベル付きブロックは飛ばす)
    fn loop_index(&self, label: Option<&str>, is_break: bool) -> Option<usize> {
        self.loops.iter().rposition(|l| match label {
            Some(n) => l.label.as_deref() == Some(n) && (is_break || !l.is_block),
            None => !l.is_block,
        })
    }

    /// `'a: { .. break 'a v; .. }`
    fn labeled_block(&mut self, label: String, blk: &syn::Block, expected: Option<Ty>) -> R<V> {
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
                self.same(blk.span(), t, w)?;
            } else if t != Ty::Unit {
                return err(blk.span(), "a labeled block that breaks without a value must have type `()`");
            }
            let args: Vec<_> = v.into_iter().map(|v| v.into()).collect();
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
fn slice_parts(sp: &syn::PatSlice) -> (Vec<&Pat>, Option<&Pat>, Vec<&Pat>) {
    let mut pre = vec![];
    let mut post = vec![];
    let mut rest = None;
    for e in &sp.elems {
        let is_rest = match e {
            Pat::Rest(_) => true,
            Pat::Ident(pi) => matches!(pi.subpat.as_ref().map(|s| &*s.1), Some(Pat::Rest(_))),
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

impl Cx<'_, '_> {
    /// ローカル変数の置き場所の値 (block の引数で渡す形: 集約型はアドレス)
    fn loc_arg(&mut self, loc: Loc, t: Ty) -> Option<Value> {
        match loc {
            Loc::Var(v) => Some(self.b.use_var(v)),
            Loc::Slot(ss) if t.is_agg() => Some(self.b.ins().stack_addr(PTR, ss, 0)),
            Loc::Slot(ss) => Some(self.b.ins().stack_load(PTR, clif_ty(t).unwrap_or(PTR), ss, 0)),
            Loc::Ptr(v) => Some(v),
            Loc::Zst => None,
        }
    }

    /// block の引数にするときの型 (サイズ 0 なら None)
    fn param_ty(&self, t: Ty) -> Option<types::Type> {
        if t.is_agg() || t.is_fat() {
            Some(PTR)
        } else {
            clif_ty(t)
        }
    }

    /// スライスパターンの対象: (先頭, 長さ, 要素型, 参照越しなら束縛のモード)
    fn slice_scrutinee(&mut self, p: Place, span: proc_macro2::Span) -> R<(Value, Value, Ty, Option<bool>)> {
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
    fn slice_elems<'p>(
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

impl Cx<'_, '_> {
    /// char を UTF-8 にして一時領域に置き、`&str` の fat pointer にする
    fn char_to_str(&mut self, c: Value, span: proc_macro2::Span) -> R<Value> {
        let ss = self.b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 4, 0));
        let buf = self.b.ins().stack_addr(PTR, ss, 0);
        let bt = Ty::Ptr(true, intern(Ty::U8));
        let (n, _) = self.call_named("__exr_char_utf8", vec![Ty::Unit], &[(c, Ty::Char), (buf, bt)], span)?;
        Ok(self.make_fat_v(buf, n.unwrap()))
    }

    /// `recv.name(args)` を prelude の `{prefix}{name}(recv, args)` にする (なければ None)。
    /// 引数の `&str` には char (UTF-8 にする) と `&String` も渡せる
    fn helper_method(&mut self, prefix: &str, name: &str, mc: &syn::ExprMethodCall, recv: (Value, Ty)) -> R<Option<V>> {
        let f = format!("{prefix}{name}");
        let Some(&id) = self.prog.index.get(&f) else { return Ok(None) };
        let sig = self.prog.sigs[id as usize].clone();
        if sig.params.len() != mc.args.len() + 1 {
            return err(mc.span(), format!("`{name}` takes {} arguments", sig.params.len() - 1));
        }
        let args: Vec<&Expr> = mc.args.iter().collect();
        // 本物の型引数を持つもの (`parse::<T>`) は普通の呼び出し
        if !sig.generics.iter().all(|g| g.0 == "G") {
            let exp = self.lets.get(&(mc as *const syn::ExprMethodCall as usize)).copied();
            return self.call_fn(id, Some(recv), &args, mc.turbofish.as_ref(), exp, mc.span()).map(Some);
        }
        let sref = Ty::Ref(false, intern(Ty::Str));
        let mut vals = vec![recv];
        for (a, &pt) in args.iter().zip(&sig.params[1..]) {
            if pt == sref && self.peek(a) == Some(Ty::Char) {
                let (v, _) = self.expr(a, Some(Ty::Char))?;
                let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
                let fat = self.char_to_str(v, a.span())?;
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
            self.same(a.span(), t, pt)?;
            if let Some(v) = v {
                vals.push((v, t));
            }
        }
        self.call_named(&f, vec![Ty::Unit], &vals, mc.span()).map(Some)
    }
}

impl Cx<'_, '_> {
    /// 範囲 `a..b` / `a..=b` / `..b` / `a..` の (lo, hi)
    fn range_bounds(&mut self, rg: &syn::ExprRange, len: Value) -> R<(Value, Value)> {
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
        Ok((lo, hi))
    }

    /// バイト位置 idx (<= len) が UTF-8 の文字の境界か (i8 の 0/1)
    fn char_boundary(&mut self, ptr: Value, len: Value, idx: Value) -> Value {
        let done = self.b.create_block();
        let res = self.b.append_block_param(done, types::I8);
        let load = self.b.create_block();
        let at_end = self.b.ins().icmp(IntCC::Equal, idx, len);
        let one = self.b.ins().iconst(types::I8, 1);
        self.b.ins().brif(at_end, done, &[one.into()], load, &[]);
        self.b.switch_to_block(load);
        let a = self.b.ins().iadd(ptr, idx);
        let byte = self.b.ins().load(types::I8, mf(), a, 0);
        let top = self.b.ins().band_imm_u(byte, 0xC0);
        let ok = self.b.ins().icmp_imm_u(IntCC::NotEqual, top, 0x80);
        self.b.ins().jump(done, &[ok.into()]);
        self.b.switch_to_block(done);
        res
    }
}

// --- clone (derive(Clone) など) ---------------------------------------------------

impl Cx<'_, '_> {
    /// `x.clone()` (自分の clone メソッドがない型): 整数などはそのまま読み、ほかは __exr_clone::<T>
    fn clone_call(&mut self, mc: &syn::ExprMethodCall) -> R<Option<V>> {
        if mc.method != "clone" || !mc.args.is_empty() {
            return Ok(None);
        }
        if let Some(pt) = self.peek(&mc.receiver) {
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
        let (v, vt) = self.operand(&mc.receiver, None, true)?;
        let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
        let (a, t) = self.ord_addr(v, vt);
        // (型が分からずに評価してから、自分の clone を持つ型だったとき)
        if let Ty::Struct(sid) = t {
            if let Some(id) = self.prog.method(sid, "clone") {
                return self.call_fn(id, Some((a, Ty::Ref(false, intern(t)))), &[], None, None, mc.span()).map(Some);
            }
        }
        if !self.needs_drop(t) {
            let p = Place::Mem(a, 0, t);
            return Ok(Some(self.read(p)));
        }
        if !self.prog.index.contains_key("__exr_clone") {
            return err(mc.span(), "internal: `__exr_clone` is missing from the prelude");
        }
        let r = Ty::Ref(false, intern(t));
        self.call_named("__exr_clone", vec![t], &[(a, r)], mc.span()).map(Some)
    }

    /// `__exr_clone::<T>(x: &T) -> T` の本体
    fn clone_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let mut ps = self.b.block_params(entry).to_vec();
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
    fn clone_into(&mut self, src: Value, dst: Value, t: Ty) -> R<()> {
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
                        self.resolve_call(id, &[r], sol, None, proc_macro2::Span::call_site())?
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
                let variants: Vec<(i64, Vec<(String, Ty, u32)>)> =
                    sd.variants.iter().map(|v| (v.discr, v.fields.clone())).collect();
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
    fn clone_elem(&mut self, src: Value, dst: Value, t: Ty) -> R<()> {
        if !self.needs_drop(t) {
            self.copy(dst, src, t);
            return Ok(());
        }
        let r = Ty::Ref(false, intern(t));
        let (v, _) = self.call_named("__exr_clone", vec![t], &[(src, r)], proc_macro2::Span::call_site())?;
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

impl Cx<'_, '_> {
    /// `Default::default()` / `T::default()` / `i32::default()` / `Vec::default()` (手書きの default がない型)
    fn default_call(&mut self, c: &syn::ExprCall, expected: Option<Ty>) -> R<Option<V>> {
        let Expr::Path(p) = &*c.func else { return Ok(None) };
        if !c.args.is_empty() || p.path.segments.last().map(|s| s.ident != "default").unwrap_or(true) {
            return Ok(None);
        }
        if !self.prog.index.contains_key("__exr_default") {
            return Ok(None);
        }
        // `X::default()` で X 自身の default がある (手書きの impl Default for X / 自分の関数) ならそれを呼ぶ
        if p.qself.is_none() && p.path.segments.len() == 2 && p.path.segments[0].ident != "Default" {
            if let Some(id) = self.prog.fn_by_path(&p.path, self.self_sid) {
                if self.prog.sigs[id as usize].has_body {
                    return Ok(None);
                }
            }
            if self.callee(&c.func).is_some_and(|(id, _)| self.prog.sigs[id as usize].has_body) {
                return Ok(None);
            }
        }
        let exp = expected.or_else(|| self.lets.get(&(c as *const syn::ExprCall as usize)).copied());
        let t = if let Some(q) = &p.qself {
            self.prog.parse_ty(&q.ty, &self.env).ok()
        } else {
            let segs: Vec<String> = p.path.segments.iter().map(|s| s.ident.to_string()).collect();
            match segs.as_slice() {
                [d, _] if d == "Default" => exp,
                [x, _] => {
                    let ty: syn::Type = syn::Type::Path(syn::TypePath {
                        qself: None,
                        path: syn::Path::from(p.path.segments[0].ident.clone()),
                    });
                    match self.prog.parse_ty(&ty, &self.env) {
                        Ok(t) => Some(t),
                        // ジェネリックな型の名前だけ (`Vec::default()`): 期待型から
                        Err(_) => exp.filter(|&e| matches!(e, Ty::Struct(sid) if self.prog.structs[self.prog.base_of(sid) as usize].name == *x)),
                    }
                }
                [_, _, ..] if segs.len() >= 3 && segs[segs.len() - 2] == "Default" => exp,
                _ => None,
            }
        };
        let Some(t) = t else {
            return err(c.span(), "cannot infer the type of `default()`; annotate it");
        };
        self.call_named("__exr_default", vec![t], &[], c.span()).map(Some)
    }

    /// `__exr_default::<T>() -> T` の本体
    fn default_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let ps = self.b.block_params(entry).to_vec();
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
    fn default_into(&mut self, dst: Value, t: Ty) -> R<()> {
        // 手書きの `impl Default for S`
        if let Ty::Struct(sid) = t {
            let bname = self.prog.structs[self.prog.base_of(sid) as usize].name.clone();
            if let Some(&id) = self.prog.index.get(&format!("{bname}::default")) {
                let sig0 = self.prog.sigs[id as usize].clone();
                let (fid, sig) = if sig0.is_generic() {
                    let mut sol = vec![None; sig0.generics.len()];
                    self.prog.match_ty(sig0.ret, t, &mut sol);
                    self.resolve_call(id, &[], sol, Some(t), proc_macro2::Span::call_site())?
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

impl Cx<'_, '_> {
    /// const は式そのものを埋め込む
    fn const_value(&mut self, p: &syn::ExprPath, ci: u32) -> R<V> {
        let c = &self.prog.consts[ci as usize];
        let (t, src) = (c.ty, c.expr.clone());
        let ce: Expr = syn::parse_str(&src).or_else(|er| err(p.span(), er))?;
        let (v, vt) = self.expr(&ce, Some(t)).map_err(|e| {
            format!("{}:1: in constant `{}`: {e}", p.span().start().line, c.name)
        })?;
        self.same(p.span(), vt, t)?;
        Ok((v, t))
    }
}

// --- 浮動小数点のメソッド ---------------------------------------------------------

impl Cx<'_, '_> {
    /// f32 / f64 の組み込みメソッド (命令になるものは命令、ほかは libm)
    fn float_method(&mut self, mc: &syn::ExprMethodCall, t: Ty) -> R<Option<V>> {
        let name = mc.method.to_string();
        let _ = clif_ty(t);
        let f32 = t == Ty::F32;
        let lib1 = [
            "exp", "exp2", "ln", "log10", "log2", "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh",
            "tanh", "asinh", "acosh", "atanh", "cbrt", "round", "exp_m1", "ln_1p",
        ];
        let lib2 = ["powf", "atan2", "hypot"];
        let known = [
            "sqrt", "abs", "floor", "ceil", "trunc", "fract", "mul_add", "copysign", "min", "max", "is_nan",
            "is_finite", "is_infinite", "is_sign_negative", "is_sign_positive", "signum", "to_bits", "recip",
            "to_degrees", "to_radians", "clamp", "powi", "log", "total_cmp", "abs_sub", "is_normal",
        ];
        if !(known.contains(&name.as_str()) || lib1.contains(&name.as_str()) || lib2.contains(&name.as_str())) {
            return Ok(None);
        }
        // 受け手 (参照なら中身)
        let (a, at) = if matches!(self.peek(&mc.receiver), Some(Ty::Ref(..))) {
            self.expr(&mc.receiver, None)?
        } else {
            self.expr(&mc.receiver, Some(t))?
        };
        let Some(mut a) = a else { return Ok(Some((None, Ty::Never))) };
        let mut at = at;
        while let Ty::Ref(_, i) = at {
            a = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), a, 0);
            at = *i;
        }
        let args: Vec<&Expr> = mc.args.iter().collect();
        let arg = |this: &mut Self, i: usize, want: Ty| -> R<Value> {
            let Some(e) = args.get(i) else {
                return err(mc.span(), format!("`{name}` needs an argument"));
            };
            let (v, vt) = this.expr(e, Some(want))?;
            let mut v = v.unwrap();
            let mut vt = vt;
            while let Ty::Ref(_, i) = vt {
                v = this.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), v, 0);
                vt = *i;
            }
            this.same(e.span(), vt, want)?;
            Ok(v)
        };
        let span = mc.span();
        let libm = |this: &mut Self, f: &str, vals: Vec<Value>| -> R<Value> {
            let n = format!("__exr_{f}{}", if f32 { "f" } else { "" });
            let a: Vec<(Value, Ty)> = vals.into_iter().map(|v| (v, t)).collect();
            let (r, _) = this.call_named(&n, vec![], &a, span)?;
            Ok(r.unwrap())
        };
        let fc = |this: &mut Self, x: f64| -> Value {
            if f32 { this.b.ins().f32const(x as f32) } else { this.b.ins().f64const(x) }
        };
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

impl Cx<'_, '_> {
    /// `use std::f64::consts::PI;` で取り込んだ定数の名前 (ローカル変数でも const でもないとき)
    fn float_const_name(&self, p: &syn::ExprPath) -> Option<f64> {
        use std::f64::consts as c;
        let id = p.path.get_ident()?;
        // (速い判定から: 名前は大文字だけ)
        let n = id.to_string();
        if !n.bytes().all(|b| b.is_ascii_uppercase() || b == b'_' || b.is_ascii_digit()) || n.len() < 1 {
            return None;
        }
        if self.lookup(&n).is_some() || self.prog.const_index.contains_key(&n) {
            return None;
        }
        Some(match n.as_str() {
            "PI" => c::PI,
            "E" => c::E,
            "TAU" => c::TAU,
            "SQRT_2" => c::SQRT_2,
            "LN_2" => c::LN_2,
            "LN_10" => c::LN_10,
            "FRAC_PI_2" => c::FRAC_PI_2,
            "FRAC_PI_3" => c::FRAC_PI_3,
            "FRAC_PI_4" => c::FRAC_PI_4,
            "FRAC_PI_6" => c::FRAC_PI_6,
            "FRAC_1_SQRT_2" => c::FRAC_1_SQRT_2,
            "LOG2_E" => c::LOG2_E,
            "LOG10_E" => c::LOG10_E,
            _ => return None,
        })
    }
}

/// (`-1` の式全体のアドレス: 型推論はリテラルでなく単項演算の式を記録していることがある)
fn e_addr_of_unary(u: &syn::ExprUnary) -> usize {
    u as *const syn::ExprUnary as usize
}
