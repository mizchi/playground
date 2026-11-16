//! プログラム全体の表 (関数シグネチャ・struct / enum の定義とレイアウト・trait 実装など)。
//! アイテム要約 (`Summary`) から作る。syn に依存しない (crustc 自身でもコンパイルできる) 部分で、
//! syn の構文木を引く検索 (`fn_by_path` など) は front.rs の別の `impl Program` にある。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use crate::fxhash::FxMap;
use crate::lexer::SrcItem;
use crate::types::*;
use crate::key::{Key, KeyHasher};
use crate::ast_summary::parse_fn_bound;

pub type R<T> = Result<T, String>;


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
    /// 同じ型への別々の trait impl が同じ名前の関数を持つもの (`impl From<&str> for P` と `impl From<String> for P`):
    /// 最初の名前 → 候補の関数 (2 つ目以降は `P::from#1` のような名前)。呼び出し側が引数の型で選ぶ
    pub overloads: FxMap<String, Vec<u32>>,
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
    /// キャプチャごとに値で持つか (残りは参照で持つ)
    pub moved: Vec<bool>,
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


/// struct / enum 定義の中での `Self`
pub fn self_of(name: &str, generics: &[String]) -> TySyn {
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
            overloads: Default::default(),
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
        // (同じ名前の struct / enum があればそちら: ユーザの `struct Path` と prelude の `type Path = str`)
        let adt_names: std::collections::HashSet<&str> = sums
            .iter()
            .flat_map(|sm| sm.structs.iter().map(|x| x.0.as_str()).chain(sm.enums.iter().map(|x| x.0.as_str())))
            .collect();
        for sm in sums {
            for (n, g, t) in &sm.aliases {
                if !adt_names.contains(n.as_str()) {
                    p.aliases.insert(n.clone(), (g.clone(), t.clone()));
                }
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
                // (相互再帰の型 (`Ty` ↔ `FnTy`) は届く定義の集合が同じなので、自分の名前も入れて区別する)
                if id < n_defs {
                    h.add(&sdefs[id].1);
                }
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
                let mut sym_suffix = String::new();
                let name = &if let Some(&prev) = p.index.get(name) {
                    if ss.impl_trait.is_none() || p.sigs[prev as usize].impl_trait.is_none() {
                        return Err(format!("{}:1: duplicate definition of `{name}`", it.line));
                    }
                    let k = p.overloads.entry(name.clone()).or_insert_with(|| vec![prev]);
                    k.push(p.names.len() as u32);
                    sym_suffix = format!("#{}", k.len() - 1);
                    format!("{name}{sym_suffix}")
                } else {
                    name.clone()
                };
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
                // (同名の trait impl の関数 `P::from#1` も `from` で引く)
                let last = name.rsplit("::").next().unwrap().split('#').next().unwrap().to_string();
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
                    symbol: format!("{}{sym_suffix}", ss.symbol),
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
                        moved: c.moved.clone(),
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
            // (型引数を取る trait は別々の引数で何度も実装できる: `impl From<&str> for P` と `impl From<String> for P`)
            if !p.impls.insert((sid, tr.clone()))
                && !matches!(
                    tr.as_str(),
                    "From" | "TryFrom" | "Add" | "Sub" | "Mul" | "Div" | "Rem" | "BitAnd" | "BitOr" | "BitXor" | "Shl" | "Shr"
                        | "AddAssign" | "SubAssign" | "MulAssign" | "DivAssign" | "Extend" | "FromIterator" | "AsRef"
                )
            {
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
            Ty::Str => {
                matches!(tr, "Display" | "Debug" | "PartialEq" | "Eq" | "Hash" | "PartialOrd" | "Ord")
                    || self.prim_impls.contains(&(Ty::Str, tr.to_string()))
            }
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
            // `Cow<'a, [T]>` は prelude の `Cow<T>` (借りたスライスか Vec)、`Cow<'a, str>` は `__CowStr`
            TySyn::App(n, args) if n == "Cow" && args.len() == 1 && matches!(args[0], TySyn::Slice(_) | TySyn::Str) => {
                match &args[0] {
                    TySyn::Slice(e) => {
                        let Some(&id) = self.struct_index.get("Cow") else {
                            return Err("cannot find type `Cow`".into());
                        };
                        let e = r(e)?;
                        Ty::Struct(self.instantiate(id, vec![e])?)
                    }
                    _ => match self.struct_index.get("__CowStr") {
                        Some(&id) => Ty::Struct(id),
                        None => return Err("cannot find type `Cow`".into()),
                    },
                }
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
        if v < 0 || v > u64::MAX as i128 {
            return Err(format!("constant `{name}` is negative"));
        }
        Ok(v as u64)
    }

    /// 整数の const を評価する (リテラル・四則演算・ビット演算・シフト・キャスト・他の const)
    pub fn const_int(&self, name: &str, depth: u32) -> R<i128> {
        if depth > 64 {
            return Err(format!("cycle while evaluating constant `{name}`"));
        }
        let Some(&ci) = self.const_index.get(name) else {
            return Err(format!("cannot find constant `{name}`"));
        };
        let e = crate::parse::parse_expr(&self.consts[ci as usize].expr)?;
        self.eval_const(&e, depth + 1)
    }

    /// 自前の構文木の整数の定数式を評価する
    pub fn eval_const(&self, e: &crate::parse::Expr, depth: u32) -> R<i128> {
        use crate::parse::Expr;
        Ok(match e {
            Expr::Lit(t) => lit_int(t).ok_or("not an integer constant")?,
            Expr::Paren(x) => self.eval_const(x, depth)?,
            Expr::Cast(x, _) => self.eval_const(x, depth)?,
            Expr::Unary(op, x) => {
                let v = self.eval_const(x, depth)?;
                match op.as_str() {
                    "-" => -v,
                    "!" => !v,
                    _ => return Err("not an integer constant".into()),
                }
            }
            Expr::Binary(op, l, r) => {
                let (l, r) = (self.eval_const(l, depth)?, self.eval_const(r, depth)?);
                match op.as_str() {
                    "+" => l + r,
                    "-" => l - r,
                    "*" => l * r,
                    "/" if r != 0 => l / r,
                    "%" if r != 0 => l % r,
                    "<<" => l << r,
                    ">>" => l >> r,
                    "&" => l & r,
                    "|" => l | r,
                    "^" => l ^ r,
                    _ => return Err("unsupported operator in constant".into()),
                }
            }
            Expr::Path(None, p) if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() => {
                self.const_int(&p.segs[0].name, depth)?
            }
            Expr::Path(None, p) if p.segs.len() == 2 => {
                let t = Ty::prim(&p.segs[0].name).ok_or("unsupported path in constant")?;
                prim_assoc_const(t, &p.segs[1].name).ok_or("unsupported constant")?
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
            // (ジェネリックでない定義も、フィールドのインスタンスの大きさが変わっていれば取り直す:
            //  `enum ItemKind { Const(String, Option<Ty>, ..) }` の Option<Ty> が Ty より先に作られた場合)
            for id in 0..self.structs.len() {
                let b = &self.structs[id];
                if b.base.is_some() || !b.generics.is_empty() || b.name.starts_with('#') {
                    continue;
                }
                if b.is_enum {
                    let vs: Vec<(String, VariantKind, Vec<(String, Ty)>, i64)> = b
                        .variants
                        .iter()
                        .map(|v| (v.name.clone(), v.kind, v.fields.iter().map(|(n, t, _)| (n.clone(), *t)).collect::<Vec<(String, Ty)>>(), v.discr))
                        .collect();
                    let tag = b.tag;
                    let (vs, size, align) = self.enum_layout(vs, tag);
                    let d = self.structs.get_mut(id);
                    let moved = d.variants.iter().zip(&vs).any(|(a, b)| a.fields.iter().zip(&b.fields).any(|(x, y)| x.2 != y.2));
                    if d.size != size || d.align != align || moved {
                        changed = true;
                    }
                    d.variants = vs;
                    d.size = size;
                    d.align = align;
                } else {
                    let fs: Vec<(String, Ty)> = b.fields.iter().map(|(n, t, _)| (n.clone(), *t)).collect();
                    let (fs, size, align) = self.struct_layout(fs);
                    let d = self.structs.get_mut(id);
                    let moved = d.fields.iter().zip(&fs).any(|(x, y)| x.2 != y.2);
                    if d.size != size || d.align != align || moved {
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

    pub fn resolve(&self, t: &TySyn) -> R<Ty> {
        self.resolve_with(t, &|_| None)
    }

    pub fn resolve_env(&self, t: &TySyn, env: &TyEnv) -> R<Ty> {
        self.resolve_with(t, &|n| {
            if n == "Self" {
                return env.self_ty.or(env.self_sid.map(Ty::Struct));
            }
            env.params.iter().find(|p| p.0 == n).map(|p| p.1)
        })
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
                // (const ジェネリクスの長さの仮の値 (CONST_LEN_BASE + i) は大きいので折り返す: 使われない)
                let (s, a) = self.size_align(*e);
                (s.wrapping_mul(n), a)
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
        for n in [
            "__exr_fs_read", "__exr_fs_read_to_string", "__exr_fs_write", "__exr_env_args", "__exr_env_var",
            "__exr_env_var_os", "__exr_env_split_paths", "__exr_fs_remove_file", "__exr_path_new", "__exr_ptr_eq",
        ] {
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
                                | "ShrAssign" | "Index" | "IndexMut" | "IntoIterator" | "Deref" | "DerefMut"
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

/// 整数・bool・文字・バイトのリテラル (`0x1F_u8` / `true` / `'a'` / `b'a'`)
fn lit_int(t: &str) -> Option<i128> {
    if t == "true" {
        return Some(1);
    }
    if t == "false" {
        return Some(0);
    }
    if t.starts_with('\'') || t.starts_with("b'") {
        let s = t.trim_start_matches('b');
        let inner = &s[1..s.len() - 1];
        return char_lit(inner).map(|c| c as i128);
    }
    if !t.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let s: String = t.chars().filter(|&c| c != '_').collect();
    let (radix, body) = if let Some(h) = s.strip_prefix("0x") {
        (16, h)
    } else if let Some(b) = s.strip_prefix("0b") {
        (2, b)
    } else if let Some(o) = s.strip_prefix("0o") {
        (8, o)
    } else {
        (10, s.as_str())
    };
    let digits: String = body.chars().take_while(|c| c.is_digit(radix)).collect();
    if digits.len() != body.len() && body[digits.len()..].starts_with(|c: char| c == '.' || c == 'e' || c == 'E') && radix == 10 {
        return None;
    }
    if digits.is_empty() {
        return None;
    }
    let mut v: u128 = 0;
    for c in digits.chars() {
        v = v.checked_mul(radix as u128)?.checked_add(c.to_digit(radix)? as u128)?;
    }
    Some(v as i128)
}

fn char_lit(s: &str) -> Option<char> {
    let mut cs = s.chars();
    let c = cs.next()?;
    if c != '\\' {
        return Some(c);
    }
    Some(match cs.next()? {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        '0' => '\0',
        '\\' => '\\',
        '\'' => '\'',
        '"' => '"',
        'x' => {
            let h: String = cs.take(2).collect();
            u8::from_str_radix(&h, 16).ok()? as char
        }
        'u' => {
            let h: String = cs.filter(|c| c.is_ascii_hexdigit()).collect();
            char::from_u32(u32::from_str_radix(&h, 16).ok()?)?
        }
        _ => return None,
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

pub const TYS: [Ty; 18] = [
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

pub fn ty_code(t: Ty) -> u8 {
    TYS.iter().position(|&x| x == t).unwrap() as u8
}

/// パスの 1 セグメント (名前と、型引数 `<..>` / `(..)` があるか)。syn のパスも自前の構文木のパスもこれにして引く
#[derive(Clone, Debug, PartialEq)]
pub struct PSeg {
    pub name: String,
    pub args: bool,
}

/// 名前のパス (`::a::b` の先頭の `::` と、セグメント)
#[derive(Clone, Debug, PartialEq)]
pub struct PPath {
    pub global: bool,
    pub segs: Vec<PSeg>,
}

impl PPath {
    /// 自前の構文木のパス
    pub fn of(p: &crate::parse::Path) -> PPath {
        PPath {
            global: p.global,
            segs: p.segs.iter().map(|s| PSeg { name: s.name.clone(), args: !s.args.is_empty() || s.fn_args.is_some() }).collect(),
        }
    }
    fn tail(&self, k: usize) -> PPath {
        PPath { global: false, segs: self.segs[k..].to_vec() }
    }
    fn ident(&self) -> Option<&str> {
        if !self.global && self.segs.len() == 1 && !self.segs[0].args {
            Some(&self.segs[0].name)
        } else {
            None
        }
    }
}

// --- パスで struct / enum のバリアント / 関数を引く (front.rs の syn 版はこれを呼ぶ) ---
impl Program {
    /// 先頭のモジュールの部分 (`crate::` / `self::` / `super::` / 小文字のモジュール名) を外したパス
    pub fn strip_mods_p(&self, p: &PPath) -> Option<PPath> {
        let mut k = 0;
        let n = p.segs.len();
        while k + 1 < n {
            let s = p.segs[k].name.as_str();
            let module = matches!(s, "crate" | "self" | "super")
                || (s.starts_with(|c: char| c.is_ascii_lowercase())
                    && !matches!(s, "std" | "core" | "alloc" | "str" | "char" | "bool")
                    && Ty::prim(s).is_none()
                    && !self.struct_index.contains_key(s)
                    && !p.segs[k].args);
            if !module {
                break;
            }
            k += 1;
        }
        if k == 0 {
            return None;
        }
        Some(p.tail(k))
    }

    pub fn variant_by_p(&self, p: &PPath, self_sid: Option<u32>) -> Option<(u32, usize)> {
        if let Some(r) = self.variant_by_p_inner(p, self_sid) {
            return Some(r);
        }
        let q = self.strip_mods_p(p)?;
        self.variant_by_p_inner(&q, self_sid)
    }

    fn variant_by_p_inner(&self, p: &PPath, self_sid: Option<u32>) -> Option<(u32, usize)> {
        // `core::option::Option::Some` などは最後の 2 つで見る
        let segs: &[PSeg] = if p.segs.len() > 2 && matches!(p.segs[0].name.as_str(), "core" | "crate" | "std") {
            &p.segs[p.segs.len() - 2..]
        } else {
            &p.segs
        };
        if segs.len() == 1 {
            let n = segs[0].name.as_str();
            let en = match n {
                "Some" | "None" => "Option",
                "Ok" | "Err" => "Result",
                // `use Expr::*;` したバリアント (大文字で始まり、ほかの enum と名前が重ならないもの)
                // (同じ名前の struct があればそちら: `TokenTree::Punct(Punct { .. })`)
                _ if n.starts_with(|c: char| c.is_ascii_uppercase()) => {
                    if self.struct_index.get(n).is_some_and(|&id| !self.structs[id as usize].is_enum) {
                        return None;
                    }
                    return self.variant_names.get(n).copied().flatten();
                }
                _ => return None,
            };
            let sid = *self.struct_index.get(en)?;
            let vi = self.structs[sid as usize].variants.iter().position(|v| v.name == n)?;
            return Some((sid, vi));
        }
        if segs.len() != 2 || segs[1].args {
            return None;
        }
        let tn = segs[0].name.as_str();
        let sid = if tn == "Self" { self_sid? } else { *self.struct_index.get(tn)? };
        let sd = &self.structs[sid as usize];
        if !sd.is_enum {
            return None;
        }
        let vn = &segs[1].name;
        sd.variants.iter().position(|v| &v.name == vn).map(|vi| (sid, vi))
    }

    pub fn struct_by_p(&self, p: &PPath, self_sid: Option<u32>) -> Option<u32> {
        // `std::cmp::Reverse` / `core::cmp::Ordering` などは最後のセグメント (prelude の型)
        if p.segs.len() > 1 && matches!(p.segs[0].name.as_str(), "std" | "core" | "alloc") {
            return self.struct_index.get(&p.segs.last().unwrap().name).copied();
        }
        if p.segs.len() > 1 {
            return self.struct_by_p(&self.strip_mods_p(p)?, self_sid);
        }
        let id = p.ident()?;
        if id == "Self" {
            return self_sid;
        }
        self.struct_index.get(id).copied()
    }

    pub fn fn_by_p(&self, p: &PPath, self_sid: Option<u32>) -> Option<u32> {
        if let Some(r) = self.fn_by_p_inner(p, self_sid) {
            return Some(r);
        }
        let q = self.strip_mods_p(p)?;
        self.fn_by_p_inner(&q, self_sid)
    }

    fn fn_by_p_inner(&self, p: &PPath, self_sid: Option<u32>) -> Option<u32> {
        // 最後のセグメントの turbofish (`f::<T>`) は呼び出し側で読む
        let n = p.segs.len();
        // (`Buf::<8>::new`: 型のセグメントの型引数も呼び出し側で読む)
        if n == 2 && p.segs[0].args && !p.global {
            let mut q = p.clone();
            q.segs[0].args = false;
            return self.fn_by_p_inner(&q, self_sid);
        }
        if p.global || p.segs.iter().take(n.saturating_sub(1)).any(|s| s.args) {
            return None;
        }
        let name = |k: usize| p.segs[k].name.as_str();
        // `std::collections::HashMap::new` などは `HashMap::new`
        if n >= 3 && matches!(name(0), "core" | "std" | "alloc") {
            let ty = name(n - 2);
            if self.struct_index.contains_key(ty) {
                return self.index.get(&format!("{ty}::{}", name(n - 1))).copied();
            }
        }
        // `std::process::exit` / `process::exit` (ユーザの `exit` と名前がぶつからないように別の名前)
        // `fs::read_to_string` / `fs::write` / `env::args` などは prelude の `__core_*`
        if n >= 2 {
            let core = match (name(n - 2), name(n - 1)) {
                ("fs", "read_to_string") => Some("__exr_fs_read_to_string"),
                ("fs", "read") => Some("__exr_fs_read"),
                ("fs", "write") => Some("__exr_fs_write"),
                ("env", "args") => Some("__exr_env_args"),
                ("env", "var") => Some("__exr_env_var"),
                ("env", "var_os") => Some("__exr_env_var_os"),
                ("env", "split_paths") => Some("__exr_env_split_paths"),
                ("fs", "remove_file") => Some("__exr_fs_remove_file"),
                ("Path", "new") => Some("__exr_path_new"),
                ("ptr", "eq") => Some("__exr_ptr_eq"),
                _ => None,
            };
            if let Some(c) = core {
                return self.index.get(c).copied();
            }
        }
        if n >= 2 && name(n - 2) == "process" && name(n - 1) == "exit" {
            return self.index.get("__core_process_exit").copied();
        }
        // `core::cmp::max` / `core::mem::swap` などは prelude の `__core_*` へ
        if n >= 2 && matches!(name(0), "core" | "std") {
            return self.index.get(&format!("__core_{}", name(n - 1))).copied();
        }
        match n {
            1 => {
                let f = name(0);
                // `use core::cmp::max;` してからの `max(..)`: ユーザの関数がなければ core のもの
                self.index.get(f).or_else(|| self.index.get(&format!("__core_{f}"))).copied()
            }
            2 => {
                let (ty, m) = (name(0), name(1));
                // `f64::max` / `f64::sqrt` (関数として): prelude の `__exr_f64_*`、`i32::max` は core::cmp::max
                if ty == "f64" || ty == "f32" {
                    return self.index.get(&format!("__exr_{ty}_{m}")).copied();
                }
                if Ty::from_suffix(ty).is_some() && matches!(m, "max" | "min") {
                    return self.index.get(&format!("__core_{m}")).copied();
                }
                // `char::from_digit(d, 10)` / `char::from_u32(x)` / `char::from(b)`: prelude の `__exr_char_*`
                if ty == "char" {
                    let m = match m {
                        "from_u32" => "from_u32_checked",
                        "from" => "from_u8",
                        _ => m,
                    };
                    return self
                        .index
                        .get(&format!("__exr_char_{m}"))
                        .or_else(|| self.index.get(&format!("__exr_charfn_{m}")))
                        .copied();
                }
                let ty = if ty == "Self" { self.structs[self.base_of(self_sid?) as usize].name.clone() } else { ty.to_string() };
                self.index.get(&format!("{ty}::{m}")).copied()
            }
            _ => None,
        }
    }
}

/// 型の環境で name が const ジェネリクスの引数 (`[(); n]`) なら、その値
pub fn const_generic_value(env: &TyEnv, name: &str) -> Option<u32> {
    env.params.iter().find(|(n, _)| n == name).and_then(|(_, t)| match t {
        Ty::Array(Ty::Unit, n) if *n < CONST_LEN_BASE => Some(*n),
        _ => None,
    })
}
