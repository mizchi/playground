//! フロントエンド: アイテム要約 (シグネチャ・struct 定義) から型表と参照グラフを作る。
//!
//! ここは「関数本体を見ない」軽い処理だけにしている。本体の型検査・lowering は
//! codegen 側で関数ごとに行い、キャッシュヒットした関数は本体に一切触れない。

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use syn::spanned::Spanned;

use crate::cache::{Key, KeyHasher};
use crate::enc::{Enc, Rd};
use crate::lexer::{ItemKind, SrcItem};

pub type R<T> = Result<T, String>;

pub fn err<T>(span: proc_macro2::Span, msg: impl std::fmt::Display) -> R<T> {
    let s = span.start();
    Err(format!("{}:{}: {}", s.line, s.column + 1, msg))
}

// ---------------------------------------------------------------------------
// 型
// ---------------------------------------------------------------------------

/// 解決済みの型。Copy にするため参照先は intern した `&'static Ty` で持つ。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Ty {
    I8,
    I16,
    I32,
    I64,
    Isize,
    U8,
    U16,
    U32,
    U64,
    Usize,
    Bool,
    Unit,
    Never,
    /// `&T` (false) / `&mut T` (true)
    Ref(bool, &'static Ty),
    /// Program::structs の index
    Struct(u32),
    /// ジェネリック関数の型パラメータ (FnSig::generics の index)。インスタンス化で具体型に置き換わる
    Param(u8),
    /// `[T; N]` (値として扱う集約型)
    Array(&'static Ty, u32),
    /// `[T]` (サイズ不定。`&[T]` の中身としてだけ現れる)
    Slice(&'static Ty),
    /// `str` (サイズ不定。`&str` の中身としてだけ現れる)
    Str,
    /// `*const T` (false) / `*mut T` (true)
    Ptr(bool, &'static Ty),
}

impl Ty {
    /// メモリ上に置き、値としてはアドレスで扱う型 (struct / enum / 配列 / `&[T]`・`&str` の fat pointer)
    pub fn is_agg(self) -> bool {
        matches!(self, Ty::Struct(_) | Ty::Array(..)) || self.is_fat()
    }
    /// (ポインタ, 長さ) の 2 語で表す参照
    pub fn is_fat(self) -> bool {
        matches!(self, Ty::Ref(_, Ty::Slice(_) | Ty::Str))
    }
}

/// 型の intern 表。種類数はプログラム中の型の数で抑えられるので解放しない。
pub fn intern(t: Ty) -> &'static Ty {
    static TABLE: OnceLock<Mutex<HashSet<&'static Ty>>> = OnceLock::new();
    let mut tab = TABLE.get_or_init(Default::default).lock().unwrap();
    if let Some(&r) = tab.get(&t) {
        return r;
    }
    let r: &'static Ty = Box::leak(Box::new(t));
    tab.insert(r);
    r
}

impl Ty {
    pub fn from_suffix(s: &str) -> Option<Ty> {
        Some(match s {
            "i8" => Ty::I8,
            "i16" => Ty::I16,
            "i32" => Ty::I32,
            "i64" => Ty::I64,
            "isize" => Ty::Isize,
            "u8" => Ty::U8,
            "u16" => Ty::U16,
            "u32" => Ty::U32,
            "u64" => Ty::U64,
            "usize" => Ty::Usize,
            _ => return None,
        })
    }

    fn prim(s: &str) -> Option<Ty> {
        Ty::from_suffix(s).or(match s {
            "bool" => Some(Ty::Bool),
            _ => None,
        })
    }

    pub fn is_int(self) -> bool {
        matches!(
            self,
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::Isize | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 | Ty::Usize
        )
    }

    pub fn signed(self) -> bool {
        matches!(self, Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::Isize)
    }

    /// スカラー (整数・bool・参照) のビット幅。struct / unit / never は 0。
    pub fn bits(self) -> u32 {
        match self {
            Ty::I8 | Ty::U8 | Ty::Bool => 8,
            Ty::I16 | Ty::U16 => 16,
            Ty::I32 | Ty::U32 => 32,
            Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize | Ty::Ptr(..) => 64,
            Ty::Ref(..) if self.is_fat() => 0,
            Ty::Ref(..) => 64,
            Ty::Unit | Ty::Never | Ty::Struct(_) | Ty::Param(_) | Ty::Array(..) | Ty::Slice(_) | Ty::Str => 0,
        }
    }
}

/// 名前未解決の型 (要約に保存する形)。struct は名前で持つので、
/// アイテムの並び順が変わってもキャッシュが壊れない。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TySyn {
    Prim(Ty),
    Ref(bool, Box<TySyn>),
    Named(String),
    /// `Option<T>` など、ジェネリックな struct / enum の適用
    App(String, Vec<TySyn>),
    /// `(A, B)`
    Tuple(Vec<TySyn>),
    /// `[T; N]` (N はリテラルか const の名前)
    Array(Box<TySyn>, ArrLen),
    Slice(Box<TySyn>),
    Str,
    Ptr(bool, Box<TySyn>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ArrLen {
    Lit(u64),
    Const(String),
}

/// 型の構文を TySyn にする。`Self` は self_ty (impl の self 型) に置き換える。
/// 本体の中では self_ty に `Named("Self")` を渡し、解決時に TyEnv から引く
pub fn parse_tysyn(t: &syn::Type, self_ty: Option<&TySyn>) -> R<TySyn> {
    Ok(match t {
        syn::Type::Never(_) => TySyn::Prim(Ty::Never),
        syn::Type::Tuple(tt) if tt.elems.is_empty() => TySyn::Prim(Ty::Unit),
        syn::Type::Tuple(tt) => TySyn::Tuple(tt.elems.iter().map(|e| parse_tysyn(e, self_ty)).collect::<R<_>>()?),
        syn::Type::Paren(p) => parse_tysyn(&p.elem, self_ty)?,
        syn::Type::Group(g) => parse_tysyn(&g.elem, self_ty)?,
        // ライフタイムはコード生成に影響しないので消去する
        syn::Type::Reference(r) => TySyn::Ref(r.mutability.is_some(), Box::new(parse_tysyn(&r.elem, self_ty)?)),
        syn::Type::Ptr(p) => TySyn::Ptr(p.mutability.is_some(), Box::new(parse_tysyn(&p.elem, self_ty)?)),
        syn::Type::Slice(sl) => TySyn::Slice(Box::new(parse_tysyn(&sl.elem, self_ty)?)),
        syn::Type::Array(a) => {
            let len = match &a.len {
                syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(i), .. }) => ArrLen::Lit(i.base10_parse().map_err(|e| e.to_string())?),
                syn::Expr::Path(p) if p.path.get_ident().is_some() => ArrLen::Const(p.path.get_ident().unwrap().to_string()),
                e => return err(e.span(), "array length must be a literal or a const"),
            };
            TySyn::Array(Box::new(parse_tysyn(&a.elem, self_ty)?), len)
        }
        syn::Type::Path(p) if p.qself.is_none() => {
            // `core::ffi::c_int` などは最後のセグメントで見る
            let seg = p.path.segments.last().unwrap();
            let s = seg.ident.to_string();
            let generic_args = match &seg.arguments {
                syn::PathArguments::None => vec![],
                syn::PathArguments::AngleBracketed(ab) => {
                    let mut v = vec![];
                    for a in &ab.args {
                        match a {
                            syn::GenericArgument::Type(t) => v.push(parse_tysyn(t, self_ty)?),
                            syn::GenericArgument::Lifetime(_) => {}
                            _ => return err(a.span(), "unsupported generic argument"),
                        }
                    }
                    v
                }
                _ => return err(t.span(), "unsupported type path"),
            };
            if !generic_args.is_empty() {
                return Ok(TySyn::App(s, generic_args));
            }
            match Ty::prim(&s) {
                Some(t) => TySyn::Prim(t),
                None if s == "Self" => match self_ty {
                    Some(t) => t.clone(),
                    None => return err(t.span(), "`Self` is only available in impls"),
                },
                None if s == "str" => TySyn::Str,
                None => match ffi_alias(&s) {
                    Some(t) => TySyn::Prim(t),
                    None => TySyn::Named(s),
                },
            }
        }
        _ => return err(t.span(), "unsupported type"),
    })
}

/// `core::ffi` の C 型の別名 (x86-64 / AArch64 Linux)
fn ffi_alias(s: &str) -> Option<Ty> {
    Some(match s {
        "c_char" | "c_schar" => Ty::I8,
        "c_uchar" => Ty::U8,
        "c_short" => Ty::I16,
        "c_ushort" => Ty::U16,
        "c_int" => Ty::I32,
        "c_uint" => Ty::U32,
        "c_long" | "c_longlong" => Ty::I64,
        "c_ulong" | "c_ulonglong" => Ty::U64,
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct SigSyn {
    pub params: Vec<TySyn>,
    pub ret: TySyn,
    pub symbol: String,
    pub is_extern: bool,
    pub is_main: bool,
    /// impl の中の関数なら self 型の名前
    pub impl_of: Option<String>,
    /// 第 1 引数が self (メソッド呼び出し構文で呼べる)
    pub has_self: bool,
    /// 型パラメータと、それぞれのトレイト境界。trait の中の関数は先頭に `Self` を持つ
    pub generics: Vec<(String, Vec<String>)>,
    /// trait の中で宣言された関数ならその trait 名
    pub trait_of: Option<String>,
    /// 本体があるか (trait の宣言だけの関数は false)
    pub has_body: bool,
    /// `impl Trait for S` の中の関数ならその trait 名
    pub impl_trait: Option<String>,
    /// impl の中の関数なら `Self` の型 (`impl<T> Option<T>` なら `Option<T>`)
    pub self_ty: Option<TySyn>,
}

#[derive(Clone, Debug)]
pub struct FnSig {
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// オブジェクトファイル上のシンボル名
    pub symbol: String,
    pub is_extern: bool,
    pub is_main: bool,
    /// impl の中の関数なら self 型 (struct id)
    pub impl_of: Option<u32>,
    pub has_self: bool,
    pub generics: Vec<(String, Vec<String>)>,
    pub trait_of: Option<String>,
    pub has_body: bool,
    pub impl_trait: Option<String>,
    /// impl の中の関数の `Self` (型パラメータを含みうる)
    pub self_ty: Option<Ty>,
}

impl FnSig {
    /// 単相化が必要か (直接はコンパイルしない)
    pub fn is_generic(&self) -> bool {
        !self.generics.is_empty()
    }
}

/// 関数本体の中で型名を解決するための環境 (`Self` と型パラメータ)
#[derive(Clone, Debug, Default)]
pub struct TyEnv {
    pub self_sid: Option<u32>,
    pub params: Vec<(String, Ty)>,
}

/// enum のバリアント (要約に保存する形)
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct VariantSyn {
    pub name: String,
    pub kind: VariantKind,
    pub fields: Vec<(String, TySyn)>,
    pub discr: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum VariantKind {
    #[default]
    Unit,
    /// `V(a, b)`。フィールド名は "0", "1", ...
    Tuple,
    /// `V { x, y }`
    Struct,
}

pub struct VariantDef {
    pub name: String,
    pub kind: VariantKind,
    /// (名前, 型, enum 先頭からのオフセット)
    pub fields: Vec<(String, Ty, u32)>,
    pub discr: i64,
}

/// struct と enum (名前付きの集約型) の定義。enum も同じ表に入れることで、
/// コピー・引数渡し・impl・deep hash などの処理を struct と共有する。
pub struct StructDef {
    pub name: String,
    /// (名前, 型, オフセット)
    pub fields: Vec<(String, Ty, u32)>,
    pub size: u32,
    pub align: u32,
    /// 定義と、フィールドから推移的に届く struct の定義すべてのハッシュ (レイアウトが変わったら変わる)
    pub deep: Key,
    /// enum ならバリアント (struct なら空)
    pub variants: Vec<VariantDef>,
    pub is_enum: bool,
    /// enum のタグの型 (U8 か I32)。オフセット 0 に置く
    pub tag: Ty,
    /// ジェネリックな定義 (`Option<T>`) なら型パラメータ名。フィールドの型は Param を含み、レイアウトはない
    pub generics: Vec<String>,
    /// インスタンス (`Option<i64>`) なら元の定義と型引数
    pub base: Option<u32>,
    pub args: Vec<Ty>,
}

impl StructDef {
    fn blank(name: String, is_enum: bool) -> StructDef {
        StructDef {
            name,
            fields: vec![],
            size: 0,
            align: 1,
            deep: 0,
            variants: vec![],
            is_enum,
            tag: Ty::U8,
            generics: vec![],
            base: None,
            args: vec![],
        }
    }
}

/// struct / enum の表。後からインスタンス (`Option<i64>` など) を足せる (並列のコード生成中にも)。
/// 要素は Box で持ち、決して削除・移動しないので、`&self` から要素への参照を返せる
pub struct AdtTable {
    v: Mutex<Vec<Box<StructDef>>>,
    inst: Mutex<HashMap<(u32, Vec<Ty>), u32>>,
    tuples: Mutex<HashMap<usize, u32>>,
}

impl AdtTable {
    fn new() -> Self {
        AdtTable { v: Mutex::new(vec![]), inst: Mutex::new(HashMap::new()), tuples: Mutex::new(HashMap::new()) }
    }
    pub fn len(&self) -> usize {
        self.v.lock().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn push(&self, d: StructDef) -> u32 {
        let mut g = self.v.lock().unwrap();
        g.push(Box::new(d));
        g.len() as u32 - 1
    }
    fn get_mut(&mut self, i: usize) -> &mut StructDef {
        &mut self.v.get_mut().unwrap()[i]
    }
}

impl std::ops::Index<usize> for AdtTable {
    type Output = StructDef;
    fn index(&self, i: usize) -> &StructDef {
        let g = self.v.lock().unwrap();
        let p: *const StructDef = &*g[i];
        drop(g);
        // SAFETY: 要素は Box に入っていて削除されない (Vec の再確保で動くのは Box のポインタだけ)。
        // 公開後の要素を変更するのは &mut self を取る get_mut (build 中) だけ
        unsafe { &*p }
    }
}

/// レイアウト計算の入力
#[derive(Clone)]
enum AdtShape {
    Struct(Vec<(String, TySyn)>),
    Enum(Vec<VariantSyn>),
}

/// アイテム 1 個分の要約 (キャッシュされる)。ここに入っている情報だけで
/// 他の関数のコード生成ができる。
#[derive(Clone, Debug, Default)]
pub struct Summary {
    /// (名前, シグネチャ)。fn アイテムなら 1 個、extern ブロックなら複数
    pub fns: Vec<(String, SigSyn)>,
    /// (名前, フィールド)。struct アイテムなら 1 個
    pub structs: Vec<(String, Vec<(String, TySyn)>)>,
    /// impl ヘッダなら self 型の名前
    pub impl_self: Option<String>,
    /// (名前, バリアント)。enum アイテムなら 1 個
    pub enums: Vec<(String, Vec<VariantSyn>)>,
    /// trait ヘッダならその名前
    pub trait_decl: Option<String>,
    /// `impl Trait for S` のヘッダなら trait 名
    pub impl_trait: Option<String>,
    /// `#[derive(..)]` された (型名, trait 名)。derive した trait は実装扱いにする
    pub derives: Vec<(String, Vec<String>)>,
    /// このアイテムの struct / enum の型パラメータ (`struct W<T>` なら ["T"])
    pub adt_generics: Vec<String>,
    /// impl ヘッダの型パラメータと self 型 (`impl<T> Option<T>`)
    pub impl_generics: Vec<(String, Vec<String>)>,
    pub impl_self_ty: Option<TySyn>,
}

/// impl / trait のメンバの所有者
pub struct Owner {
    /// メソッド名の前に付ける名前 (impl なら self 型の名前、trait なら trait 名)
    pub base: String,
    /// `Self` の中身
    pub self_ty: TySyn,
    /// `impl Trait for S` の trait
    pub impl_trait: Option<String>,
    /// impl の型パラメータ (trait なら `Self`)
    pub generics: Vec<(String, Vec<String>)>,
    pub is_trait: bool,
}

/// `#[derive(A, B)]` の名前
fn derive_names(attrs: &[syn::Attribute]) -> Vec<String> {
    let mut out = vec![];
    for a in attrs {
        if a.path().is_ident("derive") {
            let _ = a.parse_nested_meta(|m| {
                if let Some(id) = m.path.get_ident() {
                    out.push(id.to_string());
                }
                Ok(())
            });
        }
    }
    out
}

pub struct Program {
    /// 全関数 (ユーザ定義 + extern)。index が u32 の関数 ID になる
    pub names: Vec<String>,
    pub index: HashMap<String, u32>,
    pub sigs: Vec<FnSig>,
    /// ユーザ定義関数を定義しているアイテム番号 (extern は None)
    pub item_of: Vec<Option<u32>>,
    pub structs: AdtTable,
    pub struct_index: HashMap<String, u32>,
    /// 名前の最後のセグメント → 関数 ID 群 (`len` → `A::len`, `B::len`)。参照の過大近似用
    pub by_last: HashMap<String, Vec<u32>>,
    /// trait 名 → その trait で宣言された関数
    pub traits: HashMap<String, Vec<u32>>,
    /// (型, trait) の実装
    pub impls: HashSet<(u32, String)>,
}

/// 単相化したインスタンス (ジェネリック関数 base に型引数 args を入れたもの)
pub struct Inst {
    pub base: u32,
    pub args: Vec<Ty>,
    pub sig: FnSig,
}

/// インスタンス表。関数 ID は Program の関数の後ろに続けて振る。
/// コード生成中に (並列に) 要求されたものを pending に積み、ドライバが不動点まで処理する
pub struct Insts {
    pub n_fns: u32,
    pub list: Vec<Inst>,
    map: HashMap<(u32, Vec<Ty>), u32>,
    pub pending: Vec<u32>,
}

impl Insts {
    pub fn new(prog: &Program) -> Insts {
        Insts { n_fns: prog.names.len() as u32, list: vec![], map: HashMap::new(), pending: vec![] }
    }

    pub fn get(&self, id: u32) -> Option<&Inst> {
        id.checked_sub(self.n_fns).and_then(|i| self.list.get(i as usize))
    }

    /// インスタンスを要求する (なければ作り、pending に積む)。トレイト境界もここで確かめる
    pub fn request(&mut self, prog: &Program, base: u32, args: Vec<Ty>) -> R<u32> {
        if let Some(&id) = self.map.get(&(base, args.clone())) {
            return Ok(id);
        }
        let b = &prog.sigs[base as usize];
        if b.generics.len() != args.len() {
            return Err(format!("`{}` takes {} type arguments", prog.names[base as usize], b.generics.len()));
        }
        for ((pn, bounds), &t) in b.generics.iter().zip(&args) {
            for tr in bounds {
                if !prog.implements(t, tr) {
                    return Err(format!("the trait bound `{}: {tr}` is not satisfied (for `{pn}`)", prog.tn(t)));
                }
            }
        }
        let names: Vec<String> = args.iter().map(|&t| prog.tn(t)).collect();
        let impl_of = if b.trait_of.is_some() {
            match args[0] {
                Ty::Struct(sid) => Some(sid),
                _ => None,
            }
        } else {
            match b.self_ty.map(|t| prog.subst(t, &args)) {
                Some(Ty::Struct(sid)) => Some(sid),
                _ => b.impl_of,
            }
        };
        let sig = FnSig {
            params: b.params.iter().map(|&t| prog.subst(t, &args)).collect(),
            ret: prog.subst(b.ret, &args),
            symbol: format!("{}<{}>", b.symbol, names.join(",")),
            is_extern: false,
            is_main: false,
            impl_of,
            has_self: b.has_self,
            generics: vec![],
            trait_of: None,
            has_body: true,
            impl_trait: None,
            self_ty: impl_of.map(Ty::Struct),
        };
        let id = self.n_fns + self.list.len() as u32;
        self.list.push(Inst { base, args: args.clone(), sig });
        self.map.insert((base, args), id);
        self.pending.push(id);
        Ok(id)
    }

    /// 関数 (またはインスタンス) id のシグネチャ
    pub fn sig<'a>(&'a self, prog: &'a Program, id: u32) -> &'a FnSig {
        match self.get(id) {
            Some(i) => &i.sig,
            None => &prog.sigs[id as usize],
        }
    }

    /// 本体を持つ関数 (インスタンスなら元のジェネリック関数) の id
    pub fn body_of(&self, id: u32) -> u32 {
        self.get(id).map(|i| i.base).unwrap_or(id)
    }

    /// 本体を型検査するときの型環境
    pub fn env(&self, prog: &Program, id: u32) -> TyEnv {
        match self.get(id) {
            Some(i) => {
                let b = &prog.sigs[i.base as usize];
                TyEnv {
                    self_sid: i.sig.impl_of,
                    params: b.generics.iter().map(|g| g.0.clone()).zip(i.args.iter().copied()).collect(),
                }
            }
            None => TyEnv { self_sid: prog.sigs[id as usize].impl_of, params: vec![] },
        }
    }
}

/// syn で 1 アイテムを parse する。エラー位置はファイル全体での行に補正する。
/// impl のヘッダは空の impl として、メンバは `Item::Fn` に直して返す。
pub fn parse_item(src: &str, it: &SrcItem) -> R<syn::Item> {
    let text = &src[it.start..it.end];
    let fix = |e: syn::Error| {
        let s = e.span().start();
        format!("{}:{}: {e}", s.line + it.line - 1, s.column + 1)
    };
    match it.kind {
        ItemKind::Normal => syn::parse_str::<syn::Item>(text).map_err(fix),
        ItemKind::ImplHeader | ItemKind::TraitHeader => {
            syn::parse_str::<syn::Item>(&format!("{text}}}")).map_err(fix)
        }
        ItemKind::TraitMember(_) => {
            let f = parse_trait_member(src, it)?;
            match f.default {
                Some(block) => Ok(syn::Item::Fn(syn::ItemFn {
                    attrs: f.attrs,
                    vis: syn::Visibility::Inherited,
                    sig: f.sig,
                    block: Box::new(block),
                })),
                None => err(f.sig.span(), "trait method has no default body").map_err(|e| relocate_err(e, it)),
            }
        }
        ItemKind::ImplMember(_) => match syn::parse_str::<syn::ImplItem>(text).map_err(fix)? {
            syn::ImplItem::Fn(f) => Ok(syn::Item::Fn(syn::ItemFn {
                attrs: f.attrs,
                vis: f.vis,
                sig: f.sig,
                block: Box::new(f.block),
            })),
            other => err(other.span(), "only fns are supported in impl blocks")
                .map_err(|e| relocate_err(e, it)),
        },
    }
}

pub fn parse_trait_member(src: &str, it: &SrcItem) -> R<syn::TraitItemFn> {
    let text = &src[it.start..it.end];
    match syn::parse_str::<syn::TraitItem>(text) {
        Ok(syn::TraitItem::Fn(f)) => Ok(f),
        Ok(other) => err(other.span(), "only fns are supported in traits").map_err(|e| relocate_err(e, it)),
        Err(e) => {
            let s = e.span().start();
            Err(format!("{}:{}: {e}", s.line + it.line - 1, s.column + 1))
        }
    }
}

/// アイテムを parse して要約する。owner は impl / trait のメンバのとき、その self 型 / trait 名。
/// codegen でも使う AST (本体のある関数) があれば一緒に返す
pub fn parse_and_summarize(src: &str, it: &SrcItem, owner: Option<&Owner>) -> R<(Summary, Option<syn::Item>)> {
    if let ItemKind::TraitMember(_) = it.kind {
        let ow = owner.ok_or("trait member without trait")?;
        let f = parse_trait_member(src, it)?;
        let mut sig = parse_sig(&f.sig, false, false, Some(ow)).map_err(|e| relocate_err(e, it))?;
        sig.has_body = f.default.is_some();
        let sm = Summary { fns: vec![(format!("{}::{}", ow.base, f.sig.ident), sig)], ..Default::default() };
        let item = f.default.map(|block| {
            syn::Item::Fn(syn::ItemFn { attrs: f.attrs, vis: syn::Visibility::Inherited, sig: f.sig, block: Box::new(block) })
        });
        return Ok((sm, item));
    }
    let item = parse_item(src, it)?;
    let sm = summarize(&item, owner).map_err(|e| relocate_err(e, it))?;
    let keep = matches!(item, syn::Item::Fn(_));
    Ok((sm, keep.then_some(item)))
}

/// ジェネリックな struct / enum の型パラメータ名 (境界は使わない)
fn adt_params(g: &syn::Generics) -> R<Vec<String>> {
    let mut v = vec![];
    collect_generics(g, &mut v)?;
    Ok(v.into_iter().map(|x| x.0).collect())
}

/// "L:C: msg" 形式のエラーの行をアイテム開始行だけずらす。
pub fn relocate_err(e: String, it: &SrcItem) -> String {
    if let Some((l, rest)) = e.split_once(':') {
        if let Ok(l) = l.parse::<usize>() {
            return format!("{}:{rest}", l + it.line - 1);
        }
    }
    e
}

/// `owner` は impl のメンバのとき、その impl
pub fn summarize(item: &syn::Item, owner: Option<&Owner>) -> R<Summary> {
    let mut sm = Summary::default();
    match item {
        syn::Item::Fn(f) if owner.is_some() => {
            let ow = owner.unwrap();
            let sig = parse_sig(&f.sig, false, false, Some(ow))?;
            sm.fns.push((format!("{}::{}", ow.base, f.sig.ident), sig));
        }
        syn::Item::Trait(t) => {
            only_lifetimes(&t.generics)?;
            sm.trait_decl = Some(t.ident.to_string());
        }
        syn::Item::Impl(im) => {
            if let Some((bang, tr, _)) = &im.trait_ {
                if bang.is_some() {
                    return err(tr.span(), "negative impls are not supported");
                }
                let Some(id) = tr.get_ident() else {
                    return err(tr.span(), "unsupported trait path");
                };
                sm.impl_trait = Some(id.to_string());
            }
            collect_generics(&im.generics, &mut sm.impl_generics)?;
            let syn::Type::Path(tp) = &*im.self_ty else {
                return err(im.self_ty.span(), "unsupported impl self type");
            };
            if tp.path.segments.len() != 1 {
                return err(im.self_ty.span(), "unsupported impl self type");
            }
            sm.impl_self = Some(tp.path.segments[0].ident.to_string());
            sm.impl_self_ty = Some(parse_tysyn(&im.self_ty, None)?);
        }
        syn::Item::Fn(f) => {
            let name = f.sig.ident.to_string();
            let is_main = name == "main";
            let sig = parse_sig(&f.sig, false, is_main, None)?;
            if is_main && (!sig.params.is_empty() || sig.ret != TySyn::Prim(Ty::Unit)) {
                return err(f.sig.span(), "`main` must be `fn main()`");
            }
            sm.fns.push((name, sig));
        }
        syn::Item::ForeignMod(m) => {
            for fi in &m.items {
                let syn::ForeignItem::Fn(f) = fi else {
                    return err(fi.span(), "only fns are supported in extern blocks");
                };
                sm.fns.push((f.sig.ident.to_string(), parse_sig(&f.sig, true, false, None)?));
            }
        }
        syn::Item::Enum(e) => {
            sm.adt_generics = adt_params(&e.generics)?;
            let sn = e.ident.to_string();
            let selft = self_of(&sn, &sm.adt_generics);
            let sn_ty = Some(&selft);
            let mut vs = vec![];
            for v in &e.variants {
                let (kind, fields) = match &v.fields {
                    syn::Fields::Unit => (VariantKind::Unit, vec![]),
                    syn::Fields::Unnamed(u) => {
                        let mut fs = vec![];
                        for (i, f) in u.unnamed.iter().enumerate() {
                            fs.push((i.to_string(), parse_tysyn(&f.ty, sn_ty)?));
                        }
                        (VariantKind::Tuple, fs)
                    }
                    syn::Fields::Named(nf) => {
                        let mut fs = vec![];
                        for f in &nf.named {
                            fs.push((f.ident.as_ref().unwrap().to_string(), parse_tysyn(&f.ty, sn_ty)?));
                        }
                        (VariantKind::Struct, fs)
                    }
                };
                let discr = match &v.discriminant {
                    None => None,
                    Some((_, ex)) => Some(const_int(ex)?),
                };
                vs.push(VariantSyn { name: v.ident.to_string(), kind, fields, discr });
            }
            let d = derive_names(&e.attrs);
            if !d.is_empty() {
                sm.derives.push((sn.clone(), d));
            }
            sm.enums.push((sn, vs));
        }
        syn::Item::Struct(s) => {
            sm.adt_generics = adt_params(&s.generics)?;
            let selft = self_of(&s.ident.to_string(), &sm.adt_generics);
            let mut fields = vec![];
            match &s.fields {
                syn::Fields::Named(nf) => {
                    for f in &nf.named {
                        let t = parse_tysyn(&f.ty, Some(&selft))?;
                        fields.push((f.ident.as_ref().unwrap().to_string(), t));
                    }
                }
                // タプル構造体: フィールド名は "0", "1", ...
                syn::Fields::Unnamed(u) => {
                    for (i, f) in u.unnamed.iter().enumerate() {
                        fields.push((i.to_string(), parse_tysyn(&f.ty, Some(&selft))?));
                    }
                }
                syn::Fields::Unit => {}
            }
            let d = derive_names(&s.attrs);
            if !d.is_empty() {
                sm.derives.push((s.ident.to_string(), d));
            }
            sm.structs.push((s.ident.to_string(), fields));
        }
        _ => return err(item.span(), "unsupported item"),
    }
    Ok(sm)
}

/// struct / enum 定義の中での `Self`
fn self_of(name: &str, generics: &[String]) -> TySyn {
    if generics.is_empty() {
        TySyn::Named(name.to_string())
    } else {
        TySyn::App(name.to_string(), generics.iter().map(|g| TySyn::Named(g.clone())).collect())
    }
}

impl Program {
    pub fn build(items: &[SrcItem], sums: &[Summary]) -> R<Program> {
        let mut p = Program {
            names: vec![],
            index: HashMap::new(),
            sigs: vec![],
            item_of: vec![],
            structs: AdtTable::new(),
            struct_index: HashMap::new(),
            by_last: HashMap::new(),
            traits: HashMap::new(),
            impls: HashSet::new(),
        };
        // 1. struct / enum 名を登録
        let mut sdefs: Vec<(usize, String, AdtShape, Vec<String>)> = vec![];
        for (ii, (it, sm)) in items.iter().zip(sums).enumerate() {
            let adts = sm
                .structs
                .iter()
                .map(|(n, f)| (n, AdtShape::Struct(f.clone())))
                .chain(sm.enums.iter().map(|(n, v)| (n, AdtShape::Enum(v.clone()))));
            for (name, shape) in adts {
                if p.struct_index.contains_key(name) {
                    return Err(format!("{}:1: duplicate definition of `{name}`", it.line));
                }
                p.struct_index.insert(name.clone(), sdefs.len() as u32);
                sdefs.push((ii, name.clone(), shape, sm.adt_generics.clone()));
            }
        }
        // 2. フィールド型を解決し、依存順にレイアウトを決める
        for (_, n, sh, g) in &sdefs {
            let mut d = StructDef::blank(n.clone(), matches!(sh, AdtShape::Enum(_)));
            d.generics = g.clone();
            p.structs.push(d);
        }
        // ジェネリックな定義を先に (フィールドの型を Param で解決するだけ。レイアウトはインスタンスごと)
        let mut state = vec![0u8; sdefs.len()]; // 0 未処理, 1 処理中, 2 済み
        for id in 0..sdefs.len() {
            if !sdefs[id].3.is_empty() {
                p.layout(id, &sdefs, items, &mut state)?;
            }
        }
        for id in 0..sdefs.len() {
            p.layout(id, &sdefs, items, &mut state)?;
        }
        // deep hash = 値・参照を問わずフィールドから推移的に届く全 struct の定義ハッシュ。
        // 参照経由 (`&'a Other`) でもフィールドオフセットはコードに埋まるので含める。
        let n_defs = sdefs.len();
        let deeps: Vec<Key> = (0..n_defs)
            .map(|id| {
                let mut seen = HashSet::new();
                let mut stack = vec![Ty::Struct(id as u32)];
                let mut hs = vec![];
                while let Some(t) = stack.pop() {
                    match t {
                        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) => stack.push(*i),
                        Ty::Struct(sid) => {
                            if !seen.insert(sid) {
                                continue;
                            }
                            let sd = &p.structs[sid as usize];
                            if let Some(b) = sd.base {
                                stack.push(Ty::Struct(b));
                                stack.extend(sd.args.iter().copied());
                                continue;
                            }
                            if (sid as usize) < n_defs {
                                hs.push(items[sdefs[sid as usize].0].hash);
                            }
                            let vf = sd.variants.iter().flat_map(|v| v.fields.iter());
                            stack.extend(sd.fields.iter().chain(vf).map(|f| f.1));
                        }
                        _ => {}
                    }
                }
                hs.sort_unstable();
                let mut h = KeyHasher::new();
                h.add(&hs);
                h.finish()
            })
            .collect();
        for (i, d) in deeps.into_iter().enumerate() {
            p.structs.get_mut(i).deep = d;
        }
        // 3. 関数
        for (ii, (it, sm)) in items.iter().zip(sums).enumerate() {
            for (name, ss) in &sm.fns {
                if p.index.contains_key(name) {
                    return Err(format!("{}:1: duplicate definition of `{name}`", it.line));
                }
                let gnames: Vec<&str> = ss.generics.iter().map(|g| g.0.as_str()).collect();
                let mut params = vec![];
                for t in &ss.params {
                    params.push(p.resolve_g(t, &gnames).map_err(|e| format!("{}:1: {e}", it.line))?);
                }
                let ret = p.resolve_g(&ss.ret, &gnames).map_err(|e| format!("{}:1: {e}", it.line))?;
                let self_ty = match &ss.self_ty {
                    Some(t) => Some(p.resolve_g(t, &gnames).map_err(|e| format!("{}:1: {e}", it.line))?),
                    None => None,
                };
                let impl_of = match &ss.impl_of {
                    Some(n) => match p.struct_index.get(n) {
                        Some(&sid) => Some(sid),
                        None => return Err(format!("{}:1: cannot find struct `{n}` for impl", it.line)),
                    },
                    None => None,
                };
                if ss.is_extern && params.iter().chain([&ret]).any(|t| t.is_agg()) {
                    return Err(format!("{}:1: passing structs by value to extern fns is not supported", it.line));
                }
                let last = name.rsplit("::").next().unwrap().to_string();
                p.by_last.entry(last).or_default().push(p.names.len() as u32);
                p.index.insert(name.clone(), p.names.len() as u32);
                p.names.push(name.clone());
                p.item_of.push((!ss.is_extern).then_some(ii as u32));
                p.sigs.push(FnSig {
                    params,
                    ret,
                    symbol: ss.symbol.clone(),
                    is_extern: ss.is_extern,
                    is_main: ss.is_main,
                    impl_of,
                    has_self: ss.has_self,
                    generics: ss.generics.clone(),
                    trait_of: ss.trait_of.clone(),
                    has_body: ss.has_body,
                    impl_trait: ss.impl_trait.clone(),
                    self_ty,
                });
                if let Some(tr) = &ss.trait_of {
                    p.traits.entry(tr.clone()).or_default().push(p.names.len() as u32 - 1);
                }
            }
            if let Some(tr) = &sm.trait_decl {
                p.traits.entry(tr.clone()).or_default();
            }
        }
        // 4. trait の実装 (impl ヘッダの要約から)
        for (it, sm) in items.iter().zip(sums) {
            let (Some(sn), Some(tr)) = (&sm.impl_self, &sm.impl_trait) else { continue };
            let Some(&sid) = p.struct_index.get(sn) else {
                return Err(format!("{}:1: cannot find type `{sn}`", it.line));
            };
            if !p.traits.contains_key(tr) {
                return Err(format!("{}:1: cannot find trait `{tr}`", it.line));
            }
            if !p.impls.insert((sid, tr.clone())) {
                return Err(format!("{}:1: conflicting implementations of trait `{tr}` for `{sn}`", it.line));
            }
        }
        // derive は実装扱い (Clone / Copy など。中身は常にビットコピーなので本体は要らない)
        for sm in sums {
            for (tn, ds) in &sm.derives {
                if let Some(&sid) = p.struct_index.get(tn) {
                    for d in ds {
                        p.impls.insert((sid, d.clone()));
                    }
                }
            }
        }
        // 必須メソッドが実装されているか
        for (sid, tr) in &p.impls {
            let Some(decls) = p.traits.get(tr) else { continue }; // derive した組み込み trait
            for &d in decls {
                let m = p.names[d as usize].rsplit("::").next().unwrap();
                if !p.sigs[d as usize].has_body && p.method(*sid, m).is_none() {
                    return Err(format!(
                        "not all trait items implemented, missing: `{m}` in `impl {tr} for {}`",
                        p.structs[*sid as usize].name
                    ));
                }
            }
        }
        if !p.index.contains_key("main") {
            return Err("no `main` function".into());
        }
        Ok(p)
    }

    /// 型 t が trait tr を実装しているか
    pub fn implements(&self, t: Ty, tr: &str) -> bool {
        match t {
            Ty::Struct(sid) => self.impls.contains(&(self.base_of(sid), tr.to_string())),
            Ty::Array(e, _) => tr == "Sized" || (matches!(tr, "Copy" | "Clone") && self.implements(*e, tr)),
            Ty::Slice(_) | Ty::Str => false,
            // 組み込み型はマーカー trait だけ
            Ty::Ref(true, _) => tr == "Sized",
            _ => matches!(tr, "Copy" | "Clone" | "Sized"),
        }
    }

    /// struct sid に名前 name のメソッドが見つからないとき、実装している trait のデフォルトメソッド
    pub fn default_method(&self, sid: u32, name: &str) -> Option<u32> {
        let sid = self.base_of(sid);
        let mut found = None;
        for (s, tr) in &self.impls {
            if *s != sid {
                continue;
            }
            let Some(decls) = self.traits.get(tr) else { continue };
            for &d in decls {
                if self.sigs[d as usize].has_body && self.names[d as usize].rsplit("::").next() == Some(name) {
                    found = Some(d);
                }
            }
        }
        found
    }

    /// TySyn → Ty。generics に含まれる名前は型パラメータになる
    pub fn resolve_g(&self, t: &TySyn, generics: &[&str]) -> R<Ty> {
        self.resolve_with(t, &|n| generics.iter().position(|g| *g == n).map(|i| Ty::Param(i as u8)))
    }

    /// 名前の解決を names に先に聞いてから struct 名を引く
    fn resolve_with(&self, t: &TySyn, names: &dyn Fn(&str) -> Option<Ty>) -> R<Ty> {
        let r = |t: &TySyn| self.resolve_with(t, names);
        Ok(match t {
            TySyn::Prim(t) => *t,
            TySyn::Ref(m, inner) => Ty::Ref(*m, intern(r(inner)?)),
            TySyn::Ptr(m, inner) => Ty::Ptr(*m, intern(r(inner)?)),
            TySyn::Slice(inner) => Ty::Slice(intern(r(inner)?)),
            TySyn::Str => Ty::Str,
            TySyn::Array(inner, len) => {
                let n = match len {
                    ArrLen::Lit(n) => *n,
                    ArrLen::Const(c) => self.const_usize(c)?,
                };
                Ty::Array(intern(r(inner)?), n as u32)
            }
            TySyn::Named(n) => {
                if let Some(t) = names(n) {
                    return Ok(t);
                }
                match self.struct_index.get(n) {
                    Some(&id) if self.structs[id as usize].generics.is_empty() => Ty::Struct(id),
                    Some(_) => return Err(format!("missing generics for `{n}`")),
                    None => return Err(format!("cannot find type `{n}`")),
                }
            }
            TySyn::App(n, args) => {
                let Some(&id) = self.struct_index.get(n) else {
                    return Err(format!("cannot find type `{n}`"));
                };
                let args = args.iter().map(r).collect::<R<Vec<Ty>>>()?;
                Ty::Struct(self.instantiate(id, args)?)
            }
            TySyn::Tuple(es) => {
                let args = es.iter().map(r).collect::<R<Vec<Ty>>>()?;
                let base = self.tuple_base(args.len());
                Ty::Struct(self.instantiate(base, args)?)
            }
        })
    }

    /// 配列の長さに使う const (要約の const 表から)
    fn const_usize(&self, name: &str) -> R<u64> {
        Err(format!("cannot find constant `{name}` (array lengths from consts are not supported yet)"))
    }

    /// n 要素のタプルの (組み込みの) ジェネリック定義
    pub fn tuple_base(&self, n: usize) -> u32 {
        if let Some(&id) = self.structs.tuples.lock().unwrap().get(&n) {
            return id;
        }
        let mut d = StructDef::blank(format!("#tuple{n}"), false);
        d.generics = (0..n).map(|i| format!("T{i}")).collect();
        d.fields = (0..n).map(|i| (i.to_string(), Ty::Param(i as u8), 0)).collect();
        let mut h = KeyHasher::new();
        h.add(&("tuple", n));
        d.deep = h.finish();
        let mut g = self.structs.tuples.lock().unwrap();
        if let Some(&id) = g.get(&n) {
            return id;
        }
        let id = self.structs.push(d);
        g.insert(n, id);
        id
    }

    pub fn is_tuple(&self, sid: u32) -> bool {
        self.structs[self.base_of(sid) as usize].name.starts_with("#tuple")
    }

    /// インスタンスなら元の定義
    pub fn base_of(&self, sid: u32) -> u32 {
        self.structs[sid as usize].base.unwrap_or(sid)
    }

    /// ジェネリックな struct / enum に型引数を入れる。型引数がすべて具体型ならレイアウトも決める。
    /// 同じ (定義, 型引数) には同じ id を返す
    pub fn instantiate(&self, base: u32, args: Vec<Ty>) -> R<u32> {
        if let Some(&id) = self.structs.inst.lock().unwrap().get(&(base, args.clone())) {
            return Ok(id);
        }
        let b = &self.structs[base as usize];
        if b.generics.len() != args.len() {
            return Err(format!("`{}` takes {} type arguments, but {} were given", b.name, b.generics.len(), args.len()));
        }
        let symbolic = args.iter().any(|&t| self.has_param(t));
        let names: Vec<String> = args.iter().map(|&t| self.tn(t)).collect();
        let name = if b.name.starts_with("#tuple") {
            if names.len() == 1 { format!("({},)", names[0]) } else { format!("({})", names.join(", ")) }
        } else {
            format!("{}<{}>", b.name, names.join(", "))
        };
        let mut d = StructDef::blank(name, b.is_enum);
        d.base = Some(base);
        d.args = args.clone();
        d.tag = b.tag;
        if !symbolic {
            if b.is_enum {
                let mut vs = vec![];
                for v in &b.variants {
                    let fs = v.fields.iter().map(|(n, t, _)| (n.clone(), self.subst(*t, &args))).collect();
                    vs.push((v.name.clone(), v.kind, fs, v.discr));
                }
                let (vs, size, align) = self.enum_layout(vs, b.tag);
                d.variants = vs;
                d.size = size;
                d.align = align;
            } else {
                let fs: Vec<(String, Ty)> = b.fields.iter().map(|(n, t, _)| (n.clone(), self.subst(*t, &args))).collect();
                let (fs, size, align) = self.struct_layout(fs);
                d.fields = fs;
                d.size = size;
                d.align = align;
            }
        }
        let mut g = self.structs.inst.lock().unwrap();
        if let Some(&id) = g.get(&(base, args.clone())) {
            return Ok(id);
        }
        let id = self.structs.push(d);
        g.insert((base, args), id);
        Ok(id)
    }

    /// フィールドを宣言順に自然なアラインメントで並べる (repr(C) 相当)
    fn struct_layout(&self, fields: Vec<(String, Ty)>) -> (Vec<(String, Ty, u32)>, u32, u32) {
        let mut out = vec![];
        let (mut off, mut align) = (0u32, 1u32);
        for (n, t) in fields {
            let (sz, al) = self.size_align(t);
            off = off.next_multiple_of(al);
            out.push((n, t, off));
            off += sz;
            align = align.max(al);
        }
        (out, off.next_multiple_of(align), align)
    }

    /// タグ (判別値) は先頭、ペイロードは全バリアントで共有する領域に置く
    fn enum_layout(&self, vs: Vec<(String, VariantKind, Vec<(String, Ty)>, i64)>, tag: Ty) -> (Vec<VariantDef>, u32, u32) {
        let tag_size = tag.bits() / 8;
        let mut laid = vec![];
        let (mut pmax, mut palign) = (0u32, 1u32);
        for (_, _, fs, _) in &vs {
            let (f, sz, al) = self.struct_layout(fs.clone());
            pmax = pmax.max(sz);
            palign = palign.max(al);
            laid.push(f);
        }
        let poff = tag_size.next_multiple_of(palign);
        let align = palign.max(tag_size);
        let out = vs
            .into_iter()
            .zip(laid)
            .map(|((name, kind, _, discr), f)| VariantDef {
                name,
                kind,
                fields: f.into_iter().map(|(n, t, o)| (n, t, o + poff)).collect(),
                discr,
            })
            .collect();
        (out, (poff + pmax).next_multiple_of(align), align)
    }

    /// 型の中に型パラメータが残っているか
    pub fn has_param(&self, t: Ty) -> bool {
        match t {
            Ty::Param(_) => true,
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) => self.has_param(*i),
            Ty::Struct(sid) => self.structs[sid as usize].args.iter().any(|&a| self.has_param(a)),
            _ => false,
        }
    }

    /// 型パラメータに型引数を入れる
    pub fn subst(&self, t: Ty, args: &[Ty]) -> Ty {
        match t {
            Ty::Param(i) => args[i as usize],
            Ty::Ref(m, i) => Ty::Ref(m, intern(self.subst(*i, args))),
            Ty::Ptr(m, i) => Ty::Ptr(m, intern(self.subst(*i, args))),
            Ty::Array(i, n) => Ty::Array(intern(self.subst(*i, args)), n),
            Ty::Slice(i) => Ty::Slice(intern(self.subst(*i, args))),
            Ty::Struct(sid) if self.has_param(t) => {
                let sd = &self.structs[sid as usize];
                let na: Vec<Ty> = sd.args.iter().map(|&a| self.subst(a, args)).collect();
                match self.instantiate(sd.base.unwrap(), na) {
                    Ok(id) => Ty::Struct(id),
                    Err(_) => t,
                }
            }
            t => t,
        }
    }

    /// 分かっている型パラメータだけ置き換える (分からないものは Param のまま)
    pub fn partial_subst(&self, t: Ty, sol: &[Option<Ty>]) -> Ty {
        let args: Vec<Ty> = (0..sol.len()).map(|i| sol[i].unwrap_or(Ty::Param(i as u8))).collect();
        if args.is_empty() { t } else { self.subst(t, &args) }
    }

    /// 仮引数型 pt (型パラメータを含む) と実引数型 at を突き合わせて型パラメータを解く
    pub fn match_ty(&self, pt: Ty, at: Ty, sol: &mut [Option<Ty>]) -> bool {
        match (pt, at) {
            (Ty::Param(i), t) => match sol.get(i as usize).copied().flatten() {
                Some(prev) => prev == t,
                None => {
                    if let Some(s) = sol.get_mut(i as usize) {
                        *s = Some(t);
                    }
                    true
                }
            },
            (Ty::Ref(_, a), Ty::Ref(_, b)) | (Ty::Ptr(_, a), Ty::Ptr(_, b)) | (Ty::Slice(a), Ty::Slice(b)) => {
                self.match_ty(*a, *b, sol)
            }
            (Ty::Array(a, n), Ty::Array(b, m)) => n == m && self.match_ty(*a, *b, sol),
            (Ty::Struct(a), Ty::Struct(b)) if a != b => {
                let (sa, sb) = (&self.structs[a as usize], &self.structs[b as usize]);
                match (sa.base, sb.base) {
                    (Some(x), Some(y)) if x == y => {
                        sa.args.iter().zip(&sb.args).all(|(&p, &q)| self.match_ty(p, q, sol))
                    }
                    _ => false,
                }
            }
            (a, b) => a == b,
        }
    }

    /// Ty → TySyn (キャッシュにインスタンスを名前で書くため)
    pub fn ty_syn(&self, t: Ty) -> TySyn {
        match t {
            Ty::Ref(m, i) => TySyn::Ref(m, Box::new(self.ty_syn(*i))),
            Ty::Ptr(m, i) => TySyn::Ptr(m, Box::new(self.ty_syn(*i))),
            Ty::Slice(i) => TySyn::Slice(Box::new(self.ty_syn(*i))),
            Ty::Array(i, n) => TySyn::Array(Box::new(self.ty_syn(*i)), ArrLen::Lit(n as u64)),
            Ty::Str => TySyn::Str,
            Ty::Struct(id) => {
                let sd = &self.structs[id as usize];
                match sd.base {
                    Some(b) if self.is_tuple(b) => TySyn::Tuple(sd.args.iter().map(|&a| self.ty_syn(a)).collect()),
                    Some(b) => TySyn::App(
                        self.structs[b as usize].name.clone(),
                        sd.args.iter().map(|&a| self.ty_syn(a)).collect(),
                    ),
                    None => TySyn::Named(sd.name.clone()),
                }
            }
            t => TySyn::Prim(t),
        }
    }

    fn layout(
        &mut self,
        id: usize,
        sdefs: &[(usize, String, AdtShape, Vec<String>)],
        items: &[SrcItem],
        state: &mut [u8],
    ) -> R<()> {
        match state[id] {
            2 => return Ok(()),
            1 => return Err(format!("recursive type `{}` has infinite size", sdefs[id].1)),
            _ => {}
        }
        state[id] = 1;
        let (ii, _, shape, generics) = &sdefs[id];
        let line = items[*ii].line;
        let gnames: Vec<&str> = generics.iter().map(|s| s.as_str()).collect();
        // 値として含む型 (参照・ポインタの先は除く) を先にレイアウトしておく
        let fields_of = |sh: &AdtShape| -> Vec<TySyn> {
            match sh {
                AdtShape::Struct(f) => f.iter().map(|x| x.1.clone()).collect(),
                AdtShape::Enum(vs) => vs.iter().flat_map(|v| v.fields.iter().map(|x| x.1.clone())).collect(),
            }
        };
        let mut todo: Vec<(TySyn, Vec<String>)> = fields_of(shape).into_iter().map(|t| (t, generics.clone())).collect();
        let mut visited_bases = HashSet::new();
        while let Some((t, gs)) = todo.pop() {
            match t {
                TySyn::Named(n) if !gs.contains(&n) => {
                    if let Some(&sid) = self.struct_index.get(&n) {
                        if sdefs[sid as usize].3.is_empty() {
                            self.layout(sid as usize, sdefs, items, state)?;
                        }
                    }
                }
                TySyn::App(n, args) => {
                    if let Some(&sid) = self.struct_index.get(&n) {
                        if visited_bases.insert(sid) {
                            let bg = sdefs[sid as usize].3.clone();
                            todo.extend(fields_of(&sdefs[sid as usize].2).into_iter().map(|t| (t, bg.clone())));
                        }
                    }
                    todo.extend(args.into_iter().map(|a| (a, gs.clone())));
                }
                TySyn::Tuple(es) => todo.extend(es.into_iter().map(|a| (a, gs.clone()))),
                TySyn::Array(e, _) => todo.push((*e, gs)),
                _ => {}
            }
        }
        let res = |this: &Program, fs: &[(String, TySyn)]| -> R<Vec<(String, Ty)>> {
            fs.iter()
                .map(|(n, t)| Ok((n.clone(), this.resolve_g(t, &gnames).map_err(|e| format!("{line}:1: {e}"))?)))
                .collect()
        };
        let generic = !generics.is_empty();
        match shape {
            AdtShape::Struct(fields) => {
                let fs = res(self, fields)?;
                if generic {
                    self.structs.get_mut(id).fields = fs.into_iter().map(|(n, t)| (n, t, 0)).collect();
                } else {
                    let (out, size, align) = self.struct_layout(fs);
                    let sd = self.structs.get_mut(id);
                    sd.fields = out;
                    sd.size = size;
                    sd.align = align;
                }
            }
            AdtShape::Enum(vs) => {
                let mut discrs = vec![];
                let mut next = 0i64;
                for v in vs {
                    let d = v.discr.unwrap_or(next);
                    if discrs.contains(&d) {
                        return Err(format!("{line}:1: discriminant value `{d}` assigned more than once"));
                    }
                    discrs.push(d);
                    next = d + 1;
                }
                let tag = if discrs.iter().all(|&d| (0..=255).contains(&d)) { Ty::U8 } else { Ty::I32 };
                let mut rv = vec![];
                for (v, d) in vs.iter().zip(discrs) {
                    rv.push((v.name.clone(), v.kind, res(self, &v.fields)?, d));
                }
                if generic {
                    let sd = self.structs.get_mut(id);
                    sd.tag = tag;
                    sd.variants = rv
                        .into_iter()
                        .map(|(name, kind, fs, discr)| VariantDef {
                            name,
                            kind,
                            fields: fs.into_iter().map(|(n, t)| (n, t, 0)).collect(),
                            discr,
                        })
                        .collect();
                } else {
                    let (vs, size, align) = self.enum_layout(rv, tag);
                    let sd = self.structs.get_mut(id);
                    sd.tag = tag;
                    sd.variants = vs;
                    sd.size = size;
                    sd.align = align;
                }
            }
        }
        state[id] = 2;
        Ok(())
    }

    /// `E::V` / `Self::V` → (enum id, バリアント番号)
    /// `Some` / `None` / `Ok` / `Err` は (prelude の) Option / Result のバリアント。
    /// ジェネリックな enum なら、返る id はジェネリックな定義 (呼び出し側でインスタンスにする)
    pub fn variant_by_path(&self, p: &syn::Path, self_sid: Option<u32>) -> Option<(u32, usize)> {
        let segs: Vec<&syn::PathSegment> = p.segments.iter().collect();
        // `core::option::Option::Some` などは最後の 2 つで見る
        let segs = if segs.len() > 2 && matches!(segs[0].ident.to_string().as_str(), "core" | "crate" | "std") {
            segs[segs.len() - 2..].to_vec()
        } else {
            segs
        };
        if segs.len() == 1 {
            let n = segs[0].ident.to_string();
            let en = match n.as_str() {
                "Some" | "None" => "Option",
                "Ok" | "Err" => "Result",
                _ => return None,
            };
            let sid = *self.struct_index.get(en)?;
            let vi = self.structs[sid as usize].variants.iter().position(|v| v.name == n)?;
            return Some((sid, vi));
        }
        if segs.len() != 2 || !segs[1].arguments.is_none() {
            return None;
        }
        let tn = segs[0].ident.to_string();
        let sid = if tn == "Self" { self_sid? } else { *self.struct_index.get(&tn)? };
        let sd = &self.structs[sid as usize];
        if !sd.is_enum {
            return None;
        }
        let vn = p.segments[1].ident.to_string();
        sd.variants.iter().position(|v| v.name == vn).map(|vi| (sid, vi))
    }

    pub fn resolve(&self, t: &TySyn) -> R<Ty> {
        self.resolve_with(t, &|_| None)
    }

    /// 関数本体の中の型を解決する (`Self` と型パラメータは env から)
    pub fn parse_ty(&self, t: &syn::Type, env: &TyEnv) -> R<Ty> {
        let ts = parse_tysyn(t, Some(&TySyn::Named("Self".into())))?;
        self.resolve_env(&ts, env).or_else(|e| err(t.span(), e))
    }

    pub fn resolve_env(&self, t: &TySyn, env: &TyEnv) -> R<Ty> {
        self.resolve_with(t, &|n| {
            if n == "Self" {
                return env.self_sid.map(Ty::Struct);
            }
            env.params.iter().find(|p| p.0 == n).map(|p| p.1)
        })
    }

    /// 型名として書かれたパス (`S` / `Self`) → struct id
    pub fn struct_by_path(&self, p: &syn::Path, self_sid: Option<u32>) -> Option<u32> {
        let id = p.get_ident()?.to_string();
        if id == "Self" {
            return self_sid;
        }
        self.struct_index.get(&id).copied()
    }

    /// 関数として書かれたパス (`f` / `S::f` / `Self::f`) → 関数 ID
    pub fn fn_by_path(&self, p: &syn::Path, self_sid: Option<u32>) -> Option<u32> {
        // 最後のセグメントの turbofish (`f::<T>`) は呼び出し側で読む
        let n = p.segments.len();
        if p.leading_colon.is_some() || p.segments.iter().take(n.saturating_sub(1)).any(|s| !s.arguments.is_none()) {
            return None;
        }
        match p.segments.len() {
            1 => self.index.get(&p.segments[0].ident.to_string()).copied(),
            2 => {
                let ty = p.segments[0].ident.to_string();
                let ty = if ty == "Self" {
                    self.structs[self.base_of(self_sid?) as usize].name.clone()
                } else {
                    ty
                };
                self.index.get(&format!("{ty}::{}", p.segments[1].ident)).copied()
            }
            _ => None,
        }
    }

    /// struct `sid` のメソッド `name`
    pub fn method(&self, sid: u32, name: &str) -> Option<u32> {
        self.index.get(&format!("{}::{name}", self.structs[self.base_of(sid) as usize].name)).copied()
    }

    pub fn size_align(&self, t: Ty) -> (u32, u32) {
        match t {
            Ty::Struct(id) => {
                let s = &self.structs[id as usize];
                (s.size, s.align)
            }
            Ty::Array(e, n) => {
                let (s, a) = self.size_align(*e);
                (s * n, a)
            }
            t if t.is_fat() => (16, 8),
            Ty::Unit | Ty::Never | Ty::Slice(_) | Ty::Str | Ty::Param(_) => (0, 1),
            t => (t.bits() / 8, t.bits() / 8),
        }
    }

    pub fn field(&self, sid: u32, name: &str) -> Option<(Ty, u32)> {
        self.structs[sid as usize]
            .fields
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|&(_, t, o)| (t, o))
    }

    /// エラーメッセージ用の型名
    pub fn tn(&self, t: Ty) -> String {
        match t {
            Ty::Ref(false, i) => format!("&{}", self.tn(*i)),
            Ty::Ref(true, i) => format!("&mut {}", self.tn(*i)),
            Ty::Struct(id) => self.structs[id as usize].name.clone(),
            Ty::Unit => "()".into(),
            Ty::Never => "!".into(),
            Ty::Param(i) => format!("T{i}"),
            Ty::Ptr(false, i) => format!("*const {}", self.tn(*i)),
            Ty::Ptr(true, i) => format!("*mut {}", self.tn(*i)),
            Ty::Array(i, n) => format!("[{}; {n}]", self.tn(*i)),
            Ty::Slice(i) => format!("[{}]", self.tn(*i)),
            Ty::Str => "str".into(),
            t => format!("{t:?}").to_lowercase(),
        }
    }

    /// キャッシュキー用: 型を (struct は deep hash で) ハッシュする
    pub fn hash_ty(&self, t: Ty, h: &mut KeyHasher) {
        match t {
            Ty::Ref(m, i) => {
                h.add(&(100u8, m));
                self.hash_ty(*i, h);
            }
            Ty::Struct(id) => {
                let sd = &self.structs[id as usize];
                match sd.base {
                    // インスタンス: 元の定義の deep hash + 型引数
                    Some(b) => {
                        h.add(&(103u8, self.structs[b as usize].deep, sd.args.len()));
                        for &a in &sd.args {
                            self.hash_ty(a, h);
                        }
                    }
                    None => h.add(&(101u8, sd.deep)),
                }
            }
            Ty::Param(i) => h.add(&(102u8, i)),
            Ty::Ptr(m, i) => {
                h.add(&(104u8, m));
                self.hash_ty(*i, h);
            }
            Ty::Array(i, n) => {
                h.add(&(105u8, n));
                self.hash_ty(*i, h);
            }
            Ty::Slice(i) => {
                h.add(&106u8);
                self.hash_ty(*i, h);
            }
            Ty::Str => h.add(&107u8),
            t => h.add(&ty_code(t)),
        }
    }

    pub fn hash_sig(&self, id: u32, h: &mut KeyHasher) {
        let s = &self.sigs[id as usize];
        h.add(self.names[id as usize].as_str());
        h.add(&(s.symbol.as_str(), s.is_extern, s.is_main, s.has_self, s.params.len()));
        h.add(&(&s.generics, &s.trait_of, s.has_body, &s.impl_trait));
        if let Some(t) = s.self_ty {
            self.hash_ty(t, h);
        }
        for &p in &s.params {
            self.hash_ty(p, h);
        }
        self.hash_ty(s.ret, h);
    }

    /// アイテム内の識別子のうち関数名に一致するもの (ソート済み) と struct 名に一致するもの。
    /// ローカル変数による shadowing は無視した過大近似だが、
    /// キャッシュキーと到達可能性の両方にとって安全側である。
    pub fn refs(&self, src: &str, it: &SrcItem) -> (Vec<u32>, Vec<u32>) {
        let mut fns = vec![];
        let mut sts = vec![];
        for &(a, b) in &it.idents {
            let s = &src[a as usize..b as usize];
            if let Some(ids) = self.by_last.get(s) {
                fns.extend_from_slice(ids);
            }
            if let Some(&i) = self.struct_index.get(s) {
                sts.push(i);
            }
        }
        for v in [&mut fns, &mut sts] {
            v.sort_unstable();
            v.dedup();
        }
        (fns, sts)
    }

    /// main から到達可能な関数 ID。
    /// 到達しない関数は parse も型検査もコード生成もしない (relax/blaze の demand-driven の AOT 版)。
    pub fn reachable(&self, refs: &[Vec<u32>]) -> Vec<bool> {
        let mut seen = vec![false; self.names.len()];
        let mut stack = vec![self.index["main"]];
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i as usize], true) {
                continue;
            }
            stack.extend(refs[i as usize].iter().copied());
        }
        seen
    }
}

// --- Summary のシリアライズ ------------------------------------------------

const TYS: [Ty; 13] = [
    Ty::I8, Ty::I16, Ty::I32, Ty::I64, Ty::Isize, Ty::U8, Ty::U16, Ty::U32, Ty::U64, Ty::Usize,
    Ty::Bool, Ty::Unit, Ty::Never,
];

fn ty_code(t: Ty) -> u8 {
    TYS.iter().position(|&x| x == t).unwrap() as u8
}

impl Default for TySyn {
    fn default() -> Self {
        TySyn::Prim(Ty::Unit)
    }
}

impl Enc for TySyn {
    fn enc(&self, out: &mut Vec<u8>) {
        match self {
            TySyn::Prim(t) => out.push(TYS.iter().position(|x| x == t).unwrap() as u8),
            TySyn::Ref(m, i) => {
                out.push(20 + *m as u8);
                i.enc(out);
            }
            TySyn::Named(n) => {
                out.push(22);
                n.enc(out);
            }
            TySyn::App(n, a) => {
                out.push(23);
                n.enc(out);
                a.enc(out);
            }
            TySyn::Tuple(a) => {
                out.push(24);
                a.enc(out);
            }
            TySyn::Array(i, ArrLen::Lit(n)) => {
                out.push(25);
                i.enc(out);
                n.enc(out);
            }
            TySyn::Array(i, ArrLen::Const(c)) => {
                out.push(26);
                i.enc(out);
                c.enc(out);
            }
            TySyn::Slice(i) => {
                out.push(27);
                i.enc(out);
            }
            TySyn::Str => out.push(28),
            TySyn::Ptr(m, i) => {
                out.push(29 + *m as u8);
                i.enc(out);
            }
        }
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some(match r.u8()? {
            c @ 0..13 => TySyn::Prim(TYS[c as usize]),
            c @ (20 | 21) => TySyn::Ref(c == 21, Box::new(TySyn::dec(r)?)),
            22 => TySyn::Named(String::dec(r)?),
            23 => TySyn::App(String::dec(r)?, Vec::dec(r)?),
            24 => TySyn::Tuple(Vec::dec(r)?),
            25 => TySyn::Array(Box::new(TySyn::dec(r)?), ArrLen::Lit(u64::dec(r)?)),
            26 => TySyn::Array(Box::new(TySyn::dec(r)?), ArrLen::Const(String::dec(r)?)),
            27 => TySyn::Slice(Box::new(TySyn::dec(r)?)),
            28 => TySyn::Str,
            c @ (29 | 30) => TySyn::Ptr(c == 30, Box::new(TySyn::dec(r)?)),
            _ => return None,
        })
    }
}

impl Enc for Box<TySyn> {
    fn enc(&self, out: &mut Vec<u8>) {
        (**self).enc(out)
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some(Box::new(TySyn::dec(r)?))
    }
}

impl Enc for VariantKind {
    fn enc(&self, out: &mut Vec<u8>) {
        out.push(*self as u8)
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        [VariantKind::Unit, VariantKind::Tuple, VariantKind::Struct].get(r.u8()? as usize).copied()
    }
    fn is_empty_val(&self) -> bool {
        *self == VariantKind::Unit
    }
}

crate::enc_struct!(VariantSyn { name, kind, fields, discr });
crate::enc_struct!(SigSyn {
    params, ret, symbol, is_extern, is_main, impl_of, has_self, generics, trait_of, has_body, impl_trait, self_ty,
});
crate::enc_struct!(Summary {
    fns, structs, impl_self, enums, trait_decl, impl_trait, derives, adt_generics, impl_generics, impl_self_ty,
});

/// キャッシュにインスタンスの型引数を書くため
pub fn put_ty(out: &mut Vec<u8>, t: &TySyn) {
    t.enc(out)
}

/// d[*pos..] から TySyn を 1 つ読む
pub fn read_ty(d: &[u8], pos: &mut usize) -> Option<TySyn> {
    let mut r = Rd { d, pos: *pos };
    let t = TySyn::dec(&mut r);
    *pos = r.pos;
    t
}

impl Summary {
    pub fn encode(&self, out: &mut Vec<u8>) {
        self.enc(out)
    }

    pub fn decode(d: &[u8]) -> Option<Summary> {
        Summary::dec(&mut Rd::new(d))
    }
}

/// 判別値などの整数定数式 (リテラルと単項マイナスだけ)
fn const_int(e: &syn::Expr) -> R<i64> {
    match e {
        syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(i), .. }) => i.base10_parse::<i64>().map_err(|e| e.to_string()),
        syn::Expr::Unary(syn::ExprUnary { op: syn::UnOp::Neg(_), expr, .. }) => Ok(-const_int(expr)?),
        syn::Expr::Paren(p) => const_int(&p.expr),
        _ => err(e.span(), "only integer literals are supported as discriminants"),
    }
}

/// ライフタイム引数だけなら許す (消去する)。型引数・const 引数はまだ未対応。
fn only_lifetimes(g: &syn::Generics) -> R<()> {
    for p in &g.params {
        if !matches!(p, syn::GenericParam::Lifetime(_)) {
            return err(p.span(), "generics are not supported yet (lifetimes only)");
        }
    }
    Ok(())
}

/// trait 境界 (`T: A + B` と where 節) を集める
fn collect_generics(g: &syn::Generics, out: &mut Vec<(String, Vec<String>)>) -> R<()> {
    let bound_names = |bs: &syn::punctuated::Punctuated<syn::TypeParamBound, syn::Token![+]>| -> R<Vec<String>> {
        let mut v = vec![];
        for b in bs {
            match b {
                syn::TypeParamBound::Trait(t) => match t.path.get_ident() {
                    Some(id) => v.push(id.to_string()),
                    None => return err(t.span(), "unsupported trait bound"),
                },
                syn::TypeParamBound::Lifetime(_) => {}
                _ => return err(b.span(), "unsupported bound"),
            }
        }
        Ok(v)
    };
    for p in &g.params {
        match p {
            syn::GenericParam::Lifetime(_) => {}
            syn::GenericParam::Type(tp) => out.push((tp.ident.to_string(), bound_names(&tp.bounds)?)),
            syn::GenericParam::Const(c) => return err(c.span(), "const generics are not supported"),
        }
    }
    if let Some(w) = &g.where_clause {
        for pr in &w.predicates {
            let syn::WherePredicate::Type(pt) = pr else { continue };
            let syn::Type::Path(tp) = &pt.bounded_ty else {
                return err(pt.span(), "unsupported where clause");
            };
            let Some(id) = tp.path.get_ident() else {
                return err(pt.span(), "unsupported where clause");
            };
            let bs = bound_names(&pt.bounds)?;
            match out.iter_mut().find(|g| *id == g.0) {
                Some(g) => g.1.extend(bs),
                None => return err(pt.span(), "where clause on an unknown type parameter"),
            }
        }
    }
    Ok(())
}

/// owner: impl / trait のメンバならその所有者
fn parse_sig(sig: &syn::Signature, is_extern: bool, is_main: bool, owner: Option<&Owner>) -> R<SigSyn> {
    let mut generics = owner.map(|o| o.generics.clone()).unwrap_or_default();
    collect_generics(&sig.generics, &mut generics)?;
    if is_extern && !generics.is_empty() {
        return err(sig.generics.span(), "extern fns cannot be generic");
    }
    if is_main && !generics.is_empty() {
        return err(sig.generics.span(), "`main` cannot be generic");
    }
    if sig.variadic.is_some() {
        return err(sig.span(), "variadic functions are not supported");
    }
    let self_ty = owner.map(|o| &o.self_ty);
    let mut params = vec![];
    let mut has_self = false;
    for a in &sig.inputs {
        match a {
            syn::FnArg::Typed(pt) => params.push(parse_tysyn(&pt.ty, self_ty)?),
            // syn 2 では `self` / `&self` / `&mut self` / `self: &Self` すべて ty に完全な型が入る
            syn::FnArg::Receiver(r) => {
                if owner.is_none() {
                    return err(r.span(), "`self` is only allowed in impls");
                }
                has_self = true;
                params.push(parse_tysyn(&r.ty, self_ty)?);
            }
        }
    }
    let ret = match &sig.output {
        syn::ReturnType::Default => TySyn::Prim(Ty::Unit),
        syn::ReturnType::Type(_, t) => parse_tysyn(t, self_ty)?,
    };
    let name = sig.ident.to_string();
    let symbol = match owner {
        _ if is_extern || is_main => name,
        // ローカルシンボル。短く保つことで .o / シンボル表を小さくする
        Some(o) => format!("_x{}.{name}", o.base),
        None => format!("_x{name}"),
    };
    let is_trait = owner.is_some_and(|o| o.is_trait);
    Ok(SigSyn {
        params,
        ret,
        symbol,
        is_extern,
        is_main,
        impl_of: owner.filter(|o| !o.is_trait).map(|o| o.base.clone()),
        has_self,
        generics,
        trait_of: owner.filter(|o| o.is_trait).map(|o| o.base.clone()),
        has_body: true,
        impl_trait: owner.and_then(|o| o.impl_trait.clone()),
        self_ty: owner.filter(|_| !is_trait).map(|o| o.self_ty.clone()),
    })
}
