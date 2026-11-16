//! 型の表現 (front.rs から分けた、syn に依存しない部分)。`Ty` (解決済みの型) と `TySyn` (構文上の型) と intern。
//! crustc 自身でもコンパイルできる書き方にしてある (セルフホストの対象)。

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

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
    let mut tab = TABLE.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap();
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
    let mut tab = TABLE.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap();
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
    let mut tab = TABLE.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap();
    if let Some(&r) = tab.get(&f) {
        return r;
    }
    let r: &'static FnTy = Box::leak(Box::new(f));
    tab.insert(r);
    r
}

/// trait 境界を文字列にする (パスの最後のセグメントだけ。`core::ops::Fn(u8) -> u8` → `Fn (u8) -> u8`)
pub fn intern(t: Ty) -> &'static Ty {
    static TABLE: OnceLock<Mutex<HashSet<&'static Ty>>> = OnceLock::new();
    let mut tab = TABLE.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap();
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

