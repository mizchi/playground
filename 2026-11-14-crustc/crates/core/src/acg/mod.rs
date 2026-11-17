//! codegen.rs (syn の構文木 → Cranelift) を、自前の構文木 (src/parse.rs) → minclif (src/minclif.rs) に移したもの。
//! Cranelift と同じ形の API は clif.rs が minclif の上に作る。syn の構文木の代わりは parse.rs の構文木、
//! syn 版の型推論 (infer) の代わりは ast_infer。外部クレートに依存しないので crustc 自身でもコンパイルできる。
//!
//! 値の表現は codegen.rs と同じ:
//! - 整数 / bool / 参照: 値 (参照はポインタ)
//! - struct / enum / 配列 / fat pointer: メモリ上にあり「値」はそのアドレス。let / 代入 / 引数 / return でコピーする。
//!   引数は呼び出し側が一時領域にコピーしてそのアドレスを渡し、戻り値は隠れ第 1 引数 (sret) の場所に書く。
//! - ローカル変数: アドレスを取られないスカラーは Variable、それ以外はスタックスロット。
//!
//! 出力は Blob (機械語 + シンボリックな再配置)。呼び出し先などは名前 `\x01f<id>` のように番号で書き、
//! Blob を作るときに Target に直す。
//!
//! 移植の決まり (codegen.rs との対応):
//! - syn の Span は使わない (Sp = u32 の 0)。エラーの場所は今は出さない
//! - syn の個別のノード (`&syn::ExprMethodCall` など) を受けていたメソッドは、式全体 (`&Expr`) を受けて中で分解する

#![allow(clippy::too_many_arguments)]

pub mod clif;
pub mod lit;
mod p1_expr;
mod p2_call;
mod p3_ops;
mod p4_methods;
mod p5_loops_macros;
mod p6_match_drop;
mod p7_fmt_hash_cmp;
mod p8_misc;
mod p9_inline;
pub use p9_inline::{inline_candidate, ClBackend, Inline, MAX_TOKENS};
#[allow(unused_imports)]
pub use p1_expr::*;
#[allow(unused_imports)]
pub use p2_call::*;
#[allow(unused_imports)]
pub use p3_ops::*;
#[allow(unused_imports)]
pub use p4_methods::*;
#[allow(unused_imports)]
pub use p5_loops_macros::*;
#[allow(unused_imports)]
pub use p6_match_drop::*;
#[allow(unused_imports)]
pub use p7_fmt_hash_cmp::*;
#[allow(unused_imports)]
pub use p8_misc::*;

use crate::fxhash::{FxMap as HashMap, FxSet as HashSet};
use std::sync::Mutex;

use crate::acg::clif::{AbiParam, FuncRef, FunctionBuilder, Signature, TrapCode, types};
use crate::minclif::{Block, IntCC, StackSlot, Type, Value, Variable};
pub use crate::minclif::FloatCC;
use lit::Lit;

use crate::parse::{Expr, Pat, Path, Stmt};
use crate::program::{FnSig, Insts, PPath, Program, TyEnv};
use crate::types::{Ty, intern};

pub type R<T> = Result<T, String>;

/// ポインタ型 (x86-64 前提)
pub const PTR: Type = types::I64;

/// エラーの場所 (今は使わない)
/// ソース上の位置 (アイテムの中の行・桁)。分からないところは SP (0 行目)
pub type Sp = crate::parse::Pos;
pub const SP: Sp = crate::parse::Pos { line: 0, col: 0 };

pub fn err<T>(_: Sp, msg: impl std::fmt::Display) -> R<T> {
    // (ACG_PANIC_ERR=1: エラーの場所をバックトレースで調べる)
    if std::env::var_os("ACG_PANIC_ERR").is_some() {
        panic!("{msg}");
    }
    Err(msg.to_string())
}

pub fn mf() -> clif::MemFlagsData {
    clif::MemFlagsData::trusted()
}

// ---------------------------------------------------------------------------
// 出力形式 (codegen.rs の Blob と同じ)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Blob {
    pub align: u32,
    pub code: Vec<u8>,
    pub relocs: Vec<BlobReloc>,
    pub datas: Vec<Vec<u8>>,
    pub vtables: Vec<Vec<u32>>,
    pub traps: Vec<(u32, u32, u32)>,
    pub line_base: Option<u32>,
    pub code_ref: Option<(u32, u64)>,
}

#[derive(Clone, Debug)]
pub struct BlobReloc {
    pub offset: u32,
    /// codegen::RELOC_KINDS の番号 (4 = X86CallPLTRel4, 5 = X86GOTPCRel4)
    pub kind: u8,
    pub addend: i64,
    pub target: Target,
}

#[derive(Clone, Debug)]
pub enum Target {
    Func(u32),
    SelfOffset(u32),
    Data(u32),
    Static(u32),
    Vtable(u32),
    /// 外部の名前 (libgcc の `__multi3` など)
    Sym(String),
}

/// 再配置の名前 (`\x01f12` / `\x01d0` / `\x01s3` / `\x01v1`)
fn sym_name(kind: char, i: u32) -> String {
    format!("\x01{kind}{i}")
}

fn target_of(name: &str) -> Option<Target> {
    let r = name.strip_prefix('\x01')?;
    let (k, n) = r.split_at(1);
    let i: u32 = n.parse().ok()?;
    Some(match k {
        "f" => Target::Func(i),
        "d" => Target::Data(i),
        "s" => Target::Static(i),
        "v" => Target::Vtable(i),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// シグネチャ
// ---------------------------------------------------------------------------

pub fn clif_ty(t: Ty) -> Option<Type> {
    Some(match t {
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
    AbiParam::new(clif_ty(t).unwrap())
}

pub fn is_sret(s: &FnSig) -> bool {
    s.ret.is_agg() && !s.is_main
}

pub fn clif_sig(s: &FnSig) -> Signature {
    let mut sig = Signature::default();
    if is_sret(s) {
        sig.params.push(AbiParam::new(PTR));
    }
    for &p in &s.params {
        if !p.is_agg() && clif_ty(p).is_none() {
            continue;
        }
        sig.params.push(abi_param(p));
    }
    if s.is_main {
        sig.returns.push(AbiParam::new(types::I32));
    } else if !is_sret(s) && clif_ty(s.ret).is_some() {
        sig.returns.push(abi_param(s.ret));
    }
    sig
}

// ---------------------------------------------------------------------------
// 関数のコンパイル
// ---------------------------------------------------------------------------

/// 関数 1 個 (自前の構文木)
#[derive(Clone, Debug)]
pub struct ItemFn {
    pub attrs: Vec<String>,
    pub sig: crate::parse::FnSig,
    pub block: crate::parse::Block,
}

/// panic の trap の最初のコード (1〜4 はほかの trap)。関数ごとに 245 か所まで場所を覚える
pub const TRAP_SITE_BASE: u8 = 5;
pub const TRAP_SITE_MAX: usize = 245;

/// 1 関数をコンパイルする。返り値は (Blob, 要求したインスタンス)
pub fn compile(prog: &Program, insts: &Mutex<Insts>, id: u32, f: &ItemFn, line_base: u32, inl: &Inline) -> R<(Blob, Vec<u32>)> {
    let (sig, env, body_id) = {
        let g = insts.lock().unwrap();
        (g.sig(prog, id).clone(), g.env(prog, id), g.body_of(id))
    };
    // クロージャの本体: キャプチャ (名前, 変数の型, by-ref か, オフセット)
    let mut caps: Vec<(String, Ty, bool, u32)> = vec![];
    if let Some(cd) = prog.closures.get(&body_id) {
        if let Some(Ty::Ptr(_, Ty::Struct(sid))) = sig.params.first() {
            let sd = &prog.structs[*sid as usize];
            for (i, (n, (_, ft, off))) in cd.captures.iter().zip(&sd.fields).enumerate() {
                match (cd.moved.get(i).copied().unwrap_or(false), ft) {
                    (false, Ty::Ptr(_, t)) => caps.push((n.clone(), **t, true, *off)),
                    _ => caps.push((n.clone(), *ft, false, *off)),
                }
            }
        }
    }
    let (root, base_k) = match prog.closures.get(&body_id) {
        Some(cd) => (cd.parent, cd.k + 1),
        None => (body_id, 0),
    };
    let mut closure_ix = HashMap::default();
    let mut taken = addr_taken(&f.block);
    for (i, (a, t)) in crate::ast_closure::closure_slots(&f.block).into_iter().enumerate() {
        let k = base_k + i as u32;
        // (本体に書かれたクロージャはアドレスで、マクロの引数の中のものは式の文字列で引く)
        // (複製された式のために文字列でも引けるようにする: 同じ文字列なら最初のもの)
        if let Some(a) = a {
            closure_ix.insert(format!("@{a}"), k);
        }
        closure_ix.entry(t).or_insert(k);
        if let Some(cd) = prog.closure_by.get(&(root, k)).and_then(|i| prog.closures.get(i)) {
            for (i, n) in cd.captures.iter().enumerate() {
                if !cd.moved.get(i).copied().unwrap_or(false) {
                    taken.insert(n.clone());
                }
            }
        }
    }
    let extra: Vec<(String, Ty)> = caps.iter().map(|c| (c.0.clone(), c.1)).collect();
    let user_code = prog.item_of.get(body_id as usize).copied().flatten().is_some_and(|i| i < prog.prelude_from);
    let ((lets, _, _), margs, texts) = crate::ast_infer::infer_ctx_margs(prog, f.sig.recv.is_some(), &f.sig.params, &f.block, &sig, &env, &extra, Some(sig.ret), Some((root, base_k)));
    let mut cx = Cx {
        b: FunctionBuilder::new(&clif_sig(&sig)),
        prog,
        scopes: vec![vec![]],
        loops: vec![],
        ret: sig.ret,
        is_main: sig.is_main,
        user_code,
        overflow_checks: prog.overflow_checks && (user_code || f.attrs.iter().any(|a| a.contains("rustc_inherit_overflow_checks"))),
        traps: vec![],
        line_base,
        loc_used: false,
        index_panic_block: None,
        caller_loc: matches!(
            prog.names.get(body_id as usize).map(|n| n.as_str()),
            Some("Option::unwrap" | "Option::expect" | "Result::unwrap" | "Result::expect" | "Result::unwrap_err" | "Result::expect_err")
        ) && prog.item_of.get(body_id as usize).copied().flatten().is_some_and(|i| i >= prog.prelude_from),
        self_sid: env.self_sid,
        sret: None,
        frefs: HashMap::default(),
        lets,
        addr_taken: taken,
        caps,
        root,
        closure_ix,
        closure_hint: None,
        seg_self: None,
        loop_bounds: vec![],
        pending_bound: None,
        pending_pair: None,
        checked: (None, vec![]),
        hoisted: vec![],
        in_fmt_args: 0,
        late_flags: vec![],
        owned: vec![],
        temps: vec![],
        owned_binds: 0,
        last_scrut: None,
        env,
        insts,
        requested: vec![],
        datas: vec![],
        vtables: vec![],
        cmp_depth: 0,
        pending_label: None,
        keep: vec![],
        margs,
        texts,
        mbase: Default::default(),
        mstart: Default::default(),
        inl,
        inline_ret: None,
        inline_depth: 0,
        inlined: 0,
        try_into_hint: None,
    };
    let r = if prog.index.get(crate::prelude::DROP_GLUE) == Some(&body_id) {
        cx.drop_glue_body(&sig)
    } else if prog.index.get("__exr_hash") == Some(&body_id) {
        cx.hash_glue_body(&sig)
    } else if prog.index.get("__exr_cmp") == Some(&body_id) {
        cx.cmp_glue_body(&sig)
    } else if prog.index.get("__exr_eq") == Some(&body_id) {
        cx.eq_glue_body(&sig)
    } else if prog.index.get("__exr_clone") == Some(&body_id) {
        cx.clone_glue_body(&sig)
    } else if prog.index.get("__exr_default") == Some(&body_id) {
        cx.default_glue_body(&sig)
    } else if prog.index.get("__exr_display") == Some(&body_id) {
        cx.fmt_glue_body(&sig, false)
    } else if prog.index.get("__exr_debug") == Some(&body_id) {
        cx.fmt_glue_body(&sig, true)
    } else {
        cx.body(f, &sig)
    };
    r?;
    // (入口以外で作った drop フラグは入口で 0 にする: minclif はフラグのスロットを関数の最初に 0 にする)
    let late = std::mem::take(&mut cx.late_flags);
    let requested = std::mem::take(&mut cx.requested);
    let datas = std::mem::take(&mut cx.datas);
    let vtables = std::mem::take(&mut cx.vtables);
    let infos = std::mem::take(&mut cx.traps);
    let loc_used = cx.loc_used;
    let mut b = cx.b;
    let (code, relocs, traps_raw) = match inl.cl {
        // (Cranelift のバックエンド: 記録した命令の列を Cranelift の IR にしてコンパイルする)
        Some(cl) => cl(&b.m, &late, prog.opt)?,
        None => {
            b.set_zero(&late);
            let t = b.traps();
            let (c, r) = b.finish_zeroing(&late);
            (c, r, t)
        }
    };
    let mut out = vec![];
    for r in relocs {
        let target = target_of(&r.name).unwrap_or_else(|| Target::Sym(r.name.clone()));
        // (ELF の種類 → Blob の番号: PLT32 4、GOTPCREL 5、PC32 6、64 7)
        let kind = match r.kind {
            9 => 5,
            2 => 6,
            1 => 7,
            _ => 4,
        };
        out.push(BlobReloc { offset: r.offset, kind, addend: r.addend, target });
    }
    let mut traps = vec![];
    for (off, c) in traps_raw {
        let c = c as usize;
        if (TRAP_SITE_BASE as usize..TRAP_SITE_BASE as usize + infos.len()).contains(&c) && off < 1 << 20 {
            let (info, msg) = infos[c - TRAP_SITE_BASE as usize];
            traps.push((off, info, msg));
        }
    }
    Ok((
        Blob { align: 16, code, relocs: out, datas, vtables, traps, line_base: loc_used.then_some(line_base), code_ref: None },
        requested,
    ))
}

/// `&x` / `&mut x.f` などでアドレスを取られるローカル変数名 (shadowing は無視した過大近似)
fn addr_taken(b: &crate::parse::Block) -> HashSet<String> {
    fn root(e: &Expr) -> Option<String> {
        match e {
            Expr::Path(None, p) => ident_of(p).map(|s| s.to_string()),
            Expr::Field(b, _, _) => root(b),
            Expr::Paren(x) => root(x),
            _ => None,
        }
    }
    fn visit(e: &Expr, out: &mut HashSet<String>) {
        match e {
            Expr::Ref(_, x) => {
                if let Some(n) = root(x) {
                    out.insert(n);
                }
            }
            Expr::Macro(p, g, _) => {
                let name = p.segs.last().map(|s| s.name.as_str()).unwrap_or("");
                if matches!(name, "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne") {
                    if let Some(es) = crate::ast_closure::macro_args(g) {
                        for e in es.iter().take(2) {
                            if let Some(n) = root(e) {
                                out.insert(n);
                            }
                        }
                    }
                }
                let es = crate::ast_closure::macro_exprs(g);
                if es.is_empty() {
                    if let Some(es) = crate::ast_closure::macro_args(g) {
                        for e in &es {
                            match e {
                                Expr::Ref(_, x) => visit(x, out),
                                Expr::Path(..) | Expr::Lit(_, _) => {}
                                e => visit(e, out),
                            }
                        }
                    }
                }
                for e in &es {
                    visit(e, out);
                }
                return;
            }
            _ => {}
        }
        crate::ast_closure::each_child_pub(e, &mut |c| visit(c, out));
    }
    let mut out = HashSet::default();
    let blk = Expr::Block(None, false, b.clone());
    visit(&blk, &mut out);
    out
}

/// エラーの場所にする位置: メソッド呼び出しはメソッド名 (rustc・syn 版と同じ)、ほかは式の先頭
pub fn err_pos(e: &Expr) -> Sp {
    match e {
        Expr::MethodCall(_, _, _, _, s) | Expr::Field(_, _, s) => s.at,
        e => expr_start(e),
    }
}

/// 式の先頭のソース上の位置 (syn の `e.span()` の始まり)。位置を持つ式か、その一番左の子から。分からなければ SP
pub fn expr_start(e: &Expr) -> Sp {
    match e {
        Expr::Unary(_, _, s) | Expr::Binary(_, _, _, s) | Expr::AssignOp(_, _, _, s) | Expr::MethodCall(_, _, _, _, s) | Expr::Index(_, _, s) | Expr::Macro(_, _, s) => s.start,
        Expr::Field(_, _, s) => s.start,
        Expr::Call(x, _) | Expr::Cast(x, _) | Expr::Try(x) | Expr::Paren(x) | Expr::Assign(x, _) => expr_start(x),
        Expr::Range(Some(x), _, _) => expr_start(x),
        Expr::Path(_, p) | Expr::Struct(p, _, _) => p.pos,
        Expr::Lit(_, p) => *p,
        _ => SP,
    }
}

/// メッセージが `行:桁: ` で始まるか
pub fn has_loc(m: &str) -> bool {
    // (数字の並び `:` 数字の並び `:` で始まる)
    let b = m.as_bytes();
    let mut i = 0;
    for _ in 0..2 {
        let st = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == st || i >= b.len() || b[i] != b':' {
            return false;
        }
        i += 1;
    }
    true
}

/// 1 つの名前だけのパス (`x`)
pub fn ident_of(p: &Path) -> Option<&str> {
    if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() && p.segs[0].fn_args.is_none() {
        Some(&p.segs[0].name)
    } else {
        None
    }
}

/// パスが 1 つの名前 n か (`p.path.is_ident(n)`)
pub fn is_ident(p: &Path, n: &str) -> bool {
    ident_of(p) == Some(n)
}

/// 名前だけのパス
pub fn path_of(n: &str) -> Path {
    Path { global: false, segs: vec![crate::parse::Seg { name: n.to_string(), args: vec![], fn_args: None, text: n.to_string() }], pos: Default::default() }
}

/// 名前だけのパスの式 (`x`)
pub fn path_expr(n: &str) -> Expr {
    Expr::Path(None, path_of(n))
}

/// メソッド呼び出しの式を作る
pub fn mcall(recv: Expr, name: &str, args: Vec<Expr>) -> Expr {
    Expr::MethodCall(Box::new(recv), name.to_string(), vec![], args, crate::parse::Span::default())
}

/// 添字の検査を覚えるときの長さ
#[derive(Clone, Copy, PartialEq)]
pub enum LenKey {
    Const(i64),
    Val(Value),
}

/// 式の評価結果。Unit / Never のとき値は None。struct のときはアドレス。
pub type V = (Option<Value>, Ty);

#[derive(Clone, Copy, PartialEq)]
pub enum Loc {
    Var(Variable),
    Slot(StackSlot),
    Ptr(Value),
    Zst,
}

#[derive(Clone, Copy)]
pub enum Place {
    Var(Variable, Ty),
    Mem(Value, i32, Ty),
    Zst(Ty),
}

impl Place {
    pub fn ty(&self) -> Ty {
        match *self {
            Place::Var(_, t) | Place::Mem(_, _, t) | Place::Zst(t) => t,
        }
    }
}

pub struct Loop {
    pub cont: Block,
    pub brk: Block,
    pub breaks: usize,
    pub label: Option<String>,
    pub val: Option<Ty>,
    pub is_block: bool,
    pub depth: usize,
    pub temps: usize,
}

#[derive(Clone, Copy)]
pub struct Owned {
    pub loc: Loc,
    pub ty: Ty,
    pub flag: StackSlot,
    pub depth: usize,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BindMode {
    /// 値として束縛 (struct / enum はコピー)
    Move,
    /// 参照を match したときのデフォルト束縛モード: フィールドへの参照として束縛
    Ref(bool),
}

pub struct Cx<'p> {
    pub b: FunctionBuilder,
    pub prog: &'p Program,
    pub scopes: Vec<Vec<(String, Loc, Ty)>>,
    pub loops: Vec<Loop>,
    pub pending_label: Option<String>,
    pub ret: Ty,
    pub is_main: bool,
    pub user_code: bool,
    pub overflow_checks: bool,
    pub traps: Vec<(u32, u32)>,
    pub line_base: u32,
    pub loc_used: bool,
    pub index_panic_block: Option<Block>,
    pub caller_loc: bool,
    pub self_sid: Option<u32>,
    pub sret: Option<Value>,
    pub frefs: HashMap<u32, FuncRef>,
    pub lets: crate::ast_infer::LetTypes,
    pub addr_taken: HashSet<String>,
    pub env: TyEnv,
    pub insts: &'p Mutex<Insts>,
    pub requested: Vec<u32>,
    pub datas: Vec<Vec<u8>>,
    pub vtables: Vec<Vec<u32>>,
    pub cmp_depth: u32,
    pub caps: Vec<(String, Ty, bool, u32)>,
    pub root: u32,
    /// クロージャ式 (Debug の文字列) → 番号
    pub closure_ix: HashMap<String, u32>,
    pub closure_hint: Option<(Vec<Ty>, Option<Ty>)>,
    pub seg_self: Option<Ty>,
    pub loop_bounds: Vec<(Loc, u64)>,
    pub pending_bound: Option<u64>,
    pub pending_pair: Option<(Value, Option<Value>, Ty)>,
    pub checked: (Option<Block>, Vec<(Value, LenKey)>),
    pub hoisted: Vec<(String, Value, Value, Ty, Option<(Value, Value, Value)>)>,
    pub in_fmt_args: u32,
    pub late_flags: Vec<StackSlot>,
    pub owned: Vec<Owned>,
    pub temps: Vec<Owned>,
    pub owned_binds: usize,
    pub last_scrut: Option<StackSlot>,
    /// 書き換えで作った式 (コード生成の間アドレスを動かさないために持っておく)
    pub keep: Vec<Box<Expr>>,
    /// 型推論が読んだマクロの引数 (同じ構文木を使うと、型の記録をアドレスで引ける)
    pub margs: crate::ast_infer::MacroArgs,
    /// 式の形 → 型 (複製した式の型を引く)
    pub texts: crate::ast_infer::TextTypes,
    /// 今コード生成しているマクロの引数 (開き括弧) のソース上の位置 (引数を読み直すときの位置の基準)
    pub mbase: crate::parse::Pos,
    /// 今コード生成しているマクロの先頭 (syn の `mac.span()`: panic! / assert! の場所)
    pub mstart: crate::parse::Pos,
    /// 展開してよい関数の本体 (p9_inline)
    pub inl: &'p Inline,
    /// 展開している本体の続きのブロック (return / 末尾の値はここへ飛ぶ)
    pub inline_ret: Option<Block>,
    pub inline_depth: u32,
    pub inlined: u32,
    /// `x.try_into().unwrap()` の unwrap の期待型 (型推論が try_into の行き先を記録していないとき)
    pub try_into_hint: Option<Ty>,
}

/// 間接呼び出しの呼び先
#[derive(Clone, Copy)]
pub enum Indirect {
    Vtable(Value, usize),
    Ptr(Value),
}

/// 静的データに昇格できる定数式か
pub fn is_promotable(cx: &Cx, e: &Expr) -> bool {
    match e {
        Expr::Lit(l, _) => !matches!(lit::lit(l), Some(Lit::Str(_) | Lit::ByteStr(_))),
        Expr::Paren(p) => is_promotable(cx, p),
        Expr::Unary(op, x, _) => op == "-" && is_promotable(cx, x),
        Expr::Struct(_, fs, rest) => rest.is_none() && fs.iter().all(|f| is_promotable(cx, &f.1)),
        Expr::Tuple(t) => t.iter().all(|x| is_promotable(cx, x)),
        Expr::Array(a) => a.iter().all(|x| is_promotable(cx, x)),
        Expr::Call(f, args) => {
            matches!(&**f, Expr::Path(None, p)
                if cx.prog.variant_by_p(&PPath::of(p), cx.self_sid).is_some()
                    || cx.prog.struct_by_p(&PPath::of(p), cx.self_sid).is_some())
                && args.iter().all(|x| is_promotable(cx, x))
        }
        Expr::Path(_, p) => {
            cx.const_of(e).is_some() || (cx.prog.variant_by_p(&PPath::of(p), cx.self_sid).is_some() && ident_of(p).is_none_or(|i| cx.lookup(i).is_none()))
        }
        _ => false,
    }
}

/// `_` の穴を含む型か
pub fn has_hole(prog: &Program, t: Ty) -> bool {
    match t {
        Ty::Param(i) => i == crate::program::HOLE,
        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => has_hole(prog, *i),
        Ty::Struct(sid) => prog.structs[sid as usize].args.iter().any(|&a| has_hole(prog, a)),
        _ => false,
    }
}

/// `Iterator<Item = T>` の (Item, T)
pub fn assoc_constraint(b: &str) -> Option<(String, crate::types::TySyn)> {
    if !b.contains('=') || crate::ast_summary::parse_fn_bound(b).is_some() {
        return None;
    }
    crate::ast_summary::assoc_bindings(b).into_iter().next()
}

pub fn strip_paren(e: &Expr) -> &Expr {
    match e {
        Expr::Paren(p) => strip_paren(p),
        e => e,
    }
}

pub fn is_place_expr(e: &Expr) -> bool {
    match e {
        Expr::Path(None, p) => ident_of(p).is_some(),
        Expr::Field(..) => true,
        Expr::Index(_, ix, _) => !matches!(&**ix, Expr::Range(..)),
        Expr::Unary(op, _, _) => op == "*",
        Expr::Paren(p) => is_place_expr(p),
        _ => false,
    }
}

/// `let x` / `let mut x` / `let _` のような単純な let か
pub fn is_simple_let(p: &Pat) -> bool {
    match p {
        Pat::Ident(by_ref, _, _, sub) => sub.is_none() && !*by_ref,
        Pat::Wild => true,
        _ => false,
    }
}

/// 識別子のパターンが const (初期値がリテラル) を指すなら、そのリテラルのパターン
pub fn const_lit_pat(prog: &Program, p: &Pat) -> Option<Pat> {
    let Pat::Ident(false, false, n, None) = p else { return None };
    if !n.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    let &ci = prog.const_index.get(n)?;
    let c = &prog.consts[ci as usize];
    if c.stat.is_some() {
        return None;
    }
    match crate::parse::parse_expr(&c.expr).ok()? {
        e @ Expr::Lit(_, _) => Some(Pat::Lit(Box::new(e))),
        _ => None,
    }
}

/// 識別子のパターンが unit バリアント (`None` など) に解決されるなら、そのパス
pub fn unit_variant_ident(prog: &Program, p: &Pat) -> Option<Path> {
    let Pat::Ident(false, false, n, None) = p else { return None };
    let path = path_of(n);
    let (sid, vi) = prog.variant_by_p(&PPath::of(&path), None)?;
    (prog.structs[sid as usize].variants[vi].kind == crate::types::VariantKind::Unit).then_some(path)
}

pub fn has_bindings(p: &Pat) -> bool {
    match p {
        Pat::Ident(_, _, n, None) if n == "None" => false,
        Pat::Ident(..) => true,
        Pat::Ref(_, x) => has_bindings(x),
        Pat::Or(cs) => cs.iter().any(has_bindings),
        Pat::TupleStruct(_, es) | Pat::Tuple(es) | Pat::Slice(es) => es.iter().any(has_bindings),
        Pat::Struct(_, fs, _) => fs.iter().any(|f| has_bindings(&f.1)),
        _ => false,
    }
}

pub fn is_cmp(op: &str) -> bool {
    matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=")
}

pub fn pat_name(p: &Pat) -> R<String> {
    match p {
        Pat::Ident(false, _, n, None) => Ok(n.clone()),
        Pat::Wild => Ok("_".into()),
        _ => err(SP, "unsupported pattern"),
    }
}

impl Cx<'_> {
    pub fn same(&self, span: Sp, got: Ty, want: Ty) -> R<()> {
        if got == want || got == Ty::Never {
            Ok(())
        } else {
            if std::env::var_os("ACG_PANIC_ERR").is_some() {
                panic!("mismatched types");
            }
            Err(self.at(span, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(want), self.prog.tn(got))))
        }
    }

    /// 書かれた型 (`Self` と型パラメータは env から)
    pub fn parse_ty(&self, t: &crate::parse::Ty) -> R<Ty> {
        let ts = crate::ast_types::tysyn(t, Some(&crate::types::TySyn::Named("Self".into())))?;
        self.prog.resolve_env(&ts, &self.env)
    }

    /// マクロの引数。型推論が読んだものがあればその構文木 (型の記録がある)、なければ読み直す
    pub fn macro_args_of(&self, g: &str) -> Option<Vec<&'static Expr>> {
        if let Some(v) = self.margs.get(&(g.as_ptr() as usize)) {
            // SAFETY: margs の中身は Cx が生きている間動かない
            return Some(v.iter().map(|e| unsafe { &*(e as *const Expr) }).collect());
        }
        None
    }

    /// 書き換えで作った式をコード生成の間持っておき、その参照を返す
    pub fn keep_expr(&mut self, e: Expr) -> &'static Expr {
        self.keep.push(Box::new(e));
        // SAFETY: keep の Box の中身は Cx が生きている間動かない
        unsafe { &*(&**self.keep.last().unwrap() as *const Expr) }
    }

    // --- メモリ操作 ------------------------------------------------------

    pub fn slot(&mut self, t: Ty) -> StackSlot {
        let (size, align) = self.prog.size_align(t);
        self.b.create_stack_slot(size.max(1), align.max(1))
    }

    /// 一時領域を確保してそのアドレスを返す
    pub fn temp(&mut self, t: Ty) -> Value {
        let ss = self.slot(t);
        self.b.ins().stack_addr(PTR, ss, 0)
    }

    pub fn addr_plus(&mut self, a: Value, off: i32) -> Value {
        if off == 0 { a } else { self.b.ins().iadd_imm_s(a, off as i64) }
    }

    /// struct のコピー
    pub fn copy(&mut self, dst: Value, src: Value, t: Ty) {
        self.copy_at(dst, src, t, 0, 0);
    }

    /// src + so にある t を dst + doff に写す。struct (enum でない) はフィールドごとにその型で写す
    /// (minclif がスタックの struct のフィールドをレジスタに置けるように: 読み書きの幅をフィールドと揃える)
    fn copy_at(&mut self, dst: Value, src: Value, t: Ty, doff: u32, soff: u32) {
        if let Ty::Struct(sid) = t {
            let sd = &self.prog.structs[sid as usize];
            if !sd.is_enum && sd.generics.is_empty() && sd.fields.len() <= 16 {
                let fs = sd.fields.clone();
                for (_, ft, off) in fs {
                    self.copy_at(dst, src, ft, doff + off, soff + off);
                }
                return;
            }
        }
        // (スカラーの小さい配列は要素ごと: 要素の幅で読み書きする)
        if let Ty::Array(e, n) = t {
            if n <= 16 && !e.is_agg() && !e.is_fat() && clif_ty(*e).is_some() {
                let (es, _) = self.prog.size_align(*e);
                for i in 0..n {
                    self.copy_at(dst, src, *e, doff + i * es, soff + i * es);
                }
                return;
            }
        }
        if !t.is_agg() && !t.is_fat() {
            if let Some(ct) = clif_ty(t) {
                let v = self.b.ins().load(ct, mf(), src, soff as i32);
                self.b.ins().store(mf(), v, dst, doff as i32);
            }
            return;
        }
        let (size, _) = self.prog.size_align(t);
        let mut off = 0;
        for (n, ty) in [(8, types::I64), (4, types::I32), (2, types::I16), (1, types::I8)] {
            while off + n <= size {
                let v = self.b.ins().load(ty, mf(), src, (soff + off) as i32);
                self.b.ins().store(mf(), v, dst, (doff + off) as i32);
                off += n;
            }
        }
    }

    pub fn loc_value(&mut self, l: Loc) -> Option<Value> {
        match l {
            Loc::Var(v) => Some(self.b.use_var(v)),
            Loc::Slot(ss) => Some(self.b.ins().stack_load(PTR, ss, 0)),
            Loc::Ptr(a) => Some(self.b.ins().load(PTR, mf(), a, 0)),
            Loc::Zst => None,
        }
    }

    pub fn loc_addr(&mut self, l: Loc) -> Option<Value> {
        match l {
            Loc::Slot(ss) => Some(self.b.ins().stack_addr(PTR, ss, 0)),
            Loc::Ptr(v) => Some(v),
            Loc::Var(_) | Loc::Zst => None,
        }
    }

    pub fn read(&mut self, p: Place) -> V {
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

    pub fn write(&mut self, p: Place, v: Option<Value>) {
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
    pub fn bind(&mut self, name: String, ty: Ty, v: Option<Value>) {
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
                        self.b.ins().stack_store(v, ss, 0);
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

    pub fn lookup(&self, name: &str) -> Option<(Loc, Ty)> {
        self.scopes.iter().rev().flat_map(|s| s.iter().rev()).find(|(n, _, _)| n == name).map(|&(_, l, t)| (l, t))
    }

    fn body(&mut self, f: &ItemFn, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        let mut params = self.b.func_params();
        if is_sret(sig) {
            self.sret = Some(params.remove(0));
        }
        let mut params = params.into_iter();
        let mut names: Vec<String> = vec![];
        if f.sig.recv.is_some() {
            names.push("self".to_string());
        }
        for (p, _) in &f.sig.params {
            names.push(pat_name(p)?);
        }
        for (name, &ty) in names.into_iter().zip(&sig.params) {
            if !ty.is_agg() && clif_ty(ty).is_none() {
                self.bind(name, ty, None);
                continue;
            }
            let Some(v) = params.next() else {
                return Err("internal: parameter count mismatch".into());
            };
            if ty.is_agg() {
                // 呼び出し側がコピー済みなので、そのメモリをそのまま使う (値渡しの引数は呼び出された側が drop する)
                self.scopes.last_mut().unwrap().push((name, Loc::Ptr(v), ty));
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
                let a = if by_ref { self.b.ins().load(PTR, mf(), env, off as i32) } else { self.addr_plus(env, off as i32) };
                let loc = if clif_ty(t).is_none() && !t.is_agg() { Loc::Zst } else { Loc::Ptr(a) };
                self.scopes.last_mut().unwrap().push((n, loc, t));
            }
        }
        if self.is_main && self.prog.panic_handler.is_none() && self.prog.index.contains_key("__exr_rt_init") {
            let _ = self.call_named("__exr_rt_init", vec![Ty::Unit], &[], SP);
        }
        let (v, t) = self.block(&f.block, Some(self.ret))?;
        if t == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(());
        }
        // (エラーの場所は末尾の式 (なければ関数の先頭))
        let tail = match f.block.stmts.last() {
            Some(Stmt::Expr(e, false)) => err_pos(e),
            _ => SP,
        };
        self.same(tail, t, self.ret)?;
        self.drop_for_exit(0, 0);
        self.emit_return(v);
        Ok(())
    }

    pub fn emit_return(&mut self, v: Option<Value>) {
        // (展開した本体: 続きのブロックへ)
        if let Some(cont) = self.inline_ret {
            if let Some(dst) = self.sret {
                if let Some(v) = v {
                    self.copy(dst, v, self.ret);
                }
                self.b.ins().jump(cont, &[]);
            } else if self.b.block_params(cont).is_empty() {
                self.b.ins().jump(cont, &[]);
            } else {
                match v {
                    Some(v) => self.b.ins().jump(cont, &[v]),
                    None => self.b.ins().trap(TrapCode::unwrap_user(1)),
                }
            }
            return;
        }
        if self.is_main {
            // `fn main() -> Result<(), E>`: Err なら `Error: {:?}` を出して 1 で終わる
            if let (Ty::Struct(rs), Some(v)) = (self.ret, v) {
                let rd = &self.prog.structs[rs as usize];
                if rd.is_enum && rd.base.is_some_and(|b| self.prog.structs[b as usize].name == "Result") {
                    if let Some(ev) = rd.variants.iter().position(|x| x.name == "Err") {
                        let (_, et, off) = rd.variants[ev].fields[0];
                        let ok = self.b.create_block();
                        if self.test_variant(Place::Mem(v, 0, self.ret), rs, ev, SP, ok).is_ok() {
                            let pa = self.addr_plus(v, off as i32);
                            let _ = self.call_named("__exr_main_err", vec![et], &[(pa, Ty::Ref(false, intern(et)))], SP);
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

    /// エラーのメッセージに場所 (ファイルの中の `行:桁: `) を付ける。もう付いているか、位置が分からなければそのまま
    pub fn at(&self, sp: Sp, m: String) -> String {
        if sp.line == 0 || has_loc(&m) {
            return m;
        }
        format!("{}:{}: {m}", self.line_base + sp.line, sp.col + 1)
    }

    /// `&&T` (`&&&T` ..) を `&T` の場所に渡すときの deref の強制変換
    pub fn deref_coerce(&mut self, mut v: Option<Value>, mut t: Ty, want: Ty) -> (Option<Value>, Ty) {
        while let (Ty::Ref(..), Ty::Ref(_, inner @ Ty::Ref(..)), Some(val)) = (want, t, v) {
            if t == want || t.is_fat() {
                break;
            }
            // (`&&str` の値は `&str` (fat) の置き場所のアドレスで、`&str` の表現と同じ: 読まない)
            v = Some(if inner.is_fat() { val } else { self.b.ins().load(PTR, mf(), val, 0) });
            t = *inner;
        }
        // (`&String` を `&str` に)
        if let (Ty::Ref(false, Ty::Str), Ty::Ref(_, Ty::Struct(sid)), Some(val)) = (want, t, v) {
            let sd = &self.prog.structs[*sid as usize];
            if sd.name == "String" && sd.base.is_none() && self.prog.index.contains_key("String::as_str") {
                if let Ok((Some(r), rt)) = self.call_named("String::as_str", vec![], &[(val, Ty::Ref(false, intern(Ty::Struct(*sid))))], SP) {
                    return (Some(r), rt);
                }
            }
        }
        (v, t)
    }

    /// 終端命令の後に続くコード用の到達不能ブロックへ移る。
    pub fn dead(&mut self) -> V {
        let blk = self.b.create_block();
        self.b.switch_to_block(blk);
        (None, Ty::Never)
    }

    pub fn block(&mut self, blk: &crate::parse::Block, expected: Option<Ty>) -> R<V> {
        self.scopes.push(vec![]);
        let mut diverged = false;
        let mut result: V = (None, Ty::Unit);
        let n = blk.stmts.len();
        for (i, st) in blk.stmts.iter().enumerate() {
            let last = i + 1 == n;
            let t0 = self.temps.len();
            match st {
                Stmt::Let(pat, ann_ty, init, els) if !is_simple_let(pat) || els.is_some() => {
                    // 分解する let (`let S { x, y } = s;`): 反駁不能パターンとして束縛だけ行う
                    let Some(init) = init else {
                        return err(SP, "`let` without initializer is not supported");
                    };
                    let ann = ann_ty.as_ref().and_then(|t| self.parse_ty(t).ok());
                    let Some(place) = self.scrutinee_as(init, ann)? else {
                        diverged = true;
                        continue;
                    };
                    // `let PAT = e else { .. };`: 合わなければ else の (発散する) ブロックへ
                    if let Some(else_b) = els {
                        let else_blk = self.b.create_block();
                        self.pat_test(pat, place, else_blk)?;
                        let cur = self.b.create_block();
                        self.b.ins().jump(cur, &[]);
                        self.b.switch_to_block(else_blk);
                        let (_, et) = self.block(else_b, Some(Ty::Never))?;
                        if et != Ty::Never {
                            return err(SP, "the `else` block of `let ... else` must diverge (return / break / panic)");
                        }
                        self.b.ins().trap(TrapCode::unwrap_user(2));
                        self.b.switch_to_block(cur);
                    }
                    let ob = self.owned_binds;
                    let pre = if is_place_expr(init) { self.moved_flag(init) } else { None };
                    self.pat_bind(pat, place, BindMode::Move)?;
                    if self.owned_binds > ob {
                        match pre {
                            Some(f) => self.clear_flag(f),
                            None => {
                                let f = self.last_scrut;
                                self.release(init, f);
                            }
                        }
                    }
                    self.extend_temps(t0);
                }
                Stmt::Let(Pat::Wild, _, init, None) => {
                    // `let _ = e;`: 場所ならムーブしない。値ならすぐ drop する
                    let Some(init) = init else {
                        return err(SP, "`let` without initializer is not supported");
                    };
                    if is_place_expr(init) {
                        let _ = self.place(init)?;
                    } else {
                        let (v, t) = self.expr(init, None)?;
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
                Stmt::Let(pat, ann_ty, init, _) => {
                    let name = pat_name(pat)?;
                    let ann = match ann_ty {
                        Some(t) => Some(self.parse_ty(t)?),
                        // (型推論は強制変換の行き先 `[T]` などを入れてしまうことがあるので、サイズ不定の型は使わない)
                        None => self
                            .lets
                            .get(&crate::ast_infer::stmt_key(st))
                            .copied()
                            .filter(|t| !matches!(t, Ty::Slice(_) | Ty::Str | Ty::Dyn(_))),
                    };
                    // (assert_eq! の展開: 右の値の型は左と同じ)
                    let hintable = |e: &Expr| {
                        let e = match e {
                            Expr::Ref(_, x) => strip_paren(x),
                            e => e,
                        };
                        !matches!(e, Expr::Lit(t, _) if matches!(lit::lit(t), Some(Lit::Str(_))))
                    };
                    let other = match name.as_str() {
                        "__exr_r" => Some("__exr_l"),
                        "__exr_l" => Some("__exr_r"),
                        _ => None,
                    };
                    let hint = match (init, other) {
                        (Some(i), Some(o)) if ann.is_none() && hintable(i) => self.lookup(o).map(|x| x.1),
                        _ => None,
                    };
                    // `let x;` (後で代入する): 0 で埋めて置く
                    let Some(init) = init else {
                        let ann = ann.filter(|&a| !has_hole(self.prog, a));
                        let Some(ty) = ann.filter(|&t| !self.prog.has_param(t)) else {
                            return err(SP, "cannot infer the type of this `let`; annotate it");
                        };
                        if self.needs_drop(ty) && !self.zeroable(ty) {
                            return err(SP, "`let` without initializer is not supported for this type");
                        }
                        let v = if ty.is_agg() {
                            let a = self.temp(ty);
                            self.zero(a, ty);
                            Some(a)
                        } else {
                            match clif_ty(ty) {
                                None => None,
                                Some(ct) if ct == types::F64 => Some(self.b.ins().f64const(0.0)),
                                Some(ct) if ct == types::F32 => Some(self.b.ins().f32const(0.0)),
                                Some(ct) if ct == types::I128 => Some(self.int_const(Ty::I128, 0)),
                                Some(ct) => Some(self.b.ins().iconst(ct, 0)),
                            }
                        };
                        self.bind(name, ty, v);
                        continue;
                    };
                    let (mut v, mut t) = self.expr(init, ann.or(hint))?;
                    if t == Ty::Never {
                        diverged = true;
                        continue;
                    }
                    let mut ann = ann.filter(|&a| !has_hole(self.prog, a));
                    // (型推論の結果 (注釈なし) と違う: `&&str` → `&str` は deref の強制変換、それでも違えば実際の型)
                    if let Some(a) = ann.filter(|&a| a != t) {
                        let (v2, t2) = self.deref_coerce(v, t, a);
                        if t2 == a {
                            v = v2;
                            t = t2;
                        } else if ann_ty.is_none() {
                            ann = None;
                        }
                    }
                    let ty = ann.unwrap_or(t);
                    self.same(SP, t, ty).map_err(|m| self.at(expr_start(init), m))?;
                    self.bind(name, ty, v);
                    self.extend_temps(t0);
                }
                // (`;` の付いたマクロの文。末尾の `vec![]` などは式として期待型を渡す)
                Stmt::Expr(Expr::Macro(p, g, sp), semi) if *semi => {
                    let saved = std::mem::replace(&mut self.mbase, sp.at);
                    let saved_s = std::mem::replace(&mut self.mstart, sp.start);
                    let r = self.macro_(p, g, None);
                    self.mbase = saved;
                    self.mstart = saved_s;
                    let r = r?;
                    if r.1 == Ty::Never {
                        diverged = true;
                    }
                    if last && !*semi {
                        result = r;
                        self.drop_tail_temps(t0, result);
                    } else {
                        self.drop_temps(t0);
                    }
                }
                Stmt::Expr(e, semi) => {
                    let exp = if last && !*semi { expected } else { None };
                    let r = self.expr(e, exp)?;
                    if r.1 == Ty::Never {
                        diverged = true;
                    }
                    if last && !*semi {
                        result = r;
                        self.drop_tail_temps(t0, result);
                    } else {
                        if !*semi && !matches!(r.1, Ty::Unit | Ty::Never) {
                            return err(SP, "expected `;` after non-unit expression");
                        }
                        if let (Some(v), t) = r {
                            if t != Ty::Never {
                                self.own_temp(v, t);
                            }
                        }
                        self.drop_temps(t0);
                    }
                }
                Stmt::Item(_) => return err(SP, "nested items are not supported"),
            }
        }
        self.pop_scope();
        if diverged && result.1 == Ty::Unit {
            result = (None, Ty::Never);
        }
        Ok(result)
    }

    /// コード生成せずに型だけ推測する (整数リテラルの型決定用)。
    pub fn peek(&self, e: &Expr) -> Option<Ty> {
        // (組み込みの AsRef: 型推論の結果より先に)
        if let Expr::MethodCall(r, m, _, a, _) = e {
            if m == "as_ref" && a.is_empty() {
                if let Some(t) = self.as_ref_ty(r, None) {
                    return Some(t);
                }
            }
        }
        if let Some(&t) = self.lets.get(&(e as *const Expr as usize)) {
            return Some(t);
        }
        // (書き換えで複製した式は、同じ形の式の型で引く。形が同じで型の違う式があれば使わない)
        if !self.texts.is_empty() && !matches!(e, Expr::Path(..)) {
            if let Some(&Some(t)) = self.texts.get(&crate::ast_infer::expr_hash(e)) {
                return Some(t);
            }
        }
        self.peek_syn(e)
    }

    pub fn peek_syn(&self, e: &Expr) -> Option<Ty> {
        match e {
            // `f64::NAN` / `std::f64::consts::PI`
            Expr::Path(None, p)
                if p.segs.len() >= 2
                    && p.segs.iter().any(|s| s.name == "f64" || s.name == "f32")
                    && p.segs.last().is_some_and(|s| s.name.chars().all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())) =>
            {
                Some(if p.segs.iter().any(|s| s.name == "f32") { Ty::F32 } else { Ty::F64 })
            }
            // `u64::MAX` / `i128::MIN` / `u32::BITS`
            Expr::Path(None, p) if p.segs.len() == 2 && matches!(p.segs[1].name.as_str(), "MAX" | "MIN" | "BITS") && Ty::from_suffix(&p.segs[0].name).is_some() => {
                if p.segs[1].name == "BITS" { Some(Ty::U32) } else { Ty::from_suffix(&p.segs[0].name) }
            }
            Expr::Path(None, p) if p.segs.len() == 2 && self.const_of(e).is_some() => Some(self.prog.consts[self.const_of(e)? as usize].ty),
            Expr::Path(None, _) if self.const_generic(e).is_some() => Some(Ty::Usize),
            Expr::Try(x) => {
                let Ty::Struct(sid) = self.peek(x)? else { return None };
                let sd = &self.prog.structs[sid as usize];
                sd.variants.iter().find(|v| v.name == "Some" || v.name == "Ok").and_then(|v| v.fields.first()).map(|f| f.1)
            }
            Expr::Lit(l, _) => match lit::lit(l)? {
                Lit::Int(i) if i.suffix() == "f32" => Some(Ty::F32),
                Lit::Int(i) if i.suffix() == "f64" => Some(Ty::F64),
                Lit::Float(f) => Some(if f.suffix() == "f32" { Ty::F32 } else { Ty::F64 }),
                Lit::Int(i) => Ty::from_suffix(i.suffix()),
                Lit::Bool(_) => Some(Ty::Bool),
                Lit::Byte(_) => Some(Ty::U8),
                Lit::Char(_) => Some(Ty::Char),
                Lit::Str(_) => Some(Ty::Ref(false, intern(Ty::Str))),
                Lit::ByteStr(b) => Some(Ty::Ref(false, intern(Ty::Array(intern(Ty::U8), b.len() as u32)))),
                _ => None,
            },
            Expr::Path(None, p) if ident_of(p).is_some_and(|id| self.lookup(id).is_none() && self.prog.const_index.contains_key(id)) => {
                let ci = self.prog.const_index[ident_of(p).unwrap()];
                Some(self.prog.consts[ci as usize].ty)
            }
            Expr::Path(None, p) if p.segs.len() == 2 => self.prog.variant_by_p(&PPath::of(p), self.self_sid).map(|v| Ty::Struct(v.0)),
            // タプル構造体の構築 `V(1, 2)` (ジェネリックでないもの)
            Expr::Call(f, _)
                if matches!(&**f, Expr::Path(None, p) if p.segs.len() == 1
                    && self.lookup(&p.segs[0].name).is_none()
                    && self.prog.struct_by_p(&PPath::of(p), self.self_sid)
                        .is_some_and(|s| self.prog.structs[s as usize].generics.is_empty())) =>
            {
                let Expr::Path(None, p) = &**f else { unreachable!() };
                self.prog.struct_by_p(&PPath::of(p), self.self_sid).map(Ty::Struct)
            }
            Expr::Call(f, _) if matches!(&**f, Expr::Path(None, p) if p.segs.len() == 2) => {
                let Expr::Path(None, p) = &**f else { unreachable!() };
                match self.prog.variant_by_p(&PPath::of(p), self.self_sid) {
                    Some((sid, _)) => Some(Ty::Struct(sid)),
                    None => self.callee(f).map(|(_, s)| s.ret),
                }
            }
            Expr::Path(None, p) => ident_of(p).and_then(|id| self.lookup(id)).map(|(_, t)| t).or_else(|| self.float_const_name(e).map(|_| Ty::F64)),
            Expr::Call(..) if self.prim_method_call(e).is_some() => self.peek(&self.prim_method_call(e)?),
            Expr::Call(f, _) => match self.callee(f) {
                Some((_, s)) => Some(s.ret).filter(|&t| !self.prog.has_param(t)),
                None => {
                    let mut t = self.peek(f)?;
                    while let Ty::Ref(_, i) = t {
                        t = *i;
                    }
                    self.closure_sig(t).map(|(_, r)| r).filter(|&t| !self.prog.has_param(t))
                }
            },
            Expr::Cast(_, ty) => self.parse_ty(ty).ok(),
            Expr::Index(b, ix, _) if !matches!(&**ix, Expr::Range(..)) => {
                let mut t = self.peek(b)?;
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
                let e = a.iter().find_map(|e| self.peek(e))?;
                Some(Ty::Array(intern(e), a.len() as u32))
            }
            Expr::MethodCall(recv, name, _, args, _) => {
                let mut t = self.peek(recv)?;
                while let Ty::Ref(_, i) = t {
                    t = *i;
                }
                if name == "to_string" && args.is_empty() {
                    if let Some(&s) = self.prog.struct_index.get("String") {
                        return Some(Ty::Struct(s));
                    }
                }
                if t == Ty::Str {
                    return match name.as_str() {
                        "trim" | "as_str" => Some(Ty::Ref(false, intern(Ty::Str))),
                        "starts_with" | "ends_with" | "contains" | "is_empty" => Some(Ty::Bool),
                        "len" => Some(Ty::Usize),
                        _ => None,
                    };
                }
                if t.is_int() || t.is_float() {
                    return match name.as_str() {
                        "count_ones" | "count_zeros" | "leading_zeros" | "trailing_zeros" | "leading_ones" | "trailing_ones" => Some(Ty::U32),
                        "is_power_of_two" | "is_positive" | "is_negative" | "is_nan" | "is_finite" | "is_infinite" | "is_sign_negative"
                        | "is_sign_positive" => Some(Ty::Bool),
                        "pow" | "wrapping_add" | "wrapping_sub" | "wrapping_mul" | "wrapping_neg" | "saturating_add" | "saturating_sub" | "abs" | "min"
                        | "max" | "rem_euclid" | "div_euclid" | "next_multiple_of" | "next_power_of_two" | "rotate_left" | "rotate_right"
                        | "swap_bytes" | "signum" | "wrapping_pow" | "saturating_pow" | "isqrt" | "sqrt" | "floor" | "ceil" | "round" | "trunc"
                        | "powi" | "powf" | "exp" | "ln" | "sin" | "cos" | "tan" | "clamp" => Some(t),
                        _ => None,
                    };
                }
                let Ty::Struct(sid) = t else { return None };
                let id = self.prog.method(sid, name).or_else(|| self.prog.default_method(sid, name))?;
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
            }
            Expr::Paren(p) => self.peek(p),
            Expr::Macro(p, _, _) if is_ident(p, "format") => self.prog.struct_index.get("String").map(|&s| Ty::Struct(s)),
            Expr::Unary(op, x, _) if op == "*" => match self.peek(x)? {
                Ty::Ref(_, i) => Some(*i),
                _ => None,
            },
            Expr::Unary(_, x, _) => self.peek(x),
            // `&v[a..b]`: スライス (文字列なら &str)
            Expr::Ref(m, x) if matches!(strip_paren(x), Expr::Index(_, ix, _) if matches!(&**ix, Expr::Range(..))) => {
                let Expr::Index(b, _, _) = strip_paren(x) else { unreachable!() };
                let mut t = self.peek(b)?;
                while let Ty::Ref(_, i) = t {
                    if t.is_fat() {
                        break;
                    }
                    t = *i;
                }
                let m = *m;
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
            Expr::Ref(m, x) => Some(Ty::Ref(*m, intern(self.peek(x)?))),
            Expr::Field(b, name, _) => {
                let mut t = self.peek(b)?;
                while let Ty::Ref(_, i) = t {
                    t = *i;
                }
                let Ty::Struct(sid) = t else { return None };
                self.prog.field(sid, name).map(|(t, _)| t)
            }
            Expr::Struct(p, _, _) => {
                let pp = PPath::of(p);
                self.prog.struct_by_p(&pp, self.self_sid).or_else(|| self.prog.variant_by_p(&pp, self.self_sid).map(|v| v.0)).map(Ty::Struct)
            }
            Expr::Binary(op, _, _, _) if is_cmp(op) || op == "&&" || op == "||" => Some(Ty::Bool),
            Expr::If(_, then, _) => match then.stmts.last() {
                Some(Stmt::Expr(e, false)) => self.peek(e),
                _ => None,
            },
            Expr::Block(_, _, b) => match b.stmts.last() {
                Some(Stmt::Expr(e, false)) => self.peek(e),
                _ => None,
            },
            Expr::Binary(op, l, _, _) if op == "<<" || op == ">>" => self.peek(l),
            Expr::Binary(_, l, r, _) => self.peek(l).or_else(|| self.peek(r)),
            _ => None,
        }
    }

    /// `f` / `S::f` / `Self::f` 形式の呼び出し先
    pub fn callee(&self, f: &Expr) -> Option<(u32, &FnSig)> {
        let Expr::Path(q, p) = f else { return None };
        let by_ty = |t: Ty, m: &str| -> Option<u32> {
            match t {
                Ty::Struct(sid) => self.prog.method(sid, m).or_else(|| self.prog.default_method(sid, m)),
                t => self.prog.prim_method(t, m),
            }
        };
        // `<T as Trait>::m` / `<T>::m`
        if let Some(q) = q {
            let t = self.parse_ty(q).ok()?;
            let m = p.segs.last()?.name.clone();
            let i = by_ty(t, &m)?;
            return Some((i, &self.prog.sigs[i as usize]));
        }
        // `T::f(..)` (T は型引数。インスタンスでは具体的な型)
        if p.segs.len() == 2 {
            let g = &p.segs[0].name;
            if self.env.params.iter().any(|(n, _)| n == g) {
                let ty = crate::parse::Ty::Path(None, path_of(g));
                if let Ok(t) = self.parse_ty(&ty) {
                    if let Some(i) = by_ty(t, &p.segs[1].name) {
                        return Some((i, &self.prog.sigs[i as usize]));
                    }
                }
            }
        }
        if let Some(id) = ident_of(p) {
            if self.lookup(id).is_some() {
                return None;
            }
        }
        let i = self.prog.fn_by_p(&PPath::of(p), self.self_sid)?;
        Some((i, &self.prog.sigs[i as usize]))
    }

    /// 関数 id (インスタンスも) を呼ぶための参照
    pub fn fref(&mut self, id: u32, sig: &FnSig) -> FuncRef {
        if let Some(&f) = self.frefs.get(&id) {
            return f;
        }
        let rets: Vec<Type> = clif_sig(sig).returns.iter().map(|p| p.value_type).collect();
        let f = self.b.import_function(&sym_name('f', id), rets);
        self.frefs.insert(id, f);
        f
    }

    /// 無名データ i のアドレス
    pub fn data_sym(&mut self, i: u32) -> Value {
        let g = self.b.symbol(&sym_name('d', i));
        self.b.ins().symbol_value(PTR, g)
    }

    /// static i のアドレス
    pub fn static_sym(&mut self, i: u32) -> Value {
        let g = self.b.symbol(&sym_name('s', i));
        self.b.ins().symbol_value(PTR, g)
    }

    /// vtable i のアドレス
    pub fn vtable_sym(&mut self, i: u32) -> Value {
        let g = self.b.symbol(&sym_name('v', i));
        self.b.ins().symbol_value(PTR, g)
    }
}
