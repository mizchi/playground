//! フロントエンド: アイテム要約 (シグネチャ・struct 定義) から型表と参照グラフを作る。
//!
//! ここは「関数本体を見ない」軽い処理だけにしている。本体の型検査・lowering は
//! codegen 側で関数ごとに行い、キャッシュヒットした関数は本体に一切触れない。

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use syn::spanned::Spanned;

use crate::cache::{Key, KeyHasher};
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
        !matches!(self, Ty::Bool | Ty::Unit | Ty::Never | Ty::Ref(..) | Ty::Struct(_) | Ty::Param(_))
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
            Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize | Ty::Ref(..) => 64,
            Ty::Unit | Ty::Never | Ty::Struct(_) | Ty::Param(_) => 0,
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
}

/// `self_name` は impl の中での `Self` の中身
pub fn parse_tysyn(t: &syn::Type, self_name: Option<&str>) -> R<TySyn> {
    Ok(match t {
        syn::Type::Never(_) => TySyn::Prim(Ty::Never),
        syn::Type::Tuple(tt) if tt.elems.is_empty() => TySyn::Prim(Ty::Unit),
        syn::Type::Paren(p) => parse_tysyn(&p.elem, self_name)?,
        syn::Type::Group(g) => parse_tysyn(&g.elem, self_name)?,
        // ライフタイムはコード生成に影響しないので消去する
        syn::Type::Reference(r) => {
            TySyn::Ref(r.mutability.is_some(), Box::new(parse_tysyn(&r.elem, self_name)?))
        }
        syn::Type::Path(p) if p.qself.is_none() => {
            let Some(id) = p.path.get_ident() else {
                return err(t.span(), "unsupported type path");
            };
            let s = id.to_string();
            match Ty::prim(&s) {
                Some(t) => TySyn::Prim(t),
                None if s == "Self" => match self_name {
                    Some(n) => TySyn::Named(n.to_string()),
                    None => return err(t.span(), "`Self` is only available in impls"),
                },
                None => TySyn::Named(s),
            }
        }
        _ => return err(t.span(), "unsupported type"),
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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

pub fn has_param(t: Ty) -> bool {
    match t {
        Ty::Param(_) => true,
        Ty::Ref(_, i) => has_param(*i),
        _ => false,
    }
}

pub fn subst(t: Ty, args: &[Ty]) -> Ty {
    match t {
        Ty::Param(i) => args[i as usize],
        Ty::Ref(m, i) => Ty::Ref(m, intern(subst(*i, args))),
        t => t,
    }
}

/// 仮引数型 pt (型パラメータを含む) と実引数型 at を突き合わせて型パラメータを解く
pub fn match_ty(pt: Ty, at: Ty, sol: &mut [Option<Ty>]) -> bool {
    match (pt, at) {
        (Ty::Param(i), t) => match sol[i as usize] {
            Some(prev) => prev == t,
            None => {
                sol[i as usize] = Some(t);
                true
            }
        },
        (Ty::Ref(_, a), Ty::Ref(_, b)) => match_ty(*a, *b, sol),
        (a, b) => a == b,
    }
}

/// enum のバリアント (要約に保存する形)
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VariantSyn {
    pub name: String,
    pub kind: VariantKind,
    pub fields: Vec<(String, TySyn)>,
    pub discr: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VariantKind {
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
    pub structs: Vec<StructDef>,
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
            b.impl_of
        };
        let sig = FnSig {
            params: b.params.iter().map(|&t| subst(t, &args)).collect(),
            ret: subst(b.ret, &args),
            symbol: format!("{}<{}>", b.symbol, names.join(",")),
            is_extern: false,
            is_main: false,
            impl_of,
            has_self: b.has_self,
            generics: vec![],
            trait_of: None,
            has_body: true,
            impl_trait: None,
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
pub fn parse_and_summarize(src: &str, it: &SrcItem, owner: Option<(&str, Option<&str>)>) -> R<(Summary, Option<syn::Item>)> {
    if let ItemKind::TraitMember(_) = it.kind {
        let (tr, _) = owner.ok_or("trait member without trait")?;
        let f = parse_trait_member(src, it)?;
        let mut sig = parse_sig(&f.sig, false, false, Some("Self"), Some(tr), None).map_err(|e| relocate_err(e, it))?;
        sig.has_body = f.default.is_some();
        let sm = Summary { fns: vec![(format!("{tr}::{}", f.sig.ident), sig)], ..Default::default() };
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

/// "L:C: msg" 形式のエラーの行をアイテム開始行だけずらす。
pub fn relocate_err(e: String, it: &SrcItem) -> String {
    if let Some((l, rest)) = e.split_once(':') {
        if let Ok(l) = l.parse::<usize>() {
            return format!("{}:{rest}", l + it.line - 1);
        }
    }
    e
}

/// `owner` は impl のメンバのとき (self 型, 実装している trait)
pub fn summarize(item: &syn::Item, owner: Option<(&str, Option<&str>)>) -> R<Summary> {
    let mut sm = Summary::default();
    match item {
        syn::Item::Fn(f) if owner.is_some() => {
            let (sn, tr) = owner.unwrap();
            let sig = parse_sig(&f.sig, false, false, Some(sn), None, tr)?;
            sm.fns.push((format!("{sn}::{}", f.sig.ident), sig));
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
            only_lifetimes(&im.generics)?;
            let syn::Type::Path(tp) = &*im.self_ty else {
                return err(im.self_ty.span(), "unsupported impl self type");
            };
            let Some(id) = tp.path.get_ident() else {
                return err(im.self_ty.span(), "unsupported impl self type");
            };
            sm.impl_self = Some(id.to_string());
        }
        syn::Item::Fn(f) => {
            let name = f.sig.ident.to_string();
            let is_main = name == "main";
            let sig = parse_sig(&f.sig, false, is_main, None, None, None)?;
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
                sm.fns.push((f.sig.ident.to_string(), parse_sig(&f.sig, true, false, None, None, None)?));
            }
        }
        syn::Item::Enum(e) => {
            only_lifetimes(&e.generics)?;
            let sn = e.ident.to_string();
            let mut vs = vec![];
            for v in &e.variants {
                let (kind, fields) = match &v.fields {
                    syn::Fields::Unit => (VariantKind::Unit, vec![]),
                    syn::Fields::Unnamed(u) => {
                        let mut fs = vec![];
                        for (i, f) in u.unnamed.iter().enumerate() {
                            fs.push((i.to_string(), parse_tysyn(&f.ty, Some(&sn))?));
                        }
                        (VariantKind::Tuple, fs)
                    }
                    syn::Fields::Named(nf) => {
                        let mut fs = vec![];
                        for f in &nf.named {
                            fs.push((f.ident.as_ref().unwrap().to_string(), parse_tysyn(&f.ty, Some(&sn))?));
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
            only_lifetimes(&s.generics)?;
            let syn::Fields::Named(nf) = &s.fields else {
                return err(s.span(), "only structs with named fields are supported");
            };
            let mut fields = vec![];
            for f in &nf.named {
                let t = parse_tysyn(&f.ty, Some(&s.ident.to_string()))?;
                fields.push((f.ident.as_ref().unwrap().to_string(), t));
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

impl Program {
    pub fn build(items: &[SrcItem], sums: &[Summary]) -> R<Program> {
        let mut p = Program {
            names: vec![],
            index: HashMap::new(),
            sigs: vec![],
            item_of: vec![],
            structs: vec![],
            struct_index: HashMap::new(),
            by_last: HashMap::new(),
            traits: HashMap::new(),
            impls: HashSet::new(),
        };
        // 1. struct / enum 名を登録
        let mut sdefs: Vec<(usize, String, AdtShape)> = vec![];
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
                sdefs.push((ii, name.clone(), shape));
            }
        }
        // 2. フィールド型を解決し、依存順にレイアウトを決める
        p.structs = sdefs
            .iter()
            .map(|(_, n, sh)| StructDef {
                name: n.clone(),
                fields: vec![],
                size: 0,
                align: 1,
                deep: 0,
                variants: vec![],
                is_enum: matches!(sh, AdtShape::Enum(_)),
                tag: Ty::U8,
            })
            .collect();
        let mut state = vec![0u8; sdefs.len()]; // 0 未処理, 1 処理中, 2 済み
        for id in 0..sdefs.len() {
            p.layout(id, &sdefs, items, &mut state)?;
        }
        // deep hash = 値・参照を問わずフィールドから推移的に届く全 struct の定義ハッシュ。
        // 参照経由 (`&'a Other`) でもフィールドオフセットはコードに埋まるので含める。
        let deeps: Vec<Key> = (0..sdefs.len())
            .map(|id| {
                let mut seen = vec![false; sdefs.len()];
                let mut stack = vec![id];
                let mut hs = vec![];
                while let Some(i) = stack.pop() {
                    if std::mem::replace(&mut seen[i], true) {
                        continue;
                    }
                    hs.push(items[sdefs[i].0].hash);
                    let sd = &p.structs[i];
                    let vf = sd.variants.iter().flat_map(|v| v.fields.iter());
                    for &(_, mut t, _) in sd.fields.iter().chain(vf) {
                        while let Ty::Ref(_, inner) = t {
                            t = *inner;
                        }
                        if let Ty::Struct(sid) = t {
                            stack.push(sid as usize);
                        }
                    }
                }
                hs.sort_unstable();
                let mut h = KeyHasher::new();
                h.add(&hs);
                h.finish()
            })
            .collect();
        for (sd, d) in p.structs.iter_mut().zip(deeps) {
            sd.deep = d;
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
                let impl_of = match &ss.impl_of {
                    Some(n) => match p.struct_index.get(n) {
                        Some(&sid) => Some(sid),
                        None => return Err(format!("{}:1: cannot find struct `{n}` for impl", it.line)),
                    },
                    None => None,
                };
                if ss.is_extern && params.iter().chain([&ret]).any(|t| matches!(t, Ty::Struct(_))) {
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
            Ty::Struct(sid) => self.impls.contains(&(sid, tr.to_string())),
            // 組み込み型はマーカー trait だけ
            Ty::Ref(true, _) => tr == "Sized",
            _ => matches!(tr, "Copy" | "Clone" | "Sized"),
        }
    }

    /// struct sid に名前 name のメソッドが見つからないとき、実装している trait のデフォルトメソッド
    pub fn default_method(&self, sid: u32, name: &str) -> Option<u32> {
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
        Ok(match t {
            TySyn::Named(n) => match generics.iter().position(|g| g == n) {
                Some(i) => Ty::Param(i as u8),
                None => self.resolve(t)?,
            },
            TySyn::Ref(m, inner) => Ty::Ref(*m, intern(self.resolve_g(inner, generics)?)),
            t => self.resolve(t)?,
        })
    }

    /// Ty → TySyn (キャッシュにインスタンスを名前で書くため)
    pub fn ty_syn(&self, t: Ty) -> TySyn {
        match t {
            Ty::Ref(m, i) => TySyn::Ref(m, Box::new(self.ty_syn(*i))),
            Ty::Struct(id) => TySyn::Named(self.structs[id as usize].name.clone()),
            t => TySyn::Prim(t),
        }
    }

    fn layout(
        &mut self,
        id: usize,
        sdefs: &[(usize, String, AdtShape)],
        items: &[SrcItem],
        state: &mut [u8],
    ) -> R<()> {
        match state[id] {
            2 => return Ok(()),
            1 => return Err(format!("recursive type `{}` has infinite size", sdefs[id].1)),
            _ => {}
        }
        state[id] = 1;
        let (ii, _, shape) = &sdefs[id];
        let line = items[*ii].line;
        // フィールド列を (先頭 0 からの) 構造体として並べる
        let mut lay = |this: &mut Program, fields: &[(String, TySyn)]| -> R<(Vec<(String, Ty, u32)>, u32, u32)> {
            let mut out = vec![];
            let (mut off, mut align) = (0u32, 1u32);
            for (fname, ft) in fields {
                let t = this.resolve(ft).map_err(|e| format!("{line}:1: {e}"))?;
                if let Ty::Struct(sid) = t {
                    this.layout(sid as usize, sdefs, items, state)?;
                }
                let (sz, al) = this.size_align(t);
                off = off.next_multiple_of(al);
                out.push((fname.clone(), t, off));
                off += sz;
                align = align.max(al);
            }
            Ok((out, off, align))
        };
        match shape {
            AdtShape::Struct(fields) => {
                let (out, size, align) = lay(self, fields)?;
                let sd = &mut self.structs[id];
                sd.fields = out;
                sd.size = size.next_multiple_of(align);
                sd.align = align;
            }
            AdtShape::Enum(vs) => {
                // タグ (判別値) は先頭、ペイロードは全バリアントで共有する領域に置く
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
                let tag_size = tag.bits() / 8;
                let mut laid = vec![];
                let (mut pmax, mut palign) = (0u32, 1u32);
                for v in vs {
                    let (f, sz, al) = lay(self, &v.fields)?;
                    pmax = pmax.max(sz);
                    palign = palign.max(al);
                    laid.push(f);
                }
                let poff = tag_size.next_multiple_of(palign);
                let align = palign.max(tag_size);
                let sd = &mut self.structs[id];
                sd.tag = tag;
                sd.variants = vs
                    .iter()
                    .zip(laid)
                    .zip(discrs)
                    .map(|((v, f), d)| VariantDef {
                        name: v.name.clone(),
                        kind: v.kind,
                        fields: f.into_iter().map(|(n, t, o)| (n, t, o + poff)).collect(),
                        discr: d,
                    })
                    .collect();
                sd.size = (poff + pmax).next_multiple_of(align);
                sd.align = align;
            }
        }
        state[id] = 2;
        Ok(())
    }

    /// `E::V` / `Self::V` → (enum id, バリアント番号)
    pub fn variant_by_path(&self, p: &syn::Path, self_sid: Option<u32>) -> Option<(u32, usize)> {
        if p.segments.len() != 2 || p.segments.iter().any(|s| !s.arguments.is_none()) {
            return None;
        }
        let tn = p.segments[0].ident.to_string();
        let sid = if tn == "Self" { self_sid? } else { *self.struct_index.get(&tn)? };
        let sd = &self.structs[sid as usize];
        if !sd.is_enum {
            return None;
        }
        let vn = p.segments[1].ident.to_string();
        sd.variants.iter().position(|v| v.name == vn).map(|vi| (sid, vi))
    }

    pub fn resolve(&self, t: &TySyn) -> R<Ty> {
        Ok(match t {
            TySyn::Prim(t) => *t,
            TySyn::Ref(m, inner) => Ty::Ref(*m, intern(self.resolve(inner)?)),
            TySyn::Named(n) => match self.struct_index.get(n) {
                Some(&id) => Ty::Struct(id),
                None => return Err(format!("cannot find type `{n}`")),
            },
        })
    }

    /// 関数本体の中の型を解決する (`Self` と型パラメータは env から)
    pub fn parse_ty(&self, t: &syn::Type, env: &TyEnv) -> R<Ty> {
        let sn = env.self_sid.map(|s| self.structs[s as usize].name.as_str());
        let ts = parse_tysyn(t, sn)?;
        self.resolve_env(&ts, env).or_else(|e| err(t.span(), e))
    }

    fn resolve_env(&self, t: &TySyn, env: &TyEnv) -> R<Ty> {
        Ok(match t {
            TySyn::Named(n) => match env.params.iter().find(|p| &p.0 == n) {
                Some(&(_, ty)) => ty,
                None => self.resolve(t)?,
            },
            TySyn::Ref(m, inner) => Ty::Ref(*m, intern(self.resolve_env(inner, env)?)),
            t => self.resolve(t)?,
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
                    self.structs[self_sid? as usize].name.clone()
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
        self.index.get(&format!("{}::{name}", self.structs[sid as usize].name)).copied()
    }

    pub fn size_align(&self, t: Ty) -> (u32, u32) {
        match t {
            Ty::Struct(id) => {
                let s = &self.structs[id as usize];
                (s.size, s.align)
            }
            Ty::Unit | Ty::Never => (0, 1),
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
            Ty::Struct(id) => h.add(&(101u8, self.structs[id as usize].deep)),
            Ty::Param(i) => h.add(&(102u8, i)),
            t => h.add(&ty_code(t)),
        }
    }

    pub fn hash_sig(&self, id: u32, h: &mut KeyHasher) {
        let s = &self.sigs[id as usize];
        h.add(self.names[id as usize].as_str());
        h.add(&(s.symbol.as_str(), s.is_extern, s.is_main, s.has_self, s.params.len()));
        h.add(&(&s.generics, &s.trait_of, s.has_body, &s.impl_trait));
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

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.push(s.len() as u8);
    out.extend_from_slice(s.as_bytes());
}

pub fn put_ty(out: &mut Vec<u8>, t: &TySyn) {
    match t {
        TySyn::Prim(t) => out.push(ty_code(*t)),
        TySyn::Ref(m, i) => {
            out.push(13 + *m as u8);
            put_ty(out, i);
        }
        TySyn::Named(n) => {
            out.push(15);
            put_str(out, n);
        }
    }
}

/// d[*pos..] から TySyn を 1 つ読む
pub fn read_ty(d: &[u8], pos: &mut usize) -> Option<TySyn> {
    let mut r = Rd(d, *pos);
    let t = r.ty();
    *pos = r.1;
    t
}

struct Rd<'a>(&'a [u8], usize);
impl Rd<'_> {
    fn u8(&mut self) -> Option<u8> {
        let b = *self.0.get(self.1)?;
        self.1 += 1;
        Some(b)
    }
    fn str(&mut self) -> Option<String> {
        let l = self.u8()? as usize;
        let s = self.0.get(self.1..self.1 + l)?;
        self.1 += l;
        String::from_utf8(s.to_vec()).ok()
    }
    fn ty(&mut self) -> Option<TySyn> {
        Some(match self.u8()? {
            c @ 0..13 => TySyn::Prim(TYS[c as usize]),
            c @ (13 | 14) => TySyn::Ref(c == 14, Box::new(self.ty()?)),
            15 => TySyn::Named(self.str()?),
            _ => return None,
        })
    }
}

impl Summary {
    /// 先頭 1 byte のフラグで、空の節は書かない (要約は全アイテム分メモリ・ディスクに載るので小さく保つ)
    pub fn encode(&self, out: &mut Vec<u8>) {
        let flags = !self.structs.is_empty() as u8
            | (self.impl_self.is_some() as u8) << 1
            | (!self.enums.is_empty() as u8) << 2
            | (self.trait_decl.is_some() as u8) << 3
            | (self.impl_trait.is_some() as u8) << 4
            | (!self.derives.is_empty() as u8) << 5;
        out.push(flags);
        out.push(self.fns.len() as u8);
        for (name, s) in &self.fns {
            put_str(out, name);
            put_str(out, &s.symbol);
            out.push(
                s.is_extern as u8
                    | (s.is_main as u8) << 1
                    | (s.has_self as u8) << 2
                    | (!s.generics.is_empty() as u8) << 3
                    | (s.trait_of.is_some() as u8) << 4
                    | (s.has_body as u8) << 5
                    | (s.impl_trait.is_some() as u8) << 6,
            );
            put_str(out, s.impl_of.as_deref().unwrap_or(""));
            if !s.generics.is_empty() {
                out.push(s.generics.len() as u8);
                for (g, bs) in &s.generics {
                    put_str(out, g);
                    out.push(bs.len() as u8);
                    for b in bs {
                        put_str(out, b);
                    }
                }
            }
            for x in [&s.trait_of, &s.impl_trait].into_iter().flatten() {
                put_str(out, x);
            }
            put_ty(out, &s.ret);
            out.push(s.params.len() as u8);
            for p in &s.params {
                put_ty(out, p);
            }
        }
        if let Some(n) = &self.impl_self {
            put_str(out, n);
        }
        for x in [&self.trait_decl, &self.impl_trait].into_iter().flatten() {
            put_str(out, x);
        }
        if !self.derives.is_empty() {
            out.push(self.derives.len() as u8);
            for (t, ds) in &self.derives {
                put_str(out, t);
                out.push(ds.len() as u8);
                for d in ds {
                    put_str(out, d);
                }
            }
        }
        if flags & 4 != 0 {
            out.push(self.enums.len() as u8);
        }
        for (name, vs) in &self.enums {
            put_str(out, name);
            out.push(vs.len() as u8);
            for v in vs {
                put_str(out, &v.name);
                out.push(v.kind as u8 | (v.discr.is_some() as u8) << 4);
                if let Some(d) = v.discr {
                    out.extend_from_slice(&d.to_le_bytes());
                }
                out.push(v.fields.len() as u8);
                for (f, t) in &v.fields {
                    put_str(out, f);
                    put_ty(out, t);
                }
            }
        }
        if flags & 1 != 0 {
            out.push(self.structs.len() as u8);
        }
        for (name, fields) in &self.structs {
            put_str(out, name);
            out.push(fields.len() as u8);
            for (f, t) in fields {
                put_str(out, f);
                put_ty(out, t);
            }
        }
    }

    pub fn decode(d: &[u8]) -> Option<Summary> {
        let mut r = Rd(d, 0);
        let mut sm = Summary::default();
        let flags = r.u8()?;
        for _ in 0..r.u8()? {
            let name = r.str()?;
            let symbol = r.str()?;
            let fl = r.u8()?;
            let impl_of = Some(r.str()?).filter(|s| !s.is_empty());
            let mut generics = vec![];
            if fl & 8 != 0 {
                for _ in 0..r.u8()? {
                    let g = r.str()?;
                    let bs = (0..r.u8()?).map(|_| r.str()).collect::<Option<Vec<_>>>()?;
                    generics.push((g, bs));
                }
            }
            let trait_of = if fl & 16 != 0 { Some(r.str()?) } else { None };
            let impl_trait = if fl & 64 != 0 { Some(r.str()?) } else { None };
            let ret = r.ty()?;
            let params = (0..r.u8()?).map(|_| r.ty()).collect::<Option<_>>()?;
            sm.fns.push((
                name,
                SigSyn {
                    params,
                    ret,
                    symbol,
                    is_extern: fl & 1 != 0,
                    is_main: fl & 2 != 0,
                    impl_of,
                    has_self: fl & 4 != 0,
                    generics,
                    trait_of,
                    has_body: fl & 32 != 0,
                    impl_trait,
                },
            ));
        }
        if flags & 2 != 0 {
            sm.impl_self = Some(r.str()?);
        }
        if flags & 8 != 0 {
            sm.trait_decl = Some(r.str()?);
        }
        if flags & 16 != 0 {
            sm.impl_trait = Some(r.str()?);
        }
        if flags & 32 != 0 {
            for _ in 0..r.u8()? {
                let t = r.str()?;
                let ds = (0..r.u8()?).map(|_| r.str()).collect::<Option<Vec<_>>>()?;
                sm.derives.push((t, ds));
            }
        }
        let ne = if flags & 4 != 0 { r.u8()? } else { 0 };
        for _ in 0..ne {
            let name = r.str()?;
            let mut vs = vec![];
            for _ in 0..r.u8()? {
                let vname = r.str()?;
                let k = r.u8()?;
                let kind = [VariantKind::Unit, VariantKind::Tuple, VariantKind::Struct].get((k & 15) as usize).copied()?;
                let discr = if k & 16 != 0 {
                    let mut b = [0u8; 8];
                    for x in &mut b {
                        *x = r.u8()?;
                    }
                    Some(i64::from_le_bytes(b))
                } else {
                    None
                };
                let fields = (0..r.u8()?).map(|_| Some((r.str()?, r.ty()?))).collect::<Option<_>>()?;
                vs.push(VariantSyn { name: vname, kind, fields, discr });
            }
            sm.enums.push((name, vs));
        }
        let ns = if flags & 1 != 0 { r.u8()? } else { 0 };
        for _ in 0..ns {
            let name = r.str()?;
            let fields = (0..r.u8()?).map(|_| Some((r.str()?, r.ty()?))).collect::<Option<_>>()?;
            sm.structs.push((name, fields));
        }
        Some(sm)
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

/// self_name: impl の self 型 (trait の中なら "Self")。trait_of: trait の中の宣言。impl_trait: `impl Tr for S` の中
fn parse_sig(
    sig: &syn::Signature,
    is_extern: bool,
    is_main: bool,
    self_name: Option<&str>,
    trait_of: Option<&str>,
    impl_trait: Option<&str>,
) -> R<SigSyn> {
    let mut generics = vec![];
    if let Some(tr) = trait_of {
        generics.push(("Self".to_string(), vec![tr.to_string()]));
    }
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
    let mut params = vec![];
    let mut has_self = false;
    for a in &sig.inputs {
        match a {
            syn::FnArg::Typed(pt) => params.push(parse_tysyn(&pt.ty, self_name)?),
            // syn 2 では `self` / `&self` / `&mut self` / `self: &Self` すべて ty に完全な型が入る
            syn::FnArg::Receiver(r) => {
                if self_name.is_none() {
                    return err(r.span(), "`self` is only allowed in impls");
                }
                has_self = true;
                params.push(parse_tysyn(&r.ty, self_name)?);
            }
        }
    }
    let ret = match &sig.output {
        syn::ReturnType::Default => TySyn::Prim(Ty::Unit),
        syn::ReturnType::Type(_, t) => parse_tysyn(t, self_name)?,
    };
    let name = sig.ident.to_string();
    let symbol = match (trait_of, self_name) {
        _ if is_extern || is_main => name,
        // ローカルシンボル。短く保つことで .o / シンボル表を小さくする
        (Some(tr), _) => format!("_x{tr}.{name}"),
        (None, Some(sn)) => format!("_x{sn}.{name}"),
        (None, None) => format!("_x{name}"),
    };
    Ok(SigSyn {
        params,
        ret,
        symbol,
        is_extern,
        is_main,
        impl_of: if trait_of.is_some() { None } else { self_name.map(String::from) },
        has_self,
        generics,
        trait_of: trait_of.map(String::from),
        has_body: true,
        impl_trait: impl_trait.map(String::from),
    })
}
