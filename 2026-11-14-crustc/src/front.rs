//! フロントエンド: アイテム要約 (シグネチャ・struct 定義) から型表と参照グラフを作る。
//!
//! ここは「関数本体を見ない」軽い処理だけにしている。本体の型検査・lowering は
//! codegen 側で関数ごとに行い、キャッシュヒットした関数は本体に一切触れない。

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use syn::spanned::Spanned;

use crate::cache::{Key, KeyHasher};
use crate::enc::{Enc, Rd};
use crate::fxhash::FxMap;
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
    /// 128 bit 整数
    I128,
    U128,
    /// Unicode のスカラー値 (32 bit)
    Char,
    /// 浮動小数点 (IEEE 754)
    F32,
    F64,
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
    /// `dyn Trait` (サイズ不定。`&dyn Trait` = (データ, vtable) の中身としてだけ現れる)。
    /// 中身は trait 境界の文字列 (`Shape` / `Fn(i32) -> i32`)
    Dyn(&'static String),
    /// 関数ポインタ `fn(A) -> R` (関数アイテムを値として使ったものもこれ)
    Fn(&'static FnTy),
    /// `Box<T>`: ヒープへの所有ポインタ (`Box<dyn Trait>` は fat)。drop で中身を drop して free する
    Box(&'static Ty),
    /// 関連型 `T::Item` (T が型パラメータのときだけ現れる。具体型になったら impl の定義に置き換わる)
    Assoc(&'static AssocTy),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AssocTy {
    pub base: Ty,
    pub name: String,
}

pub fn intern_assoc(a: AssocTy) -> &'static AssocTy {
    static TABLE: OnceLock<Mutex<HashSet<&'static AssocTy>>> = OnceLock::new();
    let mut tab = TABLE.get_or_init(Default::default).lock().unwrap();
    if let Some(&r) = tab.get(&a) {
        return r;
    }
    let r: &'static AssocTy = Box::leak(Box::new(a));
    tab.insert(r);
    r
}

impl Ty {
    /// メモリ上に置き、値としてはアドレスで扱う型 (struct / enum / 配列 / `&[T]`・`&str` の fat pointer)
    pub fn is_agg(self) -> bool {
        matches!(self, Ty::Struct(_) | Ty::Array(..)) || self.is_fat()
    }
    /// (ポインタ, 長さ) の 2 語で表す参照
    pub fn is_fat(self) -> bool {
        matches!(
            self,
            Ty::Ref(_, Ty::Slice(_) | Ty::Str | Ty::Dyn(_)) | Ty::Box(Ty::Slice(_) | Ty::Str | Ty::Dyn(_))
        )
    }
}

/// 文字列の intern 表 (`dyn` の trait 境界)
/// (`Ty` を 16 byte に保つため、fat な `&str` ではなく `&String` を持つ)
pub fn intern_str(s: &str) -> &'static String {
    static TABLE: OnceLock<Mutex<HashSet<&'static String>>> = OnceLock::new();
    let mut tab = TABLE.get_or_init(Default::default).lock().unwrap();
    if let Some(&r) = tab.get(&s.to_string()) {
        return r;
    }
    let r: &'static String = Box::leak(Box::new(s.to_string()));
    tab.insert(r);
    r
}

/// 関数ポインタ型 `fn(A, B) -> R` の中身
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FnTy {
    pub params: Vec<Ty>,
    pub ret: Ty,
}

pub fn intern_fn(f: FnTy) -> &'static FnTy {
    static TABLE: OnceLock<Mutex<HashSet<&'static FnTy>>> = OnceLock::new();
    let mut tab = TABLE.get_or_init(Default::default).lock().unwrap();
    if let Some(&r) = tab.get(&f) {
        return r;
    }
    let r: &'static FnTy = Box::leak(Box::new(f));
    tab.insert(r);
    r
}

/// trait 境界を文字列にする (パスの最後のセグメントだけ。`core::ops::Fn(u8) -> u8` → `Fn (u8) -> u8`)
pub fn bound_string(t: &syn::TraitBound) -> String {
    use quote::ToTokens;
    t.path
        .segments
        .last()
        .map(|s| s.to_token_stream().to_string())
        .unwrap_or_default()
}

/// `Fn(A, B) -> R` 形式の境界なら (種類, 引数, 戻り値)
pub fn parse_fn_bound(s: &str) -> Option<(&'static str, Vec<TySyn>, TySyn)> {
    let kind = ["FnOnce", "FnMut", "Fn"]
        .into_iter()
        .find(|k| s.starts_with(k) && s[k.len()..].trim_start().starts_with('('))?;
    let tb: syn::TraitBound = syn::parse_str(s).ok()?;
    let syn::PathArguments::Parenthesized(pa) = &tb.path.segments.last()?.arguments else {
        return None;
    };
    let args = pa
        .inputs
        .iter()
        .map(|t| parse_tysyn(t, None).ok())
        .collect::<Option<Vec<_>>>()?;
    let ret = match &pa.output {
        syn::ReturnType::Default => TySyn::Prim(Ty::Unit),
        syn::ReturnType::Type(_, t) => parse_tysyn(t, None).ok()?,
    };
    Some((kind, args, ret))
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
            "i128" => Ty::I128,
            "u128" => Ty::U128,
            _ => return None,
        })
    }

    pub fn prim(s: &str) -> Option<Ty> {
        Ty::from_suffix(s).or(match s {
            "bool" => Some(Ty::Bool),
            "char" => Some(Ty::Char),
            "f32" => Some(Ty::F32),
            "f64" => Some(Ty::F64),
            _ => None,
        })
    }

    pub fn is_float(self) -> bool {
        matches!(self, Ty::F32 | Ty::F64)
    }

    pub fn is_int(self) -> bool {
        matches!(
            self,
            Ty::I8
                | Ty::I16
                | Ty::I32
                | Ty::I64
                | Ty::Isize
                | Ty::U8
                | Ty::U16
                | Ty::U32
                | Ty::U64
                | Ty::Usize
                | Ty::I128
                | Ty::U128
        )
    }

    pub fn signed(self) -> bool {
        matches!(self, Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::Isize | Ty::I128)
    }

    /// スカラー (整数・bool・参照) のビット幅。struct / unit / never は 0。
    pub fn bits(self) -> u32 {
        match self {
            Ty::I8 | Ty::U8 | Ty::Bool => 8,
            Ty::I16 | Ty::U16 => 16,
            Ty::I32 | Ty::U32 | Ty::Char | Ty::F32 => 32,
            Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize | Ty::Ptr(..) | Ty::Fn(_) | Ty::F64 => 64,
            Ty::I128 | Ty::U128 => 128,
            Ty::Ref(..) | Ty::Box(_) if self.is_fat() => 0,
            Ty::Ref(..) | Ty::Box(_) => 64,
            Ty::Unit
            | Ty::Never
            | Ty::Struct(_)
            | Ty::Param(_)
            | Ty::Array(..)
            | Ty::Slice(_)
            | Ty::Str
            | Ty::Dyn(_)
            | Ty::Assoc(_) => 0,
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
    /// `dyn Trait` (境界の文字列)
    Dyn(String),
    /// `fn(A, B) -> R` (最後が戻り値)
    FnPtr(Vec<TySyn>),
    /// 関連型 `T::Item` / `Self::Item`
    Assoc(Box<TySyn>, String),
    /// 型推論に任せる `_` (`collect::<Vec<_>>()`)
    Infer,
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
        syn::Type::Tuple(tt) => TySyn::Tuple(
            tt.elems
                .iter()
                .map(|e| parse_tysyn(e, self_ty))
                .collect::<R<_>>()?,
        ),
        syn::Type::Paren(p) => parse_tysyn(&p.elem, self_ty)?,
        syn::Type::BareFn(bf) => {
            if bf.variadic.is_some() || bf.abi.is_some() {
                return err(bf.span(), "only Rust ABI fn pointers are supported");
            }
            let mut v = vec![];
            for a in &bf.inputs {
                v.push(parse_tysyn(&a.ty, self_ty)?);
            }
            v.push(match &bf.output {
                syn::ReturnType::Default => TySyn::Prim(Ty::Unit),
                syn::ReturnType::Type(_, t) => parse_tysyn(t, self_ty)?,
            });
            TySyn::FnPtr(v)
        }
        syn::Type::TraitObject(to) => {
            let mut tr = None;
            for b in &to.bounds {
                match b {
                    // (`dyn Error + Send + Sync`: 自動 trait は読み飛ばす)
                    syn::TypeParamBound::Trait(t)
                        if t.path.segments.last().is_some_and(|s| matches!(s.ident.to_string().as_str(), "Send" | "Sync")) => {}
                    syn::TypeParamBound::Trait(t) if tr.is_none() => tr = Some(bound_string(t)),
                    syn::TypeParamBound::Lifetime(_) => {}
                    _ => return err(b.span(), "only one trait is supported in `dyn`"),
                }
            }
            match tr {
                Some(s) => TySyn::Dyn(s),
                None => return err(to.span(), "`dyn` needs a trait"),
            }
        }
        syn::Type::Group(g) => parse_tysyn(&g.elem, self_ty)?,
        // ライフタイムはコード生成に影響しないので消去する
        syn::Type::Reference(r) => TySyn::Ref(
            r.mutability.is_some(),
            Box::new(parse_tysyn(&r.elem, self_ty)?),
        ),
        syn::Type::Ptr(p) => TySyn::Ptr(
            p.mutability.is_some(),
            Box::new(parse_tysyn(&p.elem, self_ty)?),
        ),
        syn::Type::Slice(sl) => TySyn::Slice(Box::new(parse_tysyn(&sl.elem, self_ty)?)),
        syn::Type::Array(a) => {
            let len = match &a.len {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Int(i),
                    ..
                }) => ArrLen::Lit(i.base10_parse().map_err(|e| e.to_string())?),
                syn::Expr::Path(p) if p.path.get_ident().is_some() => {
                    ArrLen::Const(p.path.get_ident().unwrap().to_string())
                }
                // 関連定数 `[u8; Buf::CAP]` / `[u8; Self::CAP]`
                syn::Expr::Path(p) if p.qself.is_none() && p.path.segments.len() == 2 => {
                    let a = p.path.segments[0].ident.to_string();
                    let a = match (a.as_str(), self_ty) {
                        ("Self", Some(TySyn::Named(n))) => n.clone(),
                        _ => a,
                    };
                    ArrLen::Const(format!("{a}::{}", p.path.segments[1].ident))
                }
                e => return err(e.span(), "array length must be a literal or a const"),
            };
            TySyn::Array(Box::new(parse_tysyn(&a.elem, self_ty)?), len)
        }
        syn::Type::Infer(_) => TySyn::Infer,
        // `Self::Item` / `I::Item` (大文字で始まる 2 セグメントのパス)
        syn::Type::Path(p)
            if p.qself.is_none()
                && p.path.segments.len() == 2
                && p.path.segments[0].arguments.is_none()
                && p.path.segments[1].arguments.is_none()
                && p.path.segments[0]
                    .ident
                    .to_string()
                    .starts_with(|c: char| c.is_ascii_uppercase()) =>
        {
            let b = p.path.segments[0].ident.to_string();
            let base = if b == "Self" {
                self_ty.cloned().unwrap_or(TySyn::Named("Self".into()))
            } else {
                TySyn::Named(b)
            };
            TySyn::Assoc(Box::new(base), p.path.segments[1].ident.to_string())
        }
        // `<T as Iterator>::Item`
        syn::Type::Path(p) if p.qself.is_some() && p.path.segments.len() >= 2 => {
            let q = p.qself.as_ref().unwrap();
            TySyn::Assoc(
                Box::new(parse_tysyn(&q.ty, self_ty)?),
                p.path.segments.last().unwrap().ident.to_string(),
            )
        }
        // `io::Error` / `io::Result<T>` (prelude の IoError)
        syn::Type::Path(p)
            if p.qself.is_none()
                && p.path.segments.len() >= 2
                && p.path.segments[p.path.segments.len() - 2].ident == "io"
                && matches!(p.path.segments.last().unwrap().ident.to_string().as_str(), "Result" | "Error") =>
        {
            let last = p.path.segments.last().unwrap();
            let io_err = TySyn::Named("IoError".into());
            if last.ident == "Error" {
                io_err
            } else {
                let syn::PathArguments::AngleBracketed(ab) = &last.arguments else {
                    return err(t.span(), "`io::Result` needs a type argument");
                };
                let Some(syn::GenericArgument::Type(a)) = ab.args.first() else {
                    return err(t.span(), "`io::Result` needs a type argument");
                };
                TySyn::App("Result".into(), vec![parse_tysyn(a, self_ty)?, io_err])
            }
        }
        // `fmt::Result` / `fmt::Error` (`std::fmt::Result` も)
        syn::Type::Path(p)
            if p.qself.is_none()
                && p.path.segments.len() >= 2
                && p.path.segments[p.path.segments.len() - 2].ident == "fmt"
                && matches!(p.path.segments.last().unwrap().ident.to_string().as_str(), "Result" | "Error") =>
        {
            let err = TySyn::Named("__FmtError".into());
            if p.path.segments.last().unwrap().ident == "Error" {
                err
            } else {
                TySyn::App("Result".into(), vec![TySyn::Prim(Ty::Unit), err])
            }
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
                            // `Buf<4>`: const ジェネリクスの引数 (長さを持つ配列型 `[(); 4]` として持つ)
                            syn::GenericArgument::Const(e) => v.push(const_arg_syn(e)?),
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
    /// `#[no_mangle]` / `#[unsafe(no_mangle)]`: 名前そのままで外に見せる
    pub export: bool,
    /// `#[panic_handler]`
    pub panic_handler: bool,
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
    pub export: bool,
    pub panic_handler: bool,
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
    /// `Self` の型 (struct なら Struct(self_sid)、プリミティブへの trait 実装なら u32 など)
    pub self_ty: Option<Ty>,
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

thread_local! {
    /// 作っている途中のインスタンス (基の型, 型引数) と、途中で取った番号
    static INSTANTIATING: std::cell::RefCell<Vec<((u32, Vec<Ty>), Option<u32>)>> = const { std::cell::RefCell::new(Vec::new()) };
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
        AdtTable {
            v: Mutex::new(vec![]),
            inst: Mutex::new(HashMap::new()),
            tuples: Mutex::new(HashMap::new()),
        }
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
    /// 先に番号だけ取ったインスタンスの中身を入れる (まだ誰にも公開していない番号だけに使う)
    fn replace(&self, i: u32, d: StructDef) {
        let mut g = self.v.lock().unwrap();
        *g[i as usize] = d;
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
    /// `const N: T = e;` の (名前, 型, 式のソース)。使う側で式として埋め込む
    pub consts: Vec<(String, TySyn, String)>,
    /// `static [mut] N: T = e;` の (名前, 型, 初期値のソース, mut)
    pub statics: Vec<(String, TySyn, String, bool)>,
    /// fn 本体の中のクロージャ (前順)。[`crate::closure`] を参照
    pub closures: Vec<ClosureSyn>,
    /// impl の関連型 (`type Item = u32;`)
    pub assocs: Vec<AssocSyn>,
    /// `type Key = u128;` / `type R<T> = Result<T, E>;` の (名前, 型パラメータ, 中身)
    pub aliases: Vec<(String, Vec<String>, TySyn)>,
}

#[derive(Clone, Debug, Default)]
pub struct AssocSyn {
    pub name: String,
    pub self_ty: TySyn,
    pub generics: Vec<String>,
    pub value: TySyn,
}

/// 持ち上げたクロージャ 1 個分 (struct と fn)
#[derive(Clone, Debug, Default)]
pub struct ClosureSyn {
    /// struct の名前 (`親::{closure#k}`)。fn は `<name>::call`
    pub name: String,
    /// struct と fn の型パラメータ: 親の型パラメータ, __C.., __P.., __R
    pub generics: Vec<String>,
    /// キャプチャ (名前, `__Ci`)
    pub fields: Vec<(String, TySyn)>,
    pub sig: SigSyn,
    pub by_move: bool,
    /// 戻り値型の注釈 (`|x| -> u8 { .. }`)
    pub ret_ann: Option<TySyn>,
    /// 親の型パラメータの数
    pub n_parent: u64,
}

/// const / static の定義
pub struct ConstDef {
    pub name: String,
    pub ty: Ty,
    /// 初期値の式のソース (使う側・初期化で parse し直す)
    pub expr: String,
    /// 定義しているアイテム (キャッシュキー用)
    pub item: u32,
    /// static なら Some(mut か)
    pub stat: Option<bool>,
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

/// `u32::MAX` などの関連定数
pub fn prim_assoc_const(t: Ty, name: &str) -> Option<i128> {
    let bits = t.bits();
    Some(match (name, t.signed()) {
        ("BITS", _) => bits as i128,
        // (u128::MAX はビット列として持つ: -1)
        ("MAX", true) => (u128::MAX >> (129 - bits)) as i128,
        ("MIN", true) => -((u128::MAX >> (129 - bits)) as i128) - 1,
        ("MAX", false) if t == Ty::Char => 0x10FFFF,
        ("MAX", false) => (u128::MAX >> (128 - bits)) as i128,
        ("MIN", false) => 0,
        _ => return None,
    })
}

/// syn の式をソースに戻す (const の式を要約に保存するため)
mod quote_like {
    pub trait ToSrc {
        fn to_src(&self) -> String;
    }
    impl ToSrc for syn::Expr {
        fn to_src(&self) -> String {
            use quote::ToTokens;
            self.to_token_stream().to_string()
        }
    }
}

/// `#[name]` か `#[unsafe(name)]` が付いているか
fn has_attr(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs.iter().any(|a| {
        if a.path().is_ident(name) {
            return true;
        }
        if a.path().is_ident("unsafe") {
            let mut found = false;
            let _ = a.parse_nested_meta(|m| {
                found |= m.path.is_ident(name);
                Ok(())
            });
            return found;
        }
        false
    })
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
    pub index: FxMap<String, u32>,
    pub sigs: Vec<FnSig>,
    /// ユーザ定義関数を定義しているアイテム番号 (extern は None)
    pub item_of: Vec<Option<u32>>,
    pub structs: AdtTable,
    pub struct_index: FxMap<String, u32>,
    /// バリアント名 → (enum, 番号)。`use E::*;` で取り込んだバリアントを短い名前で引く (同名が複数なら引かない)
    pub variant_names: FxMap<String, Option<(u32, usize)>>,
    /// 戻り値が本体から決まる `impl Trait` の関数
    pub opaque: Vec<u32>,
    /// これ以降のアイテムは prelude (オーバーフロー検査をしない)
    pub prelude_from: u32,
    /// 算術のオーバーフロー検査をするか (-O でなければ)
    pub overflow_checks: bool,
    /// -O (最適化ビルド)
    pub opt: bool,
    /// ブランケット impl のメソッド (名前 → 関数 id) と、その self 型の型パラメータ名
    pub blanket: FxMap<String, u32>,
    pub blanket_selfs: std::collections::HashSet<String>,
    /// 名前の最後のセグメント → 関数 ID 群 (`len` → `A::len`, `B::len`)。参照の過大近似用
    pub by_last: FxMap<String, Vec<u32>>,
    /// trait 名 → その trait で宣言された関数
    pub traits: FxMap<String, Vec<u32>>,
    /// (型, trait) の実装
    pub impls: HashSet<(u32, String)>,
    /// `#[panic_handler]` の関数 (no_std)。panic (範囲外アクセス・unwrap など) はここを呼ぶ
    pub panic_handler: Option<u32>,
    /// const と static
    pub consts: Vec<ConstDef>,
    pub const_index: FxMap<String, u32>,
    /// 関連定数 (`S::N`) を最後の名前 (`N`) で引く (参照関係の過大近似用)
    pub const_by_last: FxMap<String, Vec<u32>>,
    /// プリミティブ型への trait 実装 (型, trait)
    pub prim_impls: HashSet<(Ty, String)>,
    /// クロージャの fn id → 定義
    pub closures: FxMap<u32, ClosureDef>,
    /// (親の関数 id, 前順の番号) → クロージャの fn id
    pub closure_by: FxMap<(u32, u32), u32>,
    /// クロージャの (ジェネリックな) struct id → fn id
    pub closure_of_struct: FxMap<u32, u32>,
    /// `impl Drop for S` のある struct / enum (ジェネリックなら元の定義)
    pub drop_impls: crate::fxhash::FxSet<u32>,
    /// drop が要る型がありうるか (prelude に drop glue があるか。Box も Drop もなければ false)
    pub has_drop: bool,
    /// impl の関連型の定義
    pub assocs: Vec<AssocDef>,
    /// 右辺の型を持つ比較の impl の eq (`Vec::<PartialEq<[T;N]>>::eq` など)。`a == b` の型が違うとき探す
    pub peq: Vec<u32>,
    /// 型の別名: 名前 → (型パラメータ, 中身)
    pub aliases: FxMap<String, (Vec<String>, TySyn)>,
    /// 同じく `S::<PartialOrd<R>>::partial_cmp`。`a < b` の型が違うとき探す
    pub pord: Vec<u32>,
}

/// `impl<..> Trait for S<..> { type Name = Value; }`
pub struct AssocDef {
    pub name: String,
    /// impl の self 型と値 (impl の型パラメータを Param で含む)
    pub self_ty: Ty,
    pub value: Ty,
    pub ngen: usize,
}

/// 型推論に任せる `_` の印 (型パラメータの番号としては使わない値)
pub const HOLE: u8 = 250;

/// 持ち上げたクロージャ ([`crate::closure`])
pub struct ClosureDef {
    /// 親 (クロージャを書いた関数) の id と、その本体での前順の番号
    pub parent: u32,
    pub k: u32,
    /// ジェネリックな struct
    pub sid: u32,
    /// キャプチャの名前 (struct のフィールド順)
    pub captures: Vec<String>,
    pub by_move: bool,
    /// 親の型パラメータの数 (型引数の先頭)
    pub n_parent: usize,
    /// 戻り値の注釈 (親の型パラメータを Param で含みうる)
    pub ret_ann: Option<Ty>,
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
        Insts {
            n_fns: prog.names.len() as u32,
            list: vec![],
            map: HashMap::new(),
            pending: vec![],
        }
    }

    pub fn get(&self, id: u32) -> Option<&Inst> {
        id.checked_sub(self.n_fns)
            .and_then(|i| self.list.get(i as usize))
    }

    /// インスタンスを要求する (なければ作り、pending に積む)。トレイト境界もここで確かめる
    pub fn request(&mut self, prog: &Program, base: u32, args: Vec<Ty>) -> R<u32> {
        if let Some(&id) = self.map.get(&(base, args.clone())) {
            return Ok(id);
        }
        let b = &prog.sigs[base as usize];
        if b.generics.len() != args.len() {
            return Err(format!(
                "`{}` takes {} type arguments",
                prog.names[base as usize],
                b.generics.len()
            ));
        }
        for ((pn, bounds), &t) in b.generics.iter().zip(&args) {
            for tr in bounds {
                if !prog.implements(t, tr) {
                    return Err(format!(
                        "the trait bound `{}: {tr}` is not satisfied (for `{pn}`)",
                        prog.tn(t)
                    ));
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
            // Self: trait のデフォルトメソッドなら型引数の先頭、impl のメソッドなら self 型に型引数を入れたもの
            self_ty: if b.trait_of.is_some() {
                Some(args[0])
            } else {
                b.self_ty.map(|t| prog.subst(t, &args))
            },
            export: false,
            panic_handler: false,
        };
        let id = self.n_fns + self.list.len() as u32;
        self.list.push(Inst {
            base,
            args: args.clone(),
            sig,
        });
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
                    params: b
                        .generics
                        .iter()
                        .map(|g| g.0.clone())
                        .zip(i.args.iter().copied())
                        .collect(),
                    self_ty: i.sig.self_ty,
                }
            }
            None => {
                let s = &prog.sigs[id as usize];
                TyEnv {
                    self_sid: s.impl_of,
                    params: vec![],
                    self_ty: s.self_ty.filter(|t| !prog.has_param(*t)),
                }
            }
        }
    }
}

/// 関数 body の AST。クロージャなら親の本体から k 番目を取り出して fn にする
pub fn fn_ast(prog: &Program, f: syn::ItemFn, body: u32) -> syn::ItemFn {
    match prog.closures.get(&body) {
        Some(cd) => crate::closure::synth(&crate::closure::closures_of(&f.block)[cd.k as usize]),
        None => f,
    }
}

/// syn で 1 アイテムを parse する。エラー位置はファイル全体での行に補正する。
/// impl のヘッダは空の impl として、メンバは `Item::Fn` に直して返す。
pub fn parse_item(src: &str, it: &SrcItem) -> R<syn::Item> {
    let mut item = parse_item_raw(src, it)?;
    if let syn::Item::Fn(f) = &mut item {
        desugar_param_patterns(f);
    }
    Ok(item)
}

/// 引数のパターン (`fn f((i, j): (usize, usize))`) は名前の引数にして、本体の先頭で `let (i, j) = 引数;` にする
fn desugar_param_patterns(f: &mut syn::ItemFn) {
    let mut lets: Vec<syn::Stmt> = vec![];
    for (k, a) in f.sig.inputs.iter_mut().enumerate() {
        let syn::FnArg::Typed(pt) = a else { continue };
        if matches!(&*pt.pat, syn::Pat::Ident(_) | syn::Pat::Wild(_)) {
            continue;
        }
        let name = syn::Ident::new(&format!("__exr_arg{k}"), proc_macro2::Span::call_site());
        let pat = std::mem::replace(&mut *pt.pat, syn::parse_quote!(#name));
        lets.push(syn::parse_quote!(let #pat = #name;));
    }
    if !lets.is_empty() {
        lets.append(&mut f.block.stmts);
        f.block.stmts = lets;
    }
}

fn parse_item_raw(src: &str, it: &SrcItem) -> R<syn::Item> {
    let text = &src[it.start..it.end];
    let fix = |e: syn::Error| {
        let s = e.span().start();
        format!("{}:{}: {e}", s.line + it.line - 1, s.column + 1)
    };
    match it.kind {
        ItemKind::Normal => syn::parse_str::<syn::Item>(text).map_err(fix),
        // クレート属性 (`#![no_std]` など) は意味を持たない
        ItemKind::InnerAttr => Ok(syn::Item::Verbatim(Default::default())),
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
                None => err(f.sig.span(), "trait method has no default body")
                    .map_err(|e| relocate_err(e, it)),
            }
        }
        ItemKind::ImplMember(_) => match syn::parse_str::<syn::ImplItem>(text).map_err(fix)? {
            syn::ImplItem::Fn(f) => Ok(syn::Item::Fn(syn::ItemFn {
                attrs: f.attrs,
                vis: f.vis,
                sig: f.sig,
                block: Box::new(f.block),
            })),
            syn::ImplItem::Type(t) => Ok(syn::Item::Type(syn::ItemType {
                attrs: t.attrs,
                vis: t.vis,
                type_token: t.type_token,
                ident: t.ident,
                generics: t.generics,
                eq_token: t.eq_token,
                ty: Box::new(t.ty),
                semi_token: t.semi_token,
            })),
            // 関連定数 `const N: usize = 4;` (要約で `S::N` という名前の const にする)
            syn::ImplItem::Const(c) => Ok(syn::Item::Const(syn::ItemConst {
                attrs: c.attrs,
                vis: c.vis,
                const_token: c.const_token,
                ident: c.ident,
                generics: c.generics,
                colon_token: c.colon_token,
                ty: Box::new(c.ty),
                eq_token: c.eq_token,
                expr: Box::new(c.expr),
                semi_token: c.semi_token,
            })),
            other => err(other.span(), "only fns, consts and associated types are supported in impl blocks")
                .map_err(|e| relocate_err(e, it)),
        },
    }
}

pub fn parse_trait_member(src: &str, it: &SrcItem) -> R<syn::TraitItemFn> {
    let text = &src[it.start..it.end];
    match syn::parse_str::<syn::TraitItem>(text) {
        Ok(syn::TraitItem::Fn(f)) => Ok(f),
        Ok(other) => {
            err(other.span(), "only fns are supported in traits").map_err(|e| relocate_err(e, it))
        }
        Err(e) => {
            let s = e.span().start();
            Err(format!("{}:{}: {e}", s.line + it.line - 1, s.column + 1))
        }
    }
}

/// アイテムを parse して要約する。owner は impl / trait のメンバのとき、その self 型 / trait 名。
/// codegen でも使う AST (本体のある関数) があれば一緒に返す
pub fn parse_and_summarize(
    src: &str,
    it: &SrcItem,
    owner: Option<&Owner>,
) -> R<(Summary, Option<syn::Item>)> {
    if let ItemKind::TraitMember(_) = it.kind {
        let ow = owner.ok_or("trait member without trait")?;
        // `type Item;` (関連型の宣言) は何も持たない
        match syn::parse_str::<syn::TraitItem>(&src[it.start..it.end]) {
            Ok(syn::TraitItem::Type(_)) => return Ok((Summary::default(), None)),
            // `const K: i32;` (既定値があれば `Trait::K` という const にする: impl が持たないときに使う)
            Ok(syn::TraitItem::Const(c)) => {
                let mut sm = Summary::default();
                if let Some((_, e)) = &c.default {
                    use quote_like::ToSrc;
                    sm.consts.push((format!("{}::{}", ow.base, c.ident), parse_tysyn(&c.ty, None)?, e.to_src()));
                }
                return Ok((sm, None));
            }
            _ => {}
        }
        let f = parse_trait_member(src, it)?;
        let mut sig = parse_sig(&f.sig, false, false, Some(ow)).map_err(|e| relocate_err(e, it))?;
        sig.has_body = f.default.is_some();
        let sm = Summary {
            fns: vec![(format!("{}::{}", ow.base, f.sig.ident), sig)],
            ..Default::default()
        };
        let item = f.default.map(|block| {
            syn::Item::Fn(syn::ItemFn {
                attrs: f.attrs,
                vis: syn::Visibility::Inherited,
                sig: f.sig,
                block: Box::new(block),
            })
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
        syn::Item::Type(t) if owner.is_some() => {
            let ow = owner.unwrap();
            sm.assocs.push(AssocSyn {
                name: t.ident.to_string(),
                self_ty: ow.self_ty.clone(),
                generics: ow.generics.iter().map(|g| g.0.clone()).collect(),
                value: parse_tysyn(&t.ty, Some(&ow.self_ty))?,
            });
        }
        syn::Item::Fn(f) if owner.is_some() => {
            let ow = owner.unwrap();
            let mut sig = parse_sig(&f.sig, false, false, Some(ow))?;
            // Display と Debug はどちらも fmt なので、trait 名で区別する (`P::<Display>::fmt`)
            let name = match ow.impl_trait.as_deref() {
                Some(t @ ("Display" | "Debug")) => format!("{}::<{t}>::{}", ow.base, f.sig.ident),
                Some(t) if t.starts_with("PartialEq<") || t.starts_with("PartialOrd<") => {
                    format!("{}::<{t}>::{}", ow.base, f.sig.ident)
                }
                // (参照への impl のメソッドは名前を分ける: `impl Add for &M` の add は `M::__ref_add`)
                _ if matches!(ow.self_ty, TySyn::Ref(..)) => format!("{}::__ref_{}", ow.base, f.sig.ident),
                _ => format!("{}::{}", ow.base, f.sig.ident),
            };
            lift_closures(f, &name, &mut sig, Some(&ow.self_ty), &mut sm)?;
            sm.fns.push((name, sig));
        }
        // 型の別名 (`type Key = u128;`)
        syn::Item::Type(t) => {
            let mut g = vec![];
            collect_generics(&t.generics, &mut g)?;
            sm.aliases.push((t.ident.to_string(), g.into_iter().map(|x| x.0).collect(), parse_tysyn(&t.ty, None)?));
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
                // `fmt::Display` などは最後のセグメント
                let Some(last) = tr.segments.last() else {
                    return err(tr.span(), "unsupported trait path");
                };
                let id = &last.ident;
                sm.impl_trait = Some(id.to_string());
                // `impl PartialEq<[U; N]> for Vec<T>`: 右辺の型が self 型と違う比較は trait 名に型引数を残す
                // (`PartialEq<[U;N]>`。メソッドは `Vec::<PartialEq<[U;N]>>::eq` になり、`==` が右辺の型で選ぶ)
                // (`impl PartialOrd<Feet> for Meters` も同じ: `Meters::<PartialOrd<Feet>>::partial_cmp`)
                if id == "PartialEq" || id == "PartialOrd" {
                    if let syn::PathArguments::AngleBracketed(ab) = &last.arguments {
                        use quote::ToTokens;
                        let rhs: String = ab.to_token_stream().to_string().chars().filter(|c| !c.is_whitespace()).collect();
                        let selfs: String = im.self_ty.to_token_stream().to_string().chars().filter(|c| !c.is_whitespace()).collect();
                        if rhs != format!("<{selfs}>") && rhs != "<Self>" {
                            sm.impl_trait = Some(format!("{id}{rhs}"));
                        }
                    }
                }
            }
            collect_generics(&im.generics, &mut sm.impl_generics)?;
            // `impl Add for &M`: M のメソッド `__ref_add` (self は &M) として持つ
            let self_path = match &*im.self_ty {
                syn::Type::Reference(r) if im.trait_.is_some() => &*r.elem,
                t => t,
            };
            // `impl<A, B> Tr for (A, B)`: タプルの struct (`#tuple2`) のメソッド
            if let (syn::Type::Tuple(tt), true) = (self_path, im.trait_.is_some()) {
                sm.impl_self = Some(format!("#tuple{}", tt.elems.len()));
                sm.impl_self_ty = Some(parse_tysyn(&im.self_ty, None)?);
                return Ok(sm);
            }
            // `impl .. for [T; N]` / `for [T]`: 名前は `__array` / `__slice` (比較の impl だけが使う)
            if matches!(self_path, syn::Type::Array(_) | syn::Type::Slice(_)) && im.trait_.is_some() {
                sm.impl_self = Some(if matches!(self_path, syn::Type::Array(_)) { "__array" } else { "__slice" }.to_string());
                sm.impl_self_ty = Some(parse_tysyn(&im.self_ty, None)?);
                return Ok(sm);
            }
            let syn::Type::Path(tp) = self_path else {
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
            let export = has_attr(&f.attrs, "no_mangle");
            // `#[no_mangle] extern "C" fn main() -> i32` (no_main) は C の main そのもの
            let is_main = name == "main" && !export;
            let mut sig = parse_sig(&f.sig, false, is_main, None)?;
            // (`fn main() -> Result<(), E>` も: Err なら `Error: {:?}` を出して 1 で終わる)
            let result_ret = matches!(&sig.ret, TySyn::App(n, _) if n == "Result");
            if is_main && (!sig.params.is_empty() || (sig.ret != TySyn::Prim(Ty::Unit) && !result_ret)) {
                return err(f.sig.span(), "`main` must be `fn main()`");
            }
            if export {
                sig.export = true;
                sig.symbol = name.clone();
            }
            if has_attr(&f.attrs, "panic_handler") {
                sig.panic_handler = true;
                if sig.ret != TySyn::Prim(Ty::Never) {
                    return err(f.sig.span(), "`#[panic_handler]` function must return `!`");
                }
            }
            lift_closures(f, &name, &mut sig, None, &mut sm)?;
            sm.fns.push((name, sig));
        }
        // `use` は名前の最後のセグメントで解決するので読み飛ばす。クレート属性も同じ
        syn::Item::Use(_) | syn::Item::Verbatim(_) => {}
        syn::Item::Const(c) => {
            use quote_like::ToSrc;
            // (impl の中なら `S::N`)
            let name = match owner {
                Some(ow) => format!("{}::{}", ow.base, c.ident),
                None => c.ident.to_string(),
            };
            sm.consts.push((name, parse_tysyn(&c.ty, owner.map(|o| &o.self_ty))?, c.expr.to_src()));
        }
        syn::Item::Static(st) => {
            use quote_like::ToSrc;
            let m = matches!(st.mutability, syn::StaticMutability::Mut(_));
            sm.statics.push((
                st.ident.to_string(),
                parse_tysyn(&st.ty, None)?,
                st.expr.to_src(),
                m,
            ));
        }
        syn::Item::ForeignMod(m) => {
            for fi in &m.items {
                let syn::ForeignItem::Fn(f) = fi else {
                    return err(fi.span(), "only fns are supported in extern blocks");
                };
                let mut sig = parse_sig(&f.sig, true, false, None)?;
                // `#[link_name = "malloc"]`: シンボル名だけ変える
                for a in &f.attrs {
                    if let syn::Meta::NameValue(nv) = &a.meta {
                        if nv.path.is_ident("link_name") {
                            if let syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Str(l), ..
                            }) = &nv.value
                            {
                                sig.symbol = l.value();
                            }
                        }
                    }
                }
                sm.fns.push((f.sig.ident.to_string(), sig));
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
                            fs.push((
                                f.ident.as_ref().unwrap().to_string(),
                                parse_tysyn(&f.ty, sn_ty)?,
                            ));
                        }
                        (VariantKind::Struct, fs)
                    }
                };
                let discr = match &v.discriminant {
                    None => None,
                    Some((_, ex)) => Some(const_int(ex)?),
                };
                vs.push(VariantSyn {
                    name: v.ident.to_string(),
                    kind,
                    fields,
                    discr,
                });
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

/// fn 本体のクロージャを持ち上げる ([`crate::closure`])。戻り値位置の `impl Fn` も
/// ここで末尾のクロージャの型に置き換える
fn lift_closures(
    f: &syn::ItemFn,
    fname: &str,
    sig: &mut SigSyn,
    self_ty: Option<&TySyn>,
    sm: &mut Summary,
) -> R<()> {
    use crate::closure;
    let cs = closure::closures_of(&f.block);
    if cs.is_empty() {
        if sig.ret == TySyn::Named(IMPL_RET.into()) {
            sig.ret = TySyn::Named(OPAQUE_RET.into());
        }
        return Ok(());
    }
    let bound = closure::bound_in_fn(f);
    let mut out = vec![];
    for (k, c) in cs.iter().enumerate() {
        let name = format!("{fname}::{{closure#{k}}}");
        let mut generics: Vec<String> = sig.generics.iter().map(|g| g.0.clone()).collect();
        let n_parent = generics.len() as u64;
        let caps = closure::captures(c, &bound);
        let mut fields = vec![];
        for (i, n) in caps.iter().enumerate() {
            let g = format!("__C{i}");
            fields.push((n.clone(), TySyn::Named(g.clone())));
            generics.push(g);
        }
        let mut params = vec![];
        for (i, p) in c.inputs.iter().enumerate() {
            match closure::param(p).1 {
                Some(t) => params.push(parse_tysyn(t, self_ty)?),
                None => {
                    let g = format!("__P{i}");
                    params.push(TySyn::Named(g.clone()));
                    generics.push(g);
                }
            }
        }
        generics.push("__R".into());
        let ret_ann = match &c.output {
            syn::ReturnType::Default => None,
            syn::ReturnType::Type(_, t) => Some(parse_tysyn(t, self_ty)?),
        };
        let env = TySyn::App(
            name.clone(),
            generics.iter().map(|g| TySyn::Named(g.clone())).collect(),
        );
        params.insert(0, TySyn::Ptr(true, Box::new(env)));
        let mut gs: Vec<(String, Vec<String>)> = sig.generics.clone();
        gs.extend(generics[n_parent as usize..].iter().map(|g| (g.clone(), vec![])));
        let csig = SigSyn {
            params,
            ret: TySyn::Named("__R".into()),
            symbol: format!("{}.c{k}", sig.symbol),
            impl_of: sig.impl_of.clone(),
            generics: gs,
            has_body: true,
            self_ty: sig.self_ty.clone(),
            ..Default::default()
        };
        out.push(ClosureSyn {
            name,
            generics,
            fields,
            sig: csig,
            // (`move` がなくても、本体がキャプチャを値として渡す・返すなら値で持つ: rustc と同じく FnOnce)
            by_move: c.capture.is_some() || closure::moves_capture(c, &caps),
            ret_ann,
            n_parent,
        });
    }
    // `-> impl Fn(A) -> B { move |x| .. }`: 型は末尾のクロージャの struct。
    // キャプチャは引数だけ (型が注釈から分かる) に限る
    if sig.ret == TySyn::Named(IMPL_RET.into()) {
        // (末尾がクロージャでなければ、本体の型推論で決める: `-> impl Iterator<Item = u32>`)
        let tail = match f.block.stmts.last() {
            Some(syn::Stmt::Expr(e, None)) => Some(e),
            _ => None,
        };
        let mut tail = tail;
        while let Some(syn::Expr::Paren(p)) = tail {
            tail = Some(&p.expr);
        }
        let Some(syn::Expr::Closure(tc)) = tail else {
            sig.ret = TySyn::Named(OPAQUE_RET.into());
            sm.closures = out;
            return Ok(());
        };
        let key = closure::closure_key(tc);
        let k = cs.iter().position(|c| closure::closure_key(c) == key).unwrap();
        let syn::ReturnType::Type(_, rt) = &f.sig.output else {
            unreachable!()
        };
        let syn::Type::ImplTrait(it) = &**rt else {
            unreachable!()
        };
        let bs = bound_names(&it.bounds)?;
        let Some((_, bargs, bret)) = bs.iter().find_map(|b| parse_fn_bound(b)) else {
            return err(rt.span(), "`impl Trait` return types must be `impl Fn(..) -> ..`");
        };
        let cl = &out[k];
        let mut args: Vec<TySyn> = sig.generics.iter().map(|g| TySyn::Named(g.0.clone())).collect();
        for (n, _) in &cl.fields {
            if cl.by_move {
                let ty = f.sig.inputs.iter().zip(&sig.params).find_map(|(a, t)| match a {
                    syn::FnArg::Typed(pt) => match &*pt.pat {
                        syn::Pat::Ident(pi) if pi.ident == n => Some(t.clone()),
                        _ => None,
                    },
                    _ => None,
                });
                // (引数でなければ、本体の `let` の注釈か初期化式のリテラルから型を決める)
                // (接尾辞のない整数リテラルは、戻り値の型が整数ならそれ: `let mut c = 0; move || { c += 1; c }`)
                let int_hint = match &bret {
                    TySyn::Prim(t) if t.is_int() => Some(*t),
                    _ => None,
                };
                match ty.or_else(|| local_ty_syn(&f.block, n, int_hint)) {
                    Some(t) => args.push(t),
                    None => {
                        return err(
                            tc.span(),
                            format!("a returned closure can only capture parameters and locals with an obvious type (`{n}`; annotate it: `let {n}: T = ..`)"),
                        );
                    }
                }
            } else {
                return err(tc.span(), "a returned closure that captures must be `move`");
            }
        }
        for (i, p) in tc.inputs.iter().enumerate() {
            if closure::param(p).1.is_none() {
                match bargs.get(i) {
                    Some(t) => args.push(t.clone()),
                    None => return err(p.span(), "closure has more parameters than the bound"),
                }
            }
        }
        args.push(cl.ret_ann.clone().unwrap_or(bret));
        sig.ret = TySyn::App(cl.name.clone(), args);
    }
    sm.closures = out;
    Ok(())
}

/// struct / enum 定義の中での `Self`
fn self_of(name: &str, generics: &[String]) -> TySyn {
    if generics.is_empty() {
        TySyn::Named(name.to_string())
    } else {
        TySyn::App(
            name.to_string(),
            generics.iter().map(|g| TySyn::Named(g.clone())).collect(),
        )
    }
}

impl Program {
    pub fn build(items: &[SrcItem], sums: &[Summary]) -> R<Program> {
        let mut p = Program {
            names: vec![],
            index: FxMap::default(),
            sigs: vec![],
            item_of: vec![],
            structs: AdtTable::new(),
            struct_index: FxMap::default(),
            variant_names: FxMap::default(),
            opaque: vec![],
            prelude_from: u32::MAX,
            overflow_checks: false,
            opt: false,
            blanket: FxMap::default(),
            blanket_selfs: Default::default(),
            by_last: FxMap::default(),
            traits: FxMap::default(),
            impls: HashSet::new(),
            panic_handler: None,
            consts: vec![],
            const_index: FxMap::default(),
            const_by_last: FxMap::default(),
            prim_impls: HashSet::new(),
            closures: FxMap::default(),
            closure_by: FxMap::default(),
            closure_of_struct: FxMap::default(),
            drop_impls: Default::default(),
            has_drop: false,
            assocs: vec![],
            peq: vec![],
            aliases: FxMap::default(),
            pord: vec![],
        };
        // 0. 型の別名 (struct のフィールドの型より先に)
        for sm in sums {
            for (n, g, t) in &sm.aliases {
                p.aliases.insert(n.clone(), (g.clone(), t.clone()));
            }
        }
        // 0. const / static の名前 (配列の長さで使うので型より先に)
        for (ii, sm) in sums.iter().enumerate() {
            let cs = sm
                .consts
                .iter()
                .map(|(n, t, e)| (n, t, e, None))
                .chain(sm.statics.iter().map(|(n, t, e, m)| (n, t, e, Some(*m))));
            for (n, _, e, stat) in cs {
                if p.const_index.contains_key(n) {
                    return Err(format!(
                        "{}:1: duplicate definition of `{n}`",
                        items[ii].line
                    ));
                }
                if let Some((_, last)) = n.rsplit_once("::") {
                    p.const_by_last.entry(last.to_string()).or_default().push(p.consts.len() as u32);
                }
                p.const_index.insert(n.clone(), p.consts.len() as u32);
                p.consts.push(ConstDef {
                    name: n.clone(),
                    ty: Ty::Unit,
                    expr: e.clone(),
                    item: ii as u32,
                    stat,
                });
            }
        }
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
            for c in &sm.closures {
                p.struct_index.insert(c.name.clone(), sdefs.len() as u32);
                sdefs.push((
                    ii,
                    c.name.clone(),
                    AdtShape::Struct(c.fields.clone()),
                    c.generics.clone(),
                ));
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
        // レイアウトの途中で作ったインスタンス (`Node` の中の `RefCell<Node>`) は、まだ大きさの決まっていない
        // 型で計算されているので、全部決まってから取り直す
        p.relayout_instances();
        for sid in 0..p.structs.len() {
            let sd = &p.structs[sid];
            if !sd.is_enum || sd.base.is_some() || sd.name.starts_with('#') {
                continue;
            }
            for (vi, v) in sd.variants.iter().enumerate() {
                p.variant_names
                    .entry(v.name.clone())
                    .and_modify(|e| *e = None)
                    .or_insert(Some((sid as u32, vi)));
            }
        }
        // deep hash = 値・参照を問わずフィールドから推移的に届く全 struct の定義ハッシュ。
        // 参照経由 (`&'a Other`) でもフィールドオフセットはコードに埋まるので含める。
        let n_defs = sdefs.len();
        // `impl Drop` の有無も、その型を使う関数のコード (drop の呼び出し) を変えるので deep hash に入れる
        let drop_names: HashSet<&str> = sums
            .iter()
            .filter(|sm| sm.impl_trait.as_deref() == Some("Drop"))
            .filter_map(|sm| sm.impl_self.as_deref())
            .collect();
        let deeps: Vec<Key> = (0..n_defs)
            .map(|id| {
                let mut seen = HashSet::new();
                let mut stack = vec![Ty::Struct(id as u32)];
                let mut hs = vec![];
                while let Some(t) = stack.pop() {
                    match t {
                        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => {
                            stack.push(*i)
                        }
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
                            if (sid as usize) < n_defs && sdefs[sid as usize].1.contains("{closure#") {
                                // クロージャの struct: レイアウトは型引数 (キャプチャの型) で決まり、本体は
                                // クロージャの fn の側のキーに入るので、親アイテムのハッシュは入れない
                                // (クロージャを 1 つ編集しても、それを受け取る map などのインスタンスは作り直さない)
                                let mut kh = KeyHasher::new();
                                kh.add(&sdefs[sid as usize].1);
                                hs.push(kh.finish());
                            } else if (sid as usize) < n_defs {
                                hs.push(items[sdefs[sid as usize].0].hash);
                                if drop_names.contains(sdefs[sid as usize].1.as_str()) {
                                    hs.push(0xd0d0);
                                }
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
        // const / static の型
        let mut k = 0;
        for (it, sm) in items.iter().zip(sums) {
            for (_, t, _) in &sm.consts {
                p.consts[k].ty = p.resolve(t).map_err(|e| format!("{}:1: {e}", it.line))?;
                k += 1;
            }
            for (_, t, _, _) in &sm.statics {
                p.consts[k].ty = p.resolve(t).map_err(|e| format!("{}:1: {e}", it.line))?;
                k += 1;
            }
        }
        // 関連型 (関数のシグネチャの `Self::Item` を解決するので先に)
        for (it, sm) in items.iter().zip(sums) {
            for a in &sm.assocs {
                let gn: Vec<&str> = a.generics.iter().map(|g| g.as_str()).collect();
                let self_ty = p
                    .resolve_g(&a.self_ty, &gn)
                    .map_err(|e| format!("{}:1: {e}", it.line))?;
                let value = p
                    .resolve_g(&a.value, &gn)
                    .map_err(|e| format!("{}:1: {e}", it.line))?;
                p.assocs.push(AssocDef {
                    name: a.name.clone(),
                    self_ty,
                    value,
                    ngen: gn.len(),
                });
            }
        }
        // 3. 関数
        for (ii, (it, sm)) in items.iter().zip(sums).enumerate() {
            let first = p.names.len() as u32;
            let cnames: Vec<String> = sm.closures.iter().map(|c| format!("{}::call", c.name)).collect();
            let fns = sm
                .fns
                .iter()
                .map(|(n, s)| (n, s))
                .chain(cnames.iter().zip(sm.closures.iter().map(|c| &c.sig)));
            for (name, ss) in fns {
                if p.index.contains_key(name) {
                    return Err(format!("{}:1: duplicate definition of `{name}`", it.line));
                }
                let gnames: Vec<&str> = ss.generics.iter().map(|g| g.0.as_str()).collect();
                let mut params = vec![];
                for t in &ss.params {
                    params.push(
                        p.resolve_g(t, &gnames)
                            .map_err(|e| format!("{}:1: {e}", it.line))?,
                    );
                }
                // (本体から決める `impl Trait` はいったん () にして、後で resolve_opaque が入れる)
                let ret = if ss.ret == TySyn::Named(OPAQUE_RET.into()) {
                    p.opaque.push(p.names.len() as u32);
                    Ty::Unit
                } else {
                    p.resolve_g(&ss.ret, &gnames)
                        .map_err(|e| format!("{}:1: {e}", it.line))?
                };
                let self_ty = match &ss.self_ty {
                    Some(t) => Some(
                        p.resolve_g(t, &gnames)
                            .map_err(|e| format!("{}:1: {e}", it.line))?,
                    ),
                    None => None,
                };
                let mut blanket = false;
                let impl_of = match &ss.impl_of {
                    Some(n) if Ty::prim(n).is_some() || n == "__array" || n == "__slice" || n == "str" => None,
                    Some(n) => match p.struct_index.get(n).copied().or_else(|| {
                        n.strip_prefix("#tuple").and_then(|k| k.parse().ok()).map(|k| p.tuple_base(k))
                    }).as_ref() {
                        Some(&sid) => Some(sid),
                        // ブランケット impl (`impl<T: Display> Describe for T`): どの型のメソッドとしても探す
                        None if ss.generics.iter().any(|g| &g.0 == n) && ss.has_self => {
                            blanket = true;
                            p.blanket_selfs.insert(n.clone());
                            None
                        }
                        None => {
                            return Err(format!("{}:1: cannot find struct `{n}` for impl", it.line));
                        }
                    },
                    None => None,
                };
                if ss.is_extern && params.iter().chain([&ret]).any(|t| t.is_agg()) {
                    return Err(format!(
                        "{}:1: passing structs by value to extern fns is not supported",
                        it.line
                    ));
                }
                let last = name.rsplit("::").next().unwrap().to_string();
                // (`io::stdin()` / `mem::swap(..)` は prelude の `__core_*`: 名前の後ろで引けるようにする)
                if let Some(short) = last.strip_prefix("__core_") {
                    p.by_last.entry(short.to_string()).or_default().push(p.names.len() as u32);
                }
                p.by_last
                    .entry(last)
                    .or_default()
                    .push(p.names.len() as u32);
                if blanket {
                    let m = name.rsplit("::").next().unwrap().to_string();
                    p.blanket.entry(m).or_insert(p.names.len() as u32);
                }
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
                    export: ss.export,
                    panic_handler: ss.panic_handler,
                });
                if ss.panic_handler {
                    p.panic_handler = Some(p.names.len() as u32 - 1);
                }
                if let Some(tr) = &ss.trait_of {
                    p.traits
                        .entry(tr.clone())
                        .or_default()
                        .push(p.names.len() as u32 - 1);
                }
            }
            for (k, c) in sm.closures.iter().enumerate() {
                let fid = first + sm.fns.len() as u32 + k as u32;
                let sid = p.struct_index[&c.name];
                let gnames: Vec<&str> = c.generics.iter().map(|g| g.as_str()).collect();
                let ret_ann = match &c.ret_ann {
                    Some(t) => Some(
                        p.resolve_g(t, &gnames)
                            .map_err(|e| format!("{}:1: {e}", it.line))?,
                    ),
                    None => None,
                };
                p.closures.insert(
                    fid,
                    ClosureDef {
                        parent: first,
                        k: k as u32,
                        sid,
                        captures: c.fields.iter().map(|f| f.0.clone()).collect(),
                        by_move: c.by_move,
                        n_parent: c.n_parent as usize,
                        ret_ann,
                    },
                );
                p.closure_by.insert((first, k as u32), fid);
                p.closure_of_struct.insert(sid, fid);
            }
            if let Some(tr) = &sm.trait_decl {
                p.traits.entry(tr.clone()).or_default();
            }
        }
        p.peq = (0..p.names.len() as u32)
            .filter(|&i| {
                let n = &p.names[i as usize];
                n.ends_with("::eq") && n.contains("::<PartialEq<")
            })
            .collect();
        p.pord = (0..p.names.len() as u32)
            .filter(|&i| {
                let n = &p.names[i as usize];
                n.ends_with("::partial_cmp") && n.contains("::<PartialOrd<")
            })
            .collect();
        // 4. trait の実装 (impl ヘッダの要約から)
        for (it, sm) in items.iter().zip(sums) {
            let (Some(sn), Some(tr)) = (&sm.impl_self, &sm.impl_trait) else {
                continue;
            };
            // (右辺の型を持つ比較の impl と、配列・スライスへの impl は、`==` が関数を直接選ぶので数えない)
            if tr.starts_with("PartialEq<") || tr.starts_with("PartialOrd<") || sn == "__array" || sn == "__slice" {
                continue;
            }
            if !p.traits.contains_key(tr) {
                return Err(format!("{}:1: cannot find trait `{tr}`", it.line));
            }
            // プリミティブ型への trait 実装 (`impl Tr for u32`)
            // (`impl Hash for str`: str も組み込みの型として持つ)
            if let Some(pt) = Ty::prim(sn).or((sn == "str").then_some(Ty::Str)) {
                if !p.prim_impls.insert((pt, tr.clone())) {
                    return Err(format!(
                        "{}:1: conflicting implementations of trait `{tr}` for `{sn}`",
                        it.line
                    ));
                }
                continue;
            }
            let Some(&sid) = p.struct_index.get(sn).or(sn.strip_prefix("#tuple").and_then(|k| k.parse().ok()).map(|k| p.tuple_base(k)).as_ref()) else {
                if p.blanket_selfs.contains(sn) {
                    continue;
                }
                return Err(format!("{}:1: cannot find type `{sn}`", it.line));
            };
            // (参照への impl は trait の実装としては数えない: 演算子だけが `__ref_*` を呼ぶ)
            if matches!(sm.impl_self_ty, Some(TySyn::Ref(..))) {
                continue;
            }
            if !p.impls.insert((sid, tr.clone())) {
                return Err(format!(
                    "{}:1: conflicting implementations of trait `{tr}` for `{sn}`",
                    it.line
                ));
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
        // 必須メソッドが実装されているか (derive したものは本体を codegen が作る)
        let derived: HashSet<(u32, String)> = sums
            .iter()
            .flat_map(|sm| sm.derives.iter())
            .filter_map(|(tn, ds)| p.struct_index.get(tn).map(|&sid| (sid, ds)))
            .flat_map(|(sid, ds)| ds.iter().map(move |d| (sid, d.clone())))
            .collect();
        for (sid, tr) in &p.impls {
            if derived.contains(&(*sid, tr.clone())) {
                continue;
            }
            let Some(decls) = p.traits.get(tr) else {
                continue;
            }; // derive した組み込み trait
            for &d in decls {
                let m = p.names[d as usize].rsplit("::").next().unwrap();
                // (Display / Debug のメソッドは `S::<Display>::fmt`)
                let found = if matches!(tr.as_str(), "Display" | "Debug") {
                    p.index
                        .contains_key(&format!("{}::<{tr}>::{m}", p.structs[*sid as usize].name))
                } else {
                    p.method(*sid, m).is_some()
                };
                if !p.sigs[d as usize].has_body && !found {
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
        p.drop_impls = p
            .impls
            .iter()
            .filter(|(_, tr)| tr == "Drop")
            .map(|(sid, _)| *sid)
            .collect();
        p.has_drop = p.index.contains_key(crate::prelude::DROP_GLUE);
        Ok(p)
    }

    /// 型 t の値を捨てるときに drop glue が要るか (Box / `impl Drop` / それらを含む型)
    pub fn needs_drop(&self, t: Ty) -> bool {
        if !self.has_drop {
            return false;
        }
        match t {
            Ty::Box(_) => true,
            Ty::Array(e, n) => n > 0 && self.needs_drop(*e),
            Ty::Struct(sid) => {
                let sd = &self.structs[sid as usize];
                self.drop_impls.contains(&self.base_of(sid))
                    || sd.fields.iter().any(|f| self.needs_drop(f.1))
                    || sd
                        .variants
                        .iter()
                        .any(|v| v.fields.iter().any(|f| self.needs_drop(f.1)))
            }
            _ => false,
        }
    }

    /// 型 t が trait tr を実装しているか
    pub fn implements(&self, t: Ty, tr: &str) -> bool {
        // (const ジェネリクスの引数は長さを持つ配列型)
        if tr == CONST_GENERIC {
            return matches!(t, Ty::Array(Ty::Unit, _));
        }
        // `Iterator<Item = T>` は trait の名前で見る (関連型の制約は呼び出し側で解く)
        if let Some(i) = tr.find('<') {
            if parse_fn_bound(tr).is_none() {
                return self.implements(t, tr[..i].trim());
            }
        }
        match t {
            Ty::Struct(sid) if self.closure_of_struct.contains_key(&self.base_of(sid)) => {
                // クロージャ: Fn 系と、キャプチャがすべて Copy なら Copy / Clone
                parse_fn_bound(tr).is_some()
                    || matches!(tr, "Fn" | "FnMut" | "FnOnce" | "Sized")
                    || (matches!(tr, "Copy" | "Clone")
                        && self.structs[sid as usize]
                            .fields
                            .iter()
                            .all(|f| self.implements(f.1, tr)))
            }
            // flat_map / flatten の中身: イテレータもよい
            Ty::Struct(_) if tr == "__Flat" && self.implements(t, "Iterator") => true,
            // タプル: 要素がすべて実装していれば (core の impl と同じ)
            Ty::Struct(sid) if self.is_tuple(sid) => {
                if matches!(
                    tr,
                    "Copy" | "Clone" | "Sized" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash" | "Debug" | "Default"
                ) {
                    self.structs[sid as usize].fields.iter().all(|f| self.implements(f.1, tr))
                } else {
                    // ユーザの `impl<A, B> Tr for (A, B)`
                    self.impls.contains(&(self.base_of(sid), tr.to_string()))
                }
            }
            // prelude のコレクションと Option は Default (空 / None。中身が 0 のビット列)
            Ty::Struct(sid)
                if tr == "Default"
                    && matches!(
                        self.structs[self.base_of(sid) as usize].name.as_str(),
                        "Vec" | "String" | "HashMap" | "BinaryHeap" | "Option"
                    ) =>
            {
                true
            }
            // eq / clone メソッドを持つ型 (prelude の String / Vec など)
            Ty::Struct(sid)
                if (tr == "PartialEq" && self.method(sid, "eq").is_some())
                    || (tr == "Clone" && self.method(sid, "clone").is_some()) =>
            {
                true
            }
            // Vec / String は要素しだいで比較できる
            Ty::Struct(sid)
                if matches!(tr, "PartialOrd" | "Ord" | "Eq" | "Hash" | "PartialEq" | "Clone")
                    && matches!(self.structs[self.base_of(sid) as usize].name.as_str(), "Vec" | "String") =>
            {
                self.structs[sid as usize].args.iter().all(|&a| self.implements(a, tr))
            }
            Ty::Struct(sid) => self.impls.contains(&(self.base_of(sid), tr.to_string())),
            // (配列・スライス: 比較・Debug・Hash は要素が実装していれば)
            Ty::Array(e, _) => {
                tr == "Sized"
                    || (matches!(tr, "Copy" | "Clone" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Debug" | "Hash" | "Default")
                        && self.implements(*e, tr))
            }
            Ty::Str => matches!(tr, "Display" | "Debug" | "PartialEq" | "Eq" | "Hash" | "PartialOrd" | "Ord"),
            Ty::Slice(e) => matches!(tr, "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Debug" | "Hash") && self.implements(*e, tr),
            Ty::Dyn(_) => false,
            // 関数ポインタ: Fn 系 (引数の数が合うもの) と Copy
            Ty::Fn(f) => match parse_fn_bound(tr) {
                Some((_, ps, _)) => ps.len() == f.params.len(),
                None => matches!(tr, "Fn" | "FnMut" | "FnOnce" | "Copy" | "Clone" | "Sized" | "PartialEq" | "Eq"),
            },
            // 組み込み型: マーカー trait と比較・既定値など (core が実装しているもの)
            Ty::Ref(true, _) => tr == "Sized",
            Ty::Box(_) => matches!(tr, "Sized" | "Drop"),
            // `&T: Display` / `Debug` / `Hash` は T しだい
            Ty::Ref(_, i) if matches!(tr, "Display" | "Debug" | "Hash" | "PartialOrd" | "Ord") => {
                self.implements(*i, tr)
            }
            Ty::Ptr(..) | Ty::Ref(..) => {
                matches!(tr, "Copy" | "Clone" | "Sized" | "PartialEq" | "Eq")
            }
            Ty::F32 | Ty::F64 => matches!(
                tr,
                "Copy" | "Clone" | "Sized" | "PartialEq" | "PartialOrd" | "Default" | "Debug" | "Display"
            ) || self.prim_impls.contains(&(t, tr.to_string())),
            t if t.is_int() || matches!(t, Ty::Bool | Ty::Char) => {
                matches!(
                    tr,
                    "Copy"
                        | "Clone"
                        | "Sized"
                        | "PartialEq"
                        | "Eq"
                        | "PartialOrd"
                        | "Ord"
                        | "Default"
                        | "Debug"
                        | "Display"
                        | "Hash"
                ) || self.prim_impls.contains(&(t, tr.to_string()))
            }
            _ => matches!(tr, "Copy" | "Clone" | "Sized"),
        }
    }

    /// プリミティブ型 t のメソッド (trait 実装のもの → なければ trait のデフォルトメソッド)
    pub fn prim_method(&self, t: Ty, name: &str) -> Option<u32> {
        if let Some(&id) = self.index.get(&format!("{}::{name}", self.tn(t))) {
            return Some(id);
        }
        // (`impl<T: Hash> Hash for [T]` / `for [T; N]` のメソッドは `__slice::m` / `__array::m`)
        let base = match t {
            Ty::Slice(_) => Some("__slice"),
            Ty::Array(..) => Some("__array"),
            _ => None,
        };
        if let Some(&id) = base.and_then(|g| self.index.get(&format!("{g}::{name}"))) {
            return Some(id);
        }
        let mut found = None;
        for (pt, tr) in &self.prim_impls {
            if *pt != t {
                continue;
            }
            for &d in self.traits.get(tr).map(|v| v.as_slice()).unwrap_or(&[]) {
                if self.sigs[d as usize].has_body
                    && self.names[d as usize].rsplit("::").next() == Some(name)
                {
                    found = Some(d);
                }
            }
        }
        found.or_else(|| self.blanket.get(name).copied())
    }

    /// struct sid に名前 name のメソッドが見つからないとき、実装している trait のデフォルトメソッド
    pub fn default_method(&self, sid: u32, name: &str) -> Option<u32> {
        let r = self.default_method_inner(sid, name);
        // (なければブランケット impl のメソッド)
        r.or_else(|| self.blanket.get(name).copied())
    }

    fn default_method_inner(&self, sid: u32, name: &str) -> Option<u32> {
        let sid = self.base_of(sid);
        let mut found = None;
        for (s, tr) in &self.impls {
            if *s != sid {
                continue;
            }
            let Some(decls) = self.traits.get(tr) else {
                continue;
            };
            for &d in decls {
                if self.sigs[d as usize].has_body
                    && self.names[d as usize].rsplit("::").next() == Some(name)
                {
                    found = Some(d);
                }
            }
        }
        found
    }

    /// TySyn → Ty。generics に含まれる名前は型パラメータになる
    pub fn resolve_g(&self, t: &TySyn, generics: &[&str]) -> R<Ty> {
        self.resolve_with(t, &|n| {
            generics
                .iter()
                .position(|g| *g == n)
                .map(|i| Ty::Param(i as u8))
        })
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
            TySyn::Dyn(s) => Ty::Dyn(intern_str(s)),
            TySyn::Infer => Ty::Param(HOLE),
            TySyn::Assoc(b, n) => {
                let bt = r(b)?;
                if self.has_param(bt) {
                    Ty::Assoc(intern_assoc(AssocTy {
                        base: bt,
                        name: n.clone(),
                    }))
                } else {
                    match self.assoc_of(bt, n) {
                        Some(t) => t,
                        None => {
                            return Err(format!(
                                "cannot find associated type `{n}` for `{}`",
                                self.tn(bt)
                            ));
                        }
                    }
                }
            }
            TySyn::FnPtr(v) => {
                let mut ts = v.iter().map(r).collect::<R<Vec<Ty>>>()?;
                let ret = ts.pop().unwrap_or(Ty::Unit);
                Ty::Fn(intern_fn(FnTy { params: ts, ret }))
            }
            TySyn::Array(inner, len) => {
                let n = match len {
                    ArrLen::Lit(n) => *n,
                    // (const ジェネリクスの N)
                    ArrLen::Const(c) => match names(c) {
                        Some(Ty::Param(i)) => (CONST_LEN_BASE + i as u32) as u64,
                        // (インスタンスの中: N は長さを持つ配列型 / 長さが N の型引数 `[(); N]`)
                        Some(Ty::Array(Ty::Unit, m)) => m as u64,
                        _ => self.const_usize(c)?,
                    },
                };
                Ty::Array(intern(r(inner)?), n as u32)
            }
            TySyn::Named(n) => {
                if let Some(t) = names(n) {
                    return Ok(t);
                }
                if let Some((g, t)) = self.aliases.get(n) {
                    if g.is_empty() {
                        return self.resolve_with(t, names);
                    }
                }
                match self.struct_index.get(n) {
                    Some(&id) if self.structs[id as usize].generics.is_empty() => Ty::Struct(id),
                    Some(_) => return Err(format!("missing generics for `{n}`")),
                    // (`zeros::<K>()` / `Buf<K>`: const の名前は const ジェネリクスの引数)
                    None if self.const_index.contains_key(n) => Ty::Array(intern(Ty::Unit), self.const_usize(n)? as u32),
                    None => return Err(format!("cannot find type `{n}`")),
                }
            }
            // `Box<dyn Error>`: prelude の __DynError (表示と Debug の文字列を持つ)
            TySyn::App(n, args)
                if n == "Box"
                    && matches!(args.as_slice(), [TySyn::Dyn(d)] if d == "Error")
                    && self.struct_index.contains_key("__DynError") =>
            {
                Ty::Struct(self.struct_index["__DynError"])
            }
            TySyn::App(n, args) if n == "Box" && args.len() == 1 && !self.struct_index.contains_key(n) => {
                Ty::Box(intern(r(&args[0])?))
            }
            // 型パラメータを持つ別名 (`type R<T> = Result<T, String>;`)
            TySyn::App(n, args) if self.aliases.get(n).is_some_and(|a| a.0.len() == args.len()) => {
                let (g, t) = self.aliases[n].clone();
                let vals = args.iter().map(r).collect::<R<Vec<Ty>>>()?;
                return self.resolve_with(&t, &|x| match g.iter().position(|y| y == x) {
                    Some(i) => Some(vals[i]),
                    None => names(x),
                });
            }
            TySyn::App(n, args) => {
                let Some(&id) = self.struct_index.get(n) else {
                    return Err(format!("cannot find type `{n}`"));
                };
                // `HashMap<K, V, S>` / `HashSet<K, S>`: ハッシャの型 (BuildHasherDefault<..>) は無視して既定のハッシュで
                let mut args = args.as_slice();
                if matches!(n.as_str(), "HashMap" | "HashSet") && args.len() == self.structs[id as usize].generics.len() + 1 {
                    args = &args[..args.len() - 1];
                }
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

    /// 配列の長さに使う const (要約の const 表から整数の定数式として評価する)
    fn const_usize(&self, name: &str) -> R<u64> {
        let v = self.const_int(name, 0)?;
        u64::try_from(v).map_err(|_| format!("constant `{name}` is negative"))
    }

    /// 整数の const を評価する (リテラル・四則演算・ビット演算・シフト・キャスト・他の const)
    pub fn const_int(&self, name: &str, depth: u32) -> R<i128> {
        if depth > 64 {
            return Err(format!("cycle while evaluating constant `{name}`"));
        }
        let Some(&ci) = self.const_index.get(name) else {
            return Err(format!("cannot find constant `{name}`"));
        };
        let src = &self.consts[ci as usize].expr;
        let e: syn::Expr = syn::parse_str(src).map_err(|e| e.to_string())?;
        self.eval_int(&e, depth + 1)
    }

    pub fn eval_int(&self, e: &syn::Expr, depth: u32) -> R<i128> {
        use syn::{BinOp, Expr, Lit, UnOp};
        Ok(match e {
            Expr::Lit(l) => match &l.lit {
                Lit::Int(i) => i.base10_parse::<i128>().or_else(|_| i.base10_parse::<u128>().map(|v| v as i128)).map_err(|e| e.to_string())?,
                Lit::Bool(b) => b.value as i128,
                Lit::Byte(b) => b.value() as i128,
                Lit::Char(c) => c.value() as i128,
                _ => return Err("not an integer constant".into()),
            },
            Expr::Paren(p) => self.eval_int(&p.expr, depth)?,
            Expr::Group(g) => self.eval_int(&g.expr, depth)?,
            Expr::Cast(c) => self.eval_int(&c.expr, depth)?,
            Expr::Unary(u) => {
                let v = self.eval_int(&u.expr, depth)?;
                match u.op {
                    UnOp::Neg(_) => -v,
                    UnOp::Not(_) => !v,
                    _ => return Err("not an integer constant".into()),
                }
            }
            Expr::Binary(b) => {
                let (l, r) = (
                    self.eval_int(&b.left, depth)?,
                    self.eval_int(&b.right, depth)?,
                );
                match b.op {
                    BinOp::Add(_) => l + r,
                    BinOp::Sub(_) => l - r,
                    BinOp::Mul(_) => l * r,
                    BinOp::Div(_) if r != 0 => l / r,
                    BinOp::Rem(_) if r != 0 => l % r,
                    BinOp::Shl(_) => l << r,
                    BinOp::Shr(_) => l >> r,
                    BinOp::BitAnd(_) => l & r,
                    BinOp::BitOr(_) => l | r,
                    BinOp::BitXor(_) => l ^ r,
                    _ => return Err("unsupported operator in constant".into()),
                }
            }
            Expr::Path(p) if p.path.get_ident().is_some() => {
                self.const_int(&p.path.get_ident().unwrap().to_string(), depth)?
            }
            Expr::Path(p) if p.path.segments.len() == 2 => {
                let t = Ty::prim(&p.path.segments[0].ident.to_string())
                    .ok_or("unsupported path in constant")?;
                prim_assoc_const(t, &p.path.segments[1].ident.to_string())
                    .ok_or("unsupported constant")?
            }
            _ => return Err("unsupported expression in constant".into()),
        })
    }

    /// n 要素のタプルの (組み込みの) ジェネリック定義
    pub fn tuple_base(&self, n: usize) -> u32 {
        if let Some(&id) = self.structs.tuples.lock().unwrap().get(&n) {
            return id;
        }
        let mut d = StructDef::blank(format!("#tuple{n}"), false);
        d.generics = (0..n).map(|i| format!("T{i}")).collect();
        d.fields = (0..n)
            .map(|i| (i.to_string(), Ty::Param(i as u8), 0))
            .collect();
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
        self.structs[self.base_of(sid) as usize]
            .name
            .starts_with("#tuple")
    }

    /// インスタンスなら元の定義
    pub fn base_of(&self, sid: u32) -> u32 {
        self.structs[sid as usize].base.unwrap_or(sid)
    }

    /// ジェネリックな struct / enum に型引数を入れる。型引数がすべて具体型ならレイアウトも決める。
    /// 同じ (定義, 型引数) には同じ id を返す
    /// `-> impl Iterator<..>` の戻り値を本体の型推論で決める (呼び出し先も同じ形なら数回まわす)
    pub fn resolve_opaque(&mut self, src: &str, items: &[SrcItem]) -> R<()> {
        if self.opaque.is_empty() {
            return Ok(());
        }
        let ids = self.opaque.clone();
        let mut done = vec![false; ids.len()];
        for _ in 0..4 {
            let mut progress = false;
            for (k, &id) in ids.iter().enumerate() {
                if done[k] {
                    continue;
                }
                let sig = self.sigs[id as usize].clone();
                if sig.is_generic() {
                    return Err(format!("`{}`: generic functions returning `impl Trait` are not supported", self.names[id as usize]));
                }
                let Some(ii) = self.item_of[id as usize] else { continue };
                let it = &items[ii as usize];
                let syn::Item::Fn(f) = parse_item(src, it)? else { continue };
                let env = TyEnv { self_sid: sig.impl_of, params: vec![], self_ty: sig.self_ty };
                let (_, r, _) = crate::infer::infer_ctx(self, &f, &sig, &env, &[], None, Some((id, 0)));
                if let Some(t) = r.filter(|&t| t != Ty::Unit && !self.has_param(t)) {
                    self.sigs[id as usize].ret = t;
                    done[k] = true;
                    progress = true;
                }
            }
            if !progress {
                break;
            }
        }
        if let Some(k) = done.iter().position(|d| !d) {
            return Err(format!("cannot infer the `impl Trait` return type of `{}`", self.names[ids[k] as usize]));
        }
        Ok(())
    }

    fn relayout_instances(&mut self) {
        for _ in 0..8 {
            let mut ids: Vec<(u32, u32, Vec<Ty>)> =
                self.structs.inst.lock().unwrap().iter().map(|((b, a), &id)| (id, *b, a.clone())).collect();
            ids.sort_by_key(|x| x.0);
            let mut changed = false;
            for (id, base, args) in ids {
                if args.iter().any(|&t| self.has_param(t)) {
                    continue;
                }
                let b = &self.structs[base as usize];
                if b.is_enum {
                    let mut vs = vec![];
                    for v in &b.variants {
                        let fs = v.fields.iter().map(|(n, t, _)| (n.clone(), self.subst(*t, &args))).collect();
                        vs.push((v.name.clone(), v.kind, fs, v.discr));
                    }
                    let tag = b.tag;
                    let (vs, size, align) = self.enum_layout(vs, tag);
                    let d = self.structs.get_mut(id as usize);
                    if d.size != size || d.align != align {
                        changed = true;
                    }
                    d.variants = vs;
                    d.size = size;
                    d.align = align;
                } else {
                    let fs: Vec<(String, Ty)> =
                        b.fields.iter().map(|(n, t, _)| (n.clone(), self.subst(*t, &args))).collect();
                    let (fs, size, align) = self.struct_layout(fs);
                    let d = self.structs.get_mut(id as usize);
                    if d.size != size || d.align != align || d.fields.len() != fs.len() {
                        changed = true;
                    }
                    d.fields = fs;
                    d.size = size;
                    d.align = align;
                }
            }
            if !changed {
                break;
            }
        }
    }

    pub fn instantiate(&self, base: u32, args: Vec<Ty>) -> R<u32> {
        if let Some(&id) = self.structs.inst.lock().unwrap().get(&(base, args.clone())) {
            return Ok(id);
        }
        // 自分を (Box などの中に) 含む型 (`Node<T> { next: Option<Box<Node<T>>> }`): 作っている途中の
        // インスタンスがまた要求されたら、先に番号だけ取って返す (中身は外側が作り終えてから入れる)
        let key = (base, args.clone());
        let pending = INSTANTIATING.with(|v| {
            let mut v = v.borrow_mut();
            match v.iter_mut().find(|e| e.0 == key) {
                Some(e) => Some(*e.1.get_or_insert_with(|| {
                    let b = &self.structs[base as usize];
                    let names: Vec<String> = args.iter().map(|&t| self.tn(t)).collect();
                    let mut d = StructDef::blank(format!("{}<{}>", b.name, names.join(", ")), b.is_enum);
                    d.base = Some(base);
                    d.args = args.clone();
                    self.structs.push(d)
                })),
                None => {
                    v.push((key.clone(), None));
                    None
                }
            }
        });
        if let Some(id) = pending {
            return Ok(id);
        }
        let r = self.instantiate_inner(base, args);
        let placeholder = INSTANTIATING.with(|v| v.borrow_mut().pop().and_then(|e| e.1));
        let d = r?;
        let mut g = self.structs.inst.lock().unwrap();
        if let Some(&id) = g.get(&key) {
            return Ok(id);
        }
        let id = match placeholder {
            Some(pid) => {
                self.structs.replace(pid, d);
                pid
            }
            None => self.structs.push(d),
        };
        g.insert(key, id);
        Ok(id)
    }

    fn instantiate_inner(&self, base: u32, args: Vec<Ty>) -> R<StructDef> {
        let b = &self.structs[base as usize];
        if b.generics.len() != args.len() {
            return Err(format!(
                "`{}` takes {} type arguments, but {} were given",
                b.name,
                b.generics.len(),
                args.len()
            ));
        }
        let symbolic = args.iter().any(|&t| self.has_param(t));
        let names: Vec<String> = args.iter().map(|&t| self.tn(t)).collect();
        let name = if b.name.starts_with("#tuple") {
            if names.len() == 1 {
                format!("({},)", names[0])
            } else {
                format!("({})", names.join(", "))
            }
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
                    let fs = v
                        .fields
                        .iter()
                        .map(|(n, t, _)| (n.clone(), self.subst(*t, &args)))
                        .collect();
                    vs.push((v.name.clone(), v.kind, fs, v.discr));
                }
                let (vs, size, align) = self.enum_layout(vs, b.tag);
                d.variants = vs;
                d.size = size;
                d.align = align;
            } else {
                let fs: Vec<(String, Ty)> = b
                    .fields
                    .iter()
                    .map(|(n, t, _)| (n.clone(), self.subst(*t, &args)))
                    .collect();
                let (fs, size, align) = self.struct_layout(fs);
                d.fields = fs;
                d.size = size;
                d.align = align;
            }
        }
        Ok(d)
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
    fn enum_layout(
        &self,
        vs: Vec<(String, VariantKind, Vec<(String, Ty)>, i64)>,
        tag: Ty,
    ) -> (Vec<VariantDef>, u32, u32) {
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
            Ty::Array(_, n) if n >= CONST_LEN_BASE => true,
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => {
                self.has_param(*i)
            }
            Ty::Struct(sid) => self.structs[sid as usize]
                .args
                .iter()
                .any(|&a| self.has_param(a)),
            Ty::Fn(f) => f.params.iter().chain([&f.ret]).any(|&a| self.has_param(a)),
            // 関連型は具体型ではない (型パラメータを含むときだけ現れる)
            Ty::Assoc(_) => true,
            _ => false,
        }
    }

    /// 具体型 t の関連型 name (`impl Iterator for Counter { type Item = u32; }` なら u32)
    pub fn assoc_of(&self, t: Ty, name: &str) -> Option<Ty> {
        for a in &self.assocs {
            if a.name != name {
                continue;
            }
            let mut sol = vec![None; a.ngen];
            if self.match_ty(a.self_ty, t, &mut sol) {
                let Some(args) = sol.iter().copied().collect::<Option<Vec<Ty>>>() else {
                    continue;
                };
                return Some(self.subst(a.value, &args));
            }
        }
        None
    }

    /// 型パラメータに型引数を入れる
    pub fn subst(&self, t: Ty, args: &[Ty]) -> Ty {
        match t {
            Ty::Param(i) => args[i as usize],
            Ty::Ref(m, i) => Ty::Ref(m, intern(self.subst(*i, args))),
            Ty::Ptr(m, i) => Ty::Ptr(m, intern(self.subst(*i, args))),
            Ty::Array(i, n) => {
                let n = match n.checked_sub(CONST_LEN_BASE).map(|k| args[k as usize]) {
                    Some(Ty::Array(_, m)) => m,
                    _ => n,
                };
                Ty::Array(intern(self.subst(*i, args)), n)
            }
            Ty::Slice(i) => Ty::Slice(intern(self.subst(*i, args))),
            Ty::Box(i) => Ty::Box(intern(self.subst(*i, args))),
            Ty::Assoc(a) => {
                let b = self.subst(a.base, args);
                if self.has_param(b) {
                    Ty::Assoc(intern_assoc(AssocTy {
                        base: b,
                        name: a.name.clone(),
                    }))
                } else {
                    self.assoc_of(b, &a.name).unwrap_or(t)
                }
            }
            Ty::Fn(f) if self.has_param(t) => Ty::Fn(intern_fn(FnTy {
                params: f.params.iter().map(|&p| self.subst(p, args)).collect(),
                ret: self.subst(f.ret, args),
            })),
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
        let args: Vec<Ty> = (0..sol.len())
            .map(|i| sol[i].unwrap_or(Ty::Param(i as u8)))
            .collect();
        if args.is_empty() {
            t
        } else {
            self.subst(t, &args)
        }
    }

    /// 仮引数型 pt (型パラメータを含む) と実引数型 at を突き合わせて型パラメータを解く
    pub fn match_ty(&self, pt: Ty, at: Ty, sol: &mut [Option<Ty>]) -> bool {
        match (pt, at) {
            // 関連型からは解けない (具体型が分かってから置き換わる)
            (Ty::Assoc(_), _) => true,
            (Ty::Param(i), t) => match sol.get(i as usize).copied().flatten() {
                Some(prev) => prev == t,
                None => {
                    if let Some(s) = sol.get_mut(i as usize) {
                        *s = Some(t);
                    }
                    true
                }
            },
            (Ty::Ref(_, a), Ty::Ref(_, b))
            | (Ty::Ptr(_, a), Ty::Ptr(_, b))
            | (Ty::Slice(a), Ty::Slice(b))
            | (Ty::Box(a), Ty::Box(b)) => self.match_ty(*a, *b, sol),
            // (const ジェネリクスの長さ: 長さを持つ Ty::Array(Unit, m) で解く)
            (Ty::Array(a, n), Ty::Array(b, m)) if n >= CONST_LEN_BASE && m < CONST_LEN_BASE => {
                self.match_ty(Ty::Param((n - CONST_LEN_BASE) as u8), Ty::Array(intern(Ty::Unit), m), sol) && self.match_ty(*a, *b, sol)
            }
            (Ty::Array(a, n), Ty::Array(b, m)) => n == m && self.match_ty(*a, *b, sol),
            (Ty::Fn(a), Ty::Fn(b)) if a != b => {
                a.params.len() == b.params.len()
                    && a.params
                        .iter()
                        .zip(&b.params)
                        .chain([(&a.ret, &b.ret)])
                        .all(|(&p, &q)| self.match_ty(p, q, sol))
            }
            (Ty::Struct(a), Ty::Struct(b)) if a != b => {
                let (sa, sb) = (&self.structs[a as usize], &self.structs[b as usize]);
                match (sa.base, sb.base) {
                    (Some(x), Some(y)) if x == y => sa
                        .args
                        .iter()
                        .zip(&sb.args)
                        .all(|(&p, &q)| self.match_ty(p, q, sol)),
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
            Ty::Dyn(s) => TySyn::Dyn(s.to_string()),
            Ty::Box(i) => TySyn::App("Box".into(), vec![self.ty_syn(*i)]),
            Ty::Assoc(a) => TySyn::Assoc(Box::new(self.ty_syn(a.base)), a.name.clone()),
            Ty::Fn(f) => TySyn::FnPtr(
                f.params.iter().chain([&f.ret]).map(|&t| self.ty_syn(t)).collect(),
            ),
            Ty::Struct(id) => {
                let sd = &self.structs[id as usize];
                match sd.base {
                    Some(b) if self.is_tuple(b) => {
                        TySyn::Tuple(sd.args.iter().map(|&a| self.ty_syn(a)).collect())
                    }
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
            1 => {
                return Err(format!(
                    "recursive type `{}` has infinite size",
                    sdefs[id].1
                ));
            }
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
                AdtShape::Enum(vs) => vs
                    .iter()
                    .flat_map(|v| v.fields.iter().map(|x| x.1.clone()))
                    .collect(),
            }
        };
        let mut todo: Vec<(TySyn, Vec<String>)> = fields_of(shape)
            .into_iter()
            .map(|t| (t, generics.clone()))
            .collect();
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
                // `Box<T>` はポインタなので T のレイアウトは要らない
                TySyn::App(n, _) if n == "Box" && !self.struct_index.contains_key(&n) => {}
                TySyn::App(n, args) => {
                    // 型引数のうち、その型が値として持つもの (`Vec<T>` の T はポインタの先なので要らない)
                    let mut by_value = vec![true; args.len()];
                    if let Some(&sid) = self.struct_index.get(&n) {
                        let bg = sdefs[sid as usize].3.clone();
                        let fs = fields_of(&sdefs[sid as usize].2);
                        if bg.len() == args.len() {
                            for (k, g) in bg.iter().enumerate() {
                                by_value[k] = fs.iter().any(|f| syn_holds_by_value(f, g));
                            }
                        }
                        if visited_bases.insert(sid) {
                            todo.extend(fs.into_iter().map(|t| (t, bg.clone())));
                        }
                    }
                    todo.extend(args.into_iter().zip(by_value).filter(|(_, v)| *v).map(|(a, _)| (a, gs.clone())));
                }
                TySyn::Tuple(es) => todo.extend(es.into_iter().map(|a| (a, gs.clone()))),
                TySyn::Array(e, _) => todo.push((*e, gs)),
                _ => {}
            }
        }
        let res = |this: &Program, fs: &[(String, TySyn)]| -> R<Vec<(String, Ty)>> {
            fs.iter()
                .map(|(n, t)| {
                    Ok((
                        n.clone(),
                        this.resolve_g(t, &gnames)
                            .map_err(|e| format!("{line}:1: {e}"))?,
                    ))
                })
                .collect()
        };
        let generic = !generics.is_empty();
        match shape {
            AdtShape::Struct(fields) => {
                let fs = res(self, fields)?;
                if generic {
                    self.structs.get_mut(id).fields =
                        fs.into_iter().map(|(n, t)| (n, t, 0)).collect();
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
                        return Err(format!(
                            "{line}:1: discriminant value `{d}` assigned more than once"
                        ));
                    }
                    discrs.push(d);
                    next = d + 1;
                }
                let tag = if discrs.iter().all(|&d| (0..=255).contains(&d)) {
                    Ty::U8
                } else {
                    Ty::I32
                };
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
        if let Some(r) = self.variant_by_path_inner(p, self_sid) {
            return Some(r);
        }
        let q = self.strip_mods(p)?;
        self.variant_by_path_inner(&q, self_sid)
    }

    fn variant_by_path_inner(&self, p: &syn::Path, self_sid: Option<u32>) -> Option<(u32, usize)> {
        let segs: Vec<&syn::PathSegment> = p.segments.iter().collect();
        // `core::option::Option::Some` などは最後の 2 つで見る
        let segs = if segs.len() > 2
            && matches!(segs[0].ident.to_string().as_str(), "core" | "crate" | "std")
        {
            segs[segs.len() - 2..].to_vec()
        } else {
            segs
        };
        if segs.len() == 1 {
            let n = segs[0].ident.to_string();
            let en = match n.as_str() {
                "Some" | "None" => "Option",
                "Ok" | "Err" => "Result",
                // `use Expr::*;` したバリアント (大文字で始まり、ほかの enum と名前が重ならないもの)
                _ if n.starts_with(|c: char| c.is_ascii_uppercase()) => {
                    return self.variant_names.get(&n).copied().flatten();
                }
                _ => return None,
            };
            let sid = *self.struct_index.get(en)?;
            let vi = self.structs[sid as usize]
                .variants
                .iter()
                .position(|v| v.name == n)?;
            return Some((sid, vi));
        }
        if segs.len() != 2 || !segs[1].arguments.is_none() {
            return None;
        }
        let tn = segs[0].ident.to_string();
        let sid = if tn == "Self" {
            self_sid?
        } else {
            *self.struct_index.get(&tn)?
        };
        let sd = &self.structs[sid as usize];
        if !sd.is_enum {
            return None;
        }
        let vn = segs[1].ident.to_string();
        sd.variants
            .iter()
            .position(|v| v.name == vn)
            .map(|vi| (sid, vi))
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
                return env.self_ty.or(env.self_sid.map(Ty::Struct));
            }
            env.params.iter().find(|p| p.0 == n).map(|p| p.1)
        })
    }

    /// 型名として書かれたパス (`S` / `Self`) → struct id
    /// モジュールのパスの先頭 (`crate::` / `self::` / `super::` / 小文字のモジュール名) を外す。
    /// モジュールは平らにしてあるので、アイテムは最後の名前で引ける (`geo::inner::k` → `k`、`geo::P::new` → `P::new`)
    pub fn strip_mods(&self, p: &syn::Path) -> Option<syn::Path> {
        let mut k = 0;
        let n = p.segments.len();
        while k + 1 < n {
            let s = p.segments[k].ident.to_string();
            let module = matches!(s.as_str(), "crate" | "self" | "super")
                || (s.starts_with(|c: char| c.is_ascii_lowercase())
                    && !matches!(s.as_str(), "std" | "core" | "alloc" | "str" | "char" | "bool")
                    && Ty::prim(&s).is_none()
                    && !self.struct_index.contains_key(&s)
                    && p.segments[k].arguments.is_none());
            if !module {
                break;
            }
            k += 1;
        }
        if k == 0 {
            return None;
        }
        let mut q = p.clone();
        q.leading_colon = None;
        q.segments = p.segments.iter().skip(k).cloned().collect();
        Some(q)
    }

    pub fn struct_by_path(&self, p: &syn::Path, self_sid: Option<u32>) -> Option<u32> {
        // `std::cmp::Reverse` / `core::cmp::Ordering` などは最後のセグメント (prelude の型)
        if p.segments.len() > 1
            && matches!(p.segments[0].ident.to_string().as_str(), "std" | "core" | "alloc")
        {
            let last = p.segments.last().unwrap().ident.to_string();
            return self.struct_index.get(&last).copied();
        }
        if p.segments.len() > 1 {
            return self.struct_by_path(&self.strip_mods(p)?, self_sid);
        }
        let id = p.get_ident()?.to_string();
        if id == "Self" {
            return self_sid;
        }
        self.struct_index.get(&id).copied()
    }

    /// `i32::pow(2, 10)` / `u64::count_ones(x)` / `f64::abs(x)`: プリミティブのメソッドを関数の形で呼んだもの。
    /// 関数として見つからなければ、メソッド呼び出し `(2i32).pow(10)` に書き換える
    pub fn prim_method_call(&self, c: &syn::ExprCall, self_sid: Option<u32>) -> Option<syn::Expr> {
        let syn::Expr::Path(p) = &*c.func else { return None };
        if c.args.is_empty() || p.qself.is_some() {
            return None;
        }
        // `Trait::m(&x, a)` / `crate::m::Trait::m(..)` (self を取るユーザ trait のメソッド): `x.m(a)` に
        {
            let q = self.strip_mods(&p.path);
            let q = q.as_ref().unwrap_or(&p.path);
            if q.segments.len() == 2 && q.segments.iter().all(|s| s.arguments.is_none()) {
                let tr = q.segments[0].ident.to_string();
                let m = &q.segments[1].ident;
                let ms = m.to_string();
                if let Some(ds) = self.traits.get(&tr) {
                    let selfm = ds.iter().any(|&d| {
                        self.sigs[d as usize].has_self && self.names[d as usize].rsplit("::").next() == Some(ms.as_str())
                    });
                    if selfm && !matches!(tr.as_str(), "Ord" | "PartialOrd" | "PartialEq" | "Eq") {
                        let mut args = c.args.iter();
                        let a0 = args.next()?;
                        let rest: Vec<&syn::Expr> = args.collect();
                        let recv: syn::Expr = match a0 {
                            syn::Expr::Reference(r) => {
                                let e = &r.expr;
                                syn::parse_quote!((#e))
                            }
                            _ => syn::parse_quote!((#a0)),
                        };
                        return Some(syn::parse_quote!(#recv.#m(#(#rest),*)));
                    }
                }
            }
        }
        if p.path.segments.len() != 2 {
            return None;
        }
        let ty = p.path.segments[0].ident.to_string();
        // `HashMap::from([(1, 2), ..])`: 配列を vec![..] にして `from(Vec)` を呼ぶ
        if p.path.segments[1].ident == "from"
            && matches!(ty.as_str(), "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "VecDeque" | "BinaryHeap")
            && c.args.len() == 1
        {
            if let syn::Expr::Array(a) = &c.args[0] {
                let es = &a.elems;
                let f = &c.func;
                return Some(syn::parse_quote!(#f(vec![#es])));
            }
        }
        // `Ord::cmp(&a, &b)` / `PartialOrd::partial_cmp(..)` / `PartialEq::eq(..)`: メソッド呼び出しの形に
        if matches!(ty.as_str(), "Ord" | "PartialOrd" | "PartialEq" | "Eq") && c.args.len() == 2 {
            let m = &p.path.segments[1].ident;
            let (a0, a1) = (&c.args[0], &c.args[1]);
            return Some(syn::parse_quote!((#a0).#m(#a1)));
        }
        let t = Ty::prim(&ty)?;
        if t == Ty::Char || t == Ty::Bool {
            return None;
        }
        let m = &p.path.segments[1].ident;
        let ms = m.to_string();
        // `i64::from(7u32)` / `u32::from('a')`: 数値への変換はキャスト (広げる変換だけが書ける)
        if ms == "from" && c.args.len() == 1 && self.fn_by_path(&p.path, self_sid).is_none() {
            let a0 = &c.args[0];
            let tyi = &p.path.segments[0].ident;
            return Some(syn::parse_quote!(((#a0) as #tyi)));
        }
        if ms.starts_with("from") || ms.starts_with("try_from") || ms == "default" || ms == "new" {
            return None;
        }
        if self.fn_by_path(&p.path, self_sid).is_some() {
            return None;
        }
        let mut args = c.args.iter();
        let a0 = args.next()?;
        let rest: Vec<&syn::Expr> = args.collect();
        let tyi = &p.path.segments[0].ident;
        // (リテラルは型を付ける: `i32::pow(2, 10)` の 2 は i32)
        let recv: syn::Expr = if matches!(a0, syn::Expr::Lit(_)) {
            syn::parse_quote!((#a0 as #tyi))
        } else {
            syn::parse_quote!((#a0))
        };
        Some(syn::parse_quote!(#recv.#m(#(#rest),*)))
    }

    /// 関数として書かれたパス (`f` / `S::f` / `Self::f`) → 関数 ID
    pub fn fn_by_path(&self, p: &syn::Path, self_sid: Option<u32>) -> Option<u32> {
        if let Some(r) = self.fn_by_path_inner(p, self_sid) {
            return Some(r);
        }
        let q = self.strip_mods(p)?;
        self.fn_by_path_inner(&q, self_sid)
    }

    fn fn_by_path_inner(&self, p: &syn::Path, self_sid: Option<u32>) -> Option<u32> {
        // 最後のセグメントの turbofish (`f::<T>`) は呼び出し側で読む
        let n = p.segments.len();
        // (`Buf::<8>::new`: 型のセグメントの型引数も呼び出し側で読む)
        if n == 2 && !p.segments[0].arguments.is_none() && p.leading_colon.is_none() {
            let mut q = p.clone();
            q.segments[0].arguments = syn::PathArguments::None;
            return self.fn_by_path_inner(&q, self_sid);
        }
        if p.leading_colon.is_some()
            || p.segments
                .iter()
                .take(n.saturating_sub(1))
                .any(|s| !s.arguments.is_none())
        {
            return None;
        }
        // `std::collections::HashMap::new` などは `HashMap::new`
        if n >= 3 && matches!(p.segments[0].ident.to_string().as_str(), "core" | "std" | "alloc") {
            let ty = p.segments[n - 2].ident.to_string();
            if self.struct_index.contains_key(&ty) {
                return self.index.get(&format!("{ty}::{}", p.segments[n - 1].ident)).copied();
            }
        }
        // `std::process::exit` / `process::exit` (ユーザの `exit` と名前がぶつからないように別の名前)
        // `fs::read_to_string` / `fs::write` / `env::args` などは prelude の `__core_*`
        if n >= 2 {
            let (m, f) = (p.segments[n - 2].ident.to_string(), p.segments[n - 1].ident.to_string());
            let core = match (m.as_str(), f.as_str()) {
                ("fs", "read_to_string") => Some("__exr_fs_read_to_string"),
                ("fs", "read") => Some("__exr_fs_read"),
                ("fs", "write") => Some("__exr_fs_write"),
                ("env", "args") => Some("__exr_env_args"),
                ("env", "var") => Some("__exr_env_var"),
                _ => None,
            };
            if let Some(c) = core {
                return self.index.get(c).copied();
            }
        }
        if n >= 2 && p.segments[n - 2].ident == "process" && p.segments[n - 1].ident == "exit" {
            return self.index.get("__core_process_exit").copied();
        }
        // `core::cmp::max` / `core::mem::swap` などは prelude の `__core_*` へ
        if n >= 2 && matches!(p.segments[0].ident.to_string().as_str(), "core" | "std") {
            return self
                .index
                .get(&format!("__core_{}", p.segments[n - 1].ident))
                .copied();
        }
        match p.segments.len() {
            1 => {
                let name = p.segments[0].ident.to_string();
                // `use core::cmp::max;` してからの `max(..)`: ユーザの関数がなければ core のもの
                self.index
                    .get(&name)
                    .or_else(|| self.index.get(&format!("__core_{name}")))
                    .copied()
            }
            2 => {
                let ty = p.segments[0].ident.to_string();
                // `f64::max` / `f64::sqrt` (関数として): prelude の `__exr_f64_*`、`i32::max` は core::cmp::max
                if ty == "f64" || ty == "f32" {
                    return self.index.get(&format!("__exr_{ty}_{}", p.segments[1].ident)).copied();
                }
                if Ty::from_suffix(&ty).is_some() && matches!(p.segments[1].ident.to_string().as_str(), "max" | "min") {
                    return self.index.get(&format!("__core_{}", p.segments[1].ident)).copied();
                }
                // `char::from_digit(d, 10)` / `char::from_u32(x)` / `char::from(b)`: prelude の `__exr_char_*`
                if ty == "char" {
                    let m = p.segments[1].ident.to_string();
                    let m = match m.as_str() {
                        "from_u32" => "from_u32_checked".to_string(),
                        "from" => "from_u8".to_string(),
                        _ => m,
                    };
                    return self
                        .index
                        .get(&format!("__exr_char_{m}"))
                        .or_else(|| self.index.get(&format!("__exr_charfn_{m}")))
                        .copied();
                }
                let ty = if ty == "Self" {
                    self.structs[self.base_of(self_sid?) as usize].name.clone()
                } else {
                    ty
                };
                self.index
                    .get(&format!("{ty}::{}", p.segments[1].ident))
                    .copied()
            }
            _ => None,
        }
    }

    /// struct `sid` のメソッド `name`
    pub fn method(&self, sid: u32, name: &str) -> Option<u32> {
        self.index
            .get(&format!(
                "{}::{name}",
                self.structs[self.base_of(sid) as usize].name
            ))
            .copied()
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
            Ty::Unit | Ty::Never | Ty::Slice(_) | Ty::Str | Ty::Dyn(_) | Ty::Param(_) => (0, 1),
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
            Ty::Dyn(s) => format!("dyn {}", s.replace(" (", "(").replace("( ", "(").replace(" )", ")")),
            Ty::Box(i) => format!("Box<{}>", self.tn(*i)),
            Ty::Assoc(a) => format!("<{}>::{}", self.tn(a.base), a.name),
            Ty::Fn(f) => {
                let ps: Vec<String> = f.params.iter().map(|&p| self.tn(p)).collect();
                match f.ret {
                    Ty::Unit => format!("fn({})", ps.join(", ")),
                    r => format!("fn({}) -> {}", ps.join(", "), self.tn(r)),
                }
            }
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
                    // (クロージャの struct は同じアイテムに複数あるので名前も入れる)
                    Some(b) => {
                        h.add(&(103u8, self.structs[b as usize].deep, sd.args.len()));
                        if self.closure_of_struct.contains_key(&b) {
                            h.add(&self.structs[b as usize].name);
                        }
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
            // vtable の並び (trait のメソッドの順) もコードに埋まる
            Ty::Dyn(s) => {
                h.add(&(108u8, s));
                for &f in self.traits.get(s).map(|v| &v[..]).unwrap_or(&[]) {
                    h.add(&self.names[f as usize]);
                }
            }
            Ty::Box(i) => {
                h.add(&110u8);
                self.hash_ty(*i, h);
            }
            Ty::Assoc(a) => {
                h.add(&(111u8, a.name.as_str()));
                self.hash_ty(a.base, h);
            }
            Ty::Fn(f) => {
                h.add(&(109u8, f.params.len()));
                for &p in f.params.iter().chain([&f.ret]) {
                    self.hash_ty(p, h);
                }
            }
            t => h.add(&ty_code(t)),
        }
    }

    pub fn hash_sig(&self, id: u32, h: &mut KeyHasher) {
        let s = &self.sigs[id as usize];
        h.add(self.names[id as usize].as_str());
        h.add(&(
            s.symbol.as_str(),
            s.is_extern,
            s.is_main,
            s.has_self,
            s.params.len(),
        ));
        // 大半の関数では空のフィールドは、空でないときだけ入れる (no-op ビルドで全関数分ハッシュするため)
        if !s.generics.is_empty() || s.trait_of.is_some() || !s.has_body || s.impl_trait.is_some() {
            h.add(&(&s.generics, &s.trait_of, s.has_body, &s.impl_trait));
        }
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
    /// 識別子のうち const / static に一致するもの
    pub fn const_refs(&self, src: &str, it: &SrcItem) -> Vec<u32> {
        let mut v: Vec<u32> = it
            .idents
            .iter()
            .flat_map(|&(a, b)| {
                let s = &src[a as usize..b as usize];
                self.const_index
                    .get(s)
                    .copied()
                    .into_iter()
                    .chain(self.const_by_last.get(s).into_iter().flatten().copied())
            })
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// (関数, struct / enum, const / static) の参照。識別子を 1 回だけ走査する
    pub fn refs(&self, src: &str, it: &SrcItem) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
        let mut fns = vec![];
        let mut sts = vec![];
        let mut cs = vec![];
        let has_consts = !self.const_index.is_empty();
        let has_assoc = !self.const_by_last.is_empty();
        for &(a, b) in &it.idents {
            let s = &src[a as usize..b as usize];
            // `for` はイテレータなら名前を書かずに next を呼ぶ
            let s = if s == "for" { "next" } else { s };
            if let Some(ids) = self.by_last.get(s) {
                fns.extend_from_slice(ids);
            }
            if let Some(&i) = self.struct_index.get(s) {
                sts.push(i);
            }
            if has_consts {
                if let Some(&i) = self.const_index.get(s) {
                    cs.push(i);
                }
                if has_assoc {
                    if let Some(ids) = self.const_by_last.get(s) {
                        cs.extend_from_slice(ids);
                    }
                }
            }
        }
        for v in [&mut fns, &mut sts, &mut cs] {
            v.sort_unstable();
            v.dedup();
        }
        (fns, sts, cs)
    }

    /// main から到達可能な関数 ID。
    /// 到達しない関数は parse も型検査もコード生成もしない (relax/blaze の demand-driven の AOT 版)。
    pub fn reachable(&self, refs: &[Vec<u32>]) -> Vec<bool> {
        let mut seen = vec![false; self.names.len()];
        // 根: main・外に見せる関数・panic handler (panic はどこからでも起こりうる)
        let mut stack = vec![self.index["main"]];
        stack.extend((0..self.names.len() as u32).filter(|&i| self.sigs[i as usize].export));
        stack.extend(self.panic_handler);
        // `Box::new` が呼ぶアロケータと drop が呼ぶ free (prelude にあるのは Box / Drop が使われたときだけ)
        stack.extend(self.index.get(crate::prelude::BOX_ALLOC).copied());
        stack.extend(self.index.get(crate::prelude::BOX_FREE).copied());
        // `vec![..]` / `"..".to_string()` は名前を書かずに Vec の realloc / String::from を使う
        stack.extend(self.index.get("__exr_realloc").copied());
        stack.extend(self.index.get("String::from").copied());
        // (`process::exit` は名前を書かずに呼ばれる: あれば根にする。使われなければ要約で選ばれない)
        stack.extend(self.index.get("__core_process_exit").copied());
        // (`fs::read_to_string` / `env::args` などはパスで引くので、名前からは届かない)
        for n in ["__exr_fs_read", "__exr_fs_read_to_string", "__exr_fs_write", "__exr_env_args", "__exr_env_var"] {
            stack.extend(self.index.get(n).copied());
        }
        // const の式から呼ぶ関数 (`const N: u32 = sq(4);`。const は使う場所で式として展開される)
        for c in &self.consts {
            if c.expr.contains('(') {
                for w in c.expr.split(|ch: char| !(ch.is_alphanumeric() || ch == '_')) {
                    stack.extend(self.index.get(w).copied());
                }
            }
        }

        // `impl Drop` の drop は drop glue から、メソッド `eq` は `==` から (名前を書かずに) 呼ばれる
        stack.extend((0..self.names.len() as u32).filter(|&i| {
            let s = &self.sigs[i as usize];
            // (書式化の部品と Display / Debug の impl も、codegen が名前を書かずに呼ぶ)
            matches!(s.impl_trait.as_deref(), Some("Drop"))
                || (s.has_self && self.names[i as usize].ends_with("::eq"))
                // (`write!(w, ..)` は codegen が w.write_str(..) を呼ぶ)
                || (s.has_self && self.names[i as usize].ends_with("::write_str"))
                // (libm などの prelude の extern は codegen が名前を書かずに呼ぶ。宣言だけなのでコードは増えない)
                || (s.is_extern && self.names[i as usize].starts_with("__exr_"))
                // (演算子 `a + b` は codegen が `a.add(b)` にする)
                || (s.has_self
                    && matches!(
                        s.impl_trait.as_deref(),
                        Some(
                            "Add" | "Sub" | "Mul" | "Div" | "Rem" | "Neg" | "Not" | "BitAnd" | "BitOr" | "BitXor"
                                | "Shl" | "Shr" | "AddAssign" | "SubAssign" | "MulAssign" | "DivAssign"
                                | "RemAssign" | "BitAndAssign" | "BitOrAssign" | "BitXorAssign" | "ShlAssign"
                                | "ShrAssign" | "Index" | "IndexMut" | "IntoIterator"
                        )
                    ))
                // (`<` と sort は比較の glue から partial_cmp / cmp を呼ぶ)
                || (s.has_self
                    && matches!(s.impl_trait.as_deref(), Some("PartialOrd" | "Ord"))
                    && (self.names[i as usize].ends_with("::cmp")
                        || self.names[i as usize].ends_with("::partial_cmp")))
                || self.names[i as usize] == "String::new"
        }));
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

const TYS: [Ty; 18] = [
    Ty::I8,
    Ty::I16,
    Ty::I32,
    Ty::I64,
    Ty::Isize,
    Ty::U8,
    Ty::U16,
    Ty::U32,
    Ty::U64,
    Ty::Usize,
    Ty::Bool,
    Ty::Unit,
    Ty::Never,
    Ty::Char,
    Ty::F32,
    Ty::F64,
    Ty::I128,
    Ty::U128,
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
            TySyn::Dyn(n) => {
                out.push(31);
                n.enc(out);
            }
            TySyn::FnPtr(v) => {
                out.push(32);
                v.enc(out);
            }
            TySyn::Assoc(b, n) => {
                out.push(33);
                b.enc(out);
                n.enc(out);
            }
            TySyn::Infer => out.push(34),
        }
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some(match r.u8()? {
            c @ 0..18 => TySyn::Prim(TYS[c as usize]),
            c @ (20 | 21) => TySyn::Ref(c == 21, Box::new(TySyn::dec(r)?)),
            22 => TySyn::Named(String::dec(r)?),
            23 => TySyn::App(String::dec(r)?, Vec::dec(r)?),
            24 => TySyn::Tuple(Vec::dec(r)?),
            25 => TySyn::Array(Box::new(TySyn::dec(r)?), ArrLen::Lit(u64::dec(r)?)),
            26 => TySyn::Array(Box::new(TySyn::dec(r)?), ArrLen::Const(String::dec(r)?)),
            27 => TySyn::Slice(Box::new(TySyn::dec(r)?)),
            28 => TySyn::Str,
            c @ (29 | 30) => TySyn::Ptr(c == 30, Box::new(TySyn::dec(r)?)),
            31 => TySyn::Dyn(String::dec(r)?),
            32 => TySyn::FnPtr(Vec::dec(r)?),
            33 => TySyn::Assoc(Box::new(TySyn::dec(r)?), String::dec(r)?),
            34 => TySyn::Infer,
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
        [VariantKind::Unit, VariantKind::Tuple, VariantKind::Struct]
            .get(r.u8()? as usize)
            .copied()
    }
    fn is_empty_val(&self) -> bool {
        *self == VariantKind::Unit
    }
}

crate::enc_struct!(VariantSyn {
    name,
    kind,
    fields,
    discr
});
crate::enc_struct!(SigSyn {
    params,
    ret,
    symbol,
    is_extern,
    is_main,
    impl_of,
    has_self,
    generics,
    trait_of,
    has_body,
    impl_trait,
    self_ty,
    export,
    panic_handler,
});
crate::enc_struct!(Summary {
    fns,
    structs,
    impl_self,
    enums,
    trait_decl,
    impl_trait,
    derives,
    adt_generics,
    impl_generics,
    impl_self_ty,
    consts,
    statics,
    closures,
    assocs,
    aliases,
});
crate::enc_struct!(AssocSyn {
    name,
    self_ty,
    generics,
    value,
});
crate::enc_struct!(ClosureSyn {
    name,
    generics,
    fields,
    sig,
    by_move,
    ret_ann,
    n_parent,
});

impl Enc for (String, TySyn, String, bool) {
    fn enc(&self, out: &mut Vec<u8>) {
        self.0.enc(out);
        self.1.enc(out);
        self.2.enc(out);
        self.3.enc(out);
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some((
            String::dec(r)?,
            TySyn::dec(r)?,
            String::dec(r)?,
            bool::dec(r)?,
        ))
    }
}

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
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(i),
            ..
        }) => i.base10_parse::<i64>().map_err(|e| e.to_string()),
        syn::Expr::Unary(syn::ExprUnary {
            op: syn::UnOp::Neg(_),
            expr,
            ..
        }) => Ok(-const_int(expr)?),
        syn::Expr::Paren(p) => const_int(&p.expr),
        _ => err(
            e.span(),
            "only integer literals are supported as discriminants",
        ),
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
type Bounds = syn::punctuated::Punctuated<syn::TypeParamBound, syn::Token![+]>;

fn bound_names(bs: &Bounds) -> R<Vec<String>> {
            let mut v = vec![];
            for b in bs {
                match b {
                    // `?Sized` は制約を緩めるだけなので数えない
                    syn::TypeParamBound::Trait(t) if matches!(t.modifier, syn::TraitBoundModifier::Maybe(_)) => {}
                    syn::TypeParamBound::Trait(t)
                        if matches!(
                            t.path.segments.last().map(|s| &s.arguments),
                            Some(syn::PathArguments::Parenthesized(_))
                        ) =>
                    {
                        v.push(bound_string(t))
                    }
                    // `Iterator<Item = u32>`: 関連型の制約つき (文字列のまま持ち、呼び出し側で解く)
                    syn::TypeParamBound::Trait(t)
                        if t.path.segments.len() == 1
                            && matches!(t.path.segments[0].arguments, syn::PathArguments::AngleBracketed(_)) =>
                    {
                        v.push(bound_string(t))
                    }
                    // `fmt::Display` などのパスは最後のセグメント
                    syn::TypeParamBound::Trait(t) => match t.path.segments.last() {
                        Some(seg) if seg.arguments.is_none() => v.push(seg.ident.to_string()),
                        _ => return err(t.span(), "unsupported trait bound"),
                    },
                    syn::TypeParamBound::Lifetime(_) => {}
                    _ => return err(b.span(), "unsupported bound"),
                }
            }
            Ok(v)
        }

fn collect_generics(g: &syn::Generics, out: &mut Vec<(String, Vec<String>)>) -> R<()> {
    for p in &g.params {
        match p {
            syn::GenericParam::Lifetime(_) => {}
            syn::GenericParam::Type(tp) => {
                out.push((tp.ident.to_string(), bound_names(&tp.bounds)?))
            }
            // `const N: usize`: 配列の長さのパラメータ (型の引数の位置に長さを持つ Ty::Array(Unit, n) を入れる)
            syn::GenericParam::Const(c) => out.push((c.ident.to_string(), vec![CONST_GENERIC.to_string()])),
        }
    }
    if let Some(w) = &g.where_clause {
        for pr in &w.predicates {
            let syn::WherePredicate::Type(pt) = pr else {
                continue;
            };
            // (`where C::Item: Debug` / `where Self: Sized` など、型パラメータ以外への境界は検査に使わないので読み飛ばす)
            let syn::Type::Path(tp) = &pt.bounded_ty else {
                continue;
            };
            let Some(id) = tp.path.get_ident() else {
                continue;
            };
            let bs = bound_names(&pt.bounds)?;
            match out.iter_mut().find(|g| *id == g.0) {
                Some(g) => g.1.extend(bs),
                None if id == "Self" => {}
                None => return err(pt.span(), "where clause on an unknown type parameter"),
            }
        }
    }
    Ok(())
}

/// const ジェネリクスの引数 `4` / `{ 4 }` / `N`。値は長さを持つ配列型 `[(); n]` で表す
pub fn const_arg_syn(e: &syn::Expr) -> R<TySyn> {
    match e {
        syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(i), .. }) => {
            let n = i.base10_parse::<u64>().or_else(|er| err(i.span(), er))?;
            Ok(TySyn::Array(Box::new(TySyn::Prim(Ty::Unit)), ArrLen::Lit(n)))
        }
        syn::Expr::Block(b) if b.block.stmts.len() == 1 => match &b.block.stmts[0] {
            syn::Stmt::Expr(e, None) => const_arg_syn(e),
            _ => err(e.span(), "unsupported const argument"),
        },
        syn::Expr::Path(p) if p.path.get_ident().is_some() => {
            Ok(TySyn::Array(Box::new(TySyn::Prim(Ty::Unit)), ArrLen::Const(p.path.get_ident().unwrap().to_string())))
        }
        _ => err(e.span(), "const arguments must be a literal or a name"),
    }
}

/// 型の環境で name が const ジェネリクスの引数 (`[(); n]`) なら、その値
pub fn const_generic_value(env: &TyEnv, name: &str) -> Option<u32> {
    env.params.iter().find(|(n, _)| n == name).and_then(|(_, t)| match t {
        Ty::Array(Ty::Unit, n) if *n < CONST_LEN_BASE => Some(*n),
        _ => None,
    })
}

/// const ジェネリクスの印 (境界の名前の位置に入れる)
pub const CONST_GENERIC: &str = "__const";
/// `[T; N]` の N が型パラメータ i のとき、Ty::Array の長さは CONST_LEN_BASE + i
pub const CONST_LEN_BASE: u32 = 0xFFFF_FF00;

/// owner: impl / trait のメンバならその所有者
/// 戻り値位置の `impl Trait` の仮の型名
pub const IMPL_RET: &str = "impl Trait";
/// クロージャでない `impl Trait` の戻り値 (`-> impl Iterator<Item = u32>`): 本体の型推論で決める
pub const OPAQUE_RET: &str = "impl Trait (opaque)";

fn parse_sig(
    sig: &syn::Signature,
    is_extern: bool,
    is_main: bool,
    owner: Option<&Owner>,
) -> R<SigSyn> {
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
            syn::FnArg::Typed(pt) => {
                // 引数位置の `impl Trait` は名前のない型パラメータ
                let (inner, rf) = match &*pt.ty {
                    syn::Type::Reference(r) => (&*r.elem, Some(r.mutability.is_some())),
                    t => (t, None),
                };
                if let syn::Type::ImplTrait(it) = inner {
                    let g = format!("__impl{}", generics.len());
                    generics.push((g.clone(), bound_names(&it.bounds)?));
                    let t = TySyn::Named(g);
                    params.push(match rf {
                        Some(m) => TySyn::Ref(m, Box::new(t)),
                        None => t,
                    });
                } else {
                    params.push(parse_tysyn(&pt.ty, self_ty)?)
                }
            }
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
        // 戻り値位置の `impl Trait` は本体を見て summarize が具体型に置き換える
        syn::ReturnType::Type(_, t) if matches!(&**t, syn::Type::ImplTrait(_)) => {
            TySyn::Named(IMPL_RET.into())
        }
        syn::ReturnType::Type(_, t) => parse_tysyn(t, self_ty)?,
    };
    let name = sig.ident.to_string();
    let symbol = match owner {
        _ if is_extern || is_main => name,
        // ローカルシンボル。短く保つことで .o / シンボル表を小さくする
        Some(o) => match o.impl_trait.as_deref() {
            Some(t @ ("Display" | "Debug")) => format!("_x{}.{t}.{name}", o.base),
            Some(t) if t.starts_with("PartialEq<") || t.starts_with("PartialOrd<") => {
                let t: String = t.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
                format!("_x{}.{t}.{name}", o.base)
            }
            _ => format!("_x{}.{name}", o.base),
        },
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
        export: false,
        panic_handler: false,
    })
}

/// 型 t が型パラメータ g を値として含むか (参照・ポインタ・Box の先は含まない。ほかの型の引数は含むとみなす)
fn syn_holds_by_value(t: &TySyn, g: &str) -> bool {
    match t {
        TySyn::Named(n) => n == g,
        TySyn::App(n, _) if n == "Box" => false,
        TySyn::App(_, args) => args.iter().any(|a| syn_holds_by_value(a, g)),
        TySyn::Tuple(es) => es.iter().any(|a| syn_holds_by_value(a, g)),
        TySyn::Array(e, _) => syn_holds_by_value(e, g),
        _ => false,
    }
}

/// 本体の直下の `let n: T = ..` / `let mut n = 0;` から、ローカル変数 n の型 (注釈か、リテラル・String の初期化式)
fn local_ty_syn(b: &syn::Block, n: &str, int_hint: Option<Ty>) -> Option<TySyn> {
    let mut found = None;
    for st in &b.stmts {
        let syn::Stmt::Local(l) = st else { continue };
        let (pat, ann) = match &l.pat {
            syn::Pat::Type(pt) => (&*pt.pat, Some(&*pt.ty)),
            p => (p, None),
        };
        let syn::Pat::Ident(pi) = pat else { continue };
        if pi.ident != n {
            continue;
        }
        if let Some(t) = ann {
            found = parse_tysyn(t, None).ok();
            continue;
        }
        let init = l.init.as_ref().map(|i| &*i.expr);
        found = match init {
            Some(syn::Expr::Lit(el)) => match &el.lit {
                syn::Lit::Int(i) => Some(TySyn::Prim(Ty::from_suffix(i.suffix()).or(int_hint).unwrap_or(Ty::I32))),
                syn::Lit::Float(x) => Some(TySyn::Prim(if x.suffix() == "f32" { Ty::F32 } else { Ty::F64 })),
                syn::Lit::Bool(_) => Some(TySyn::Prim(Ty::Bool)),
                syn::Lit::Char(_) => Some(TySyn::Prim(Ty::Char)),
                syn::Lit::Str(_) => Some(TySyn::Ref(false, Box::new(TySyn::Str))),
                _ => None,
            },
            // `String::new()` / `String::from(..)` / `"..".to_string()`
            Some(syn::Expr::Call(c)) => match &*c.func {
                syn::Expr::Path(p) if p.path.segments.len() == 2 && p.path.segments[0].ident == "String" => {
                    Some(TySyn::Named("String".into()))
                }
                _ => None,
            },
            Some(syn::Expr::MethodCall(mc)) if mc.method == "to_string" => Some(TySyn::Named("String".into())),
            _ => None,
        };
    }
    found
}
