//! 定義へのジャンプと参照 (LSP の textDocument/definition / references)。
//! 構文木と型推論を使わず、字句の列とシンボルの一覧 (symbols) から引く: 速く、編集の途中の壊れたソースでも動く。
//! 一つに決まらないときは答えない (違う所へ飛ぶより、飛ばない方がよい)。
//!
//! カーソルの識別子が何を指すか:
//! - 定義の名前の上なら、その定義
//! - ローカル (関数の中の let / 引数 / クロージャの引数 / for / if let / while let / match の腕のパターン): ブロックの
//!   有効範囲をたどって、カーソルより前でいちばん近い束縛
//! - パス `A::b`: A を型 (struct / enum / trait / type)・モジュール・`Self` として、そのメンバ (バリアント・impl の中・
//!   モジュールの中のアイテム) の b
//! - メソッド・フィールド `x.b`: x が `self` なら囲む impl の型、ローカルなら束縛の型注釈・`T::..` / `T { .. }` の初期化・
//!   引数の型から型を決め、その型のメンバの b。型が分からなければ、ワークスペースで一つだけのとき
//! - そのほかの名前: 同じモジュール → `use` で取り込んだもの → ワークスペースで一つだけ
//! 参照は、同じ名前の識別子のうち、同じ定義に解決されるもの

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use crate::fxhash::FxMap;
use crate::lexer::tokenize;
use crate::symbols::{SymKind, Symbol, document_symbols};

/// 定義 (シンボルを平らにして、持ち主とモジュールを付けたもの)
#[derive(Clone, Debug)]
pub struct Def {
    pub name: String,
    pub kind: SymKind,
    /// メンバの持ち主の型・trait の名前 (impl なら self 型、enum のバリアントなら enum)。トップレベルのアイテムは空
    pub owner: String,
    /// 囲むもの (表示用: `impl Point` / モジュール / struct の名前)
    pub container: Arc<str>,
    /// 囲むモジュール (ファイルのモジュールのパスの後ろに、ファイルの中の `mod x { .. }`)
    pub module: Arc<[String]>,
    /// 名前と、アイテム全体の byte 範囲
    pub start: usize,
    pub end: usize,
    pub item_start: usize,
    pub item_end: usize,
    /// 型の頭の名前: フィールド・const・static はその型、関数・メソッドは戻り値の型、型の別名は右の型 (`&mut Vec<T>` → Vec)
    pub ty: String,
    /// 型の別名の型引数の名前 (`type R<T> = Result<T, E>` → [T])
    pub params: Vec<String>,
    /// slim のとき (本文を捨てたとき): 名前の両端の (行, UTF-16 の桁)
    pub pos: [u32; 4],
}

/// ファイル 1 つ分の索引
pub struct FileIndex {
    pub text: String,
    /// ライブラリ (std / core / alloc) のファイル: 定義だけを持つ (字句の列は持たない)。ワークスペースに候補がないときだけ使う
    pub lib: bool,
    /// パッケージの本体のクレート (src の lib / main / bin) のファイルか。tests・examples・benches・build.rs のクレートの
    /// アイテムは、同じクレート (krate) の中からしか見えない
    pub in_src: bool,
    /// ターゲットのクレートの番号 (同じ番号のファイルどうしは見える)
    pub krate: u32,
    /// ファイルのモジュールのパス (ワークスペースの中で: `src/a/b.rs` → [a, b])
    pub module: Vec<String>,
    pub defs: Vec<Def>,
    /// (種類, 始め, 終わり)。u32 で持つ (ワークスペースの全ファイルの分を持つので小さく)
    toks: Vec<(u8, u32, u32)>,
    /// 名前 → その識別子のトークンの番号
    idents: FxMap<String, Vec<u32>>,
    /// `use` で取り込んだもの: (このファイルでの名前 (glob なら "*"), パス)
    uses: Vec<(String, Vec<String>)>,
    /// uses のそれぞれの、パスの最後の名前の byte 位置 (glob は usize::MAX)
    use_at: Vec<usize>,
    /// `macro_rules!` の本体の byte 範囲 (関数の中のものも)
    macros: Vec<(usize, usize)>,
    /// 本文と字句の列を捨てたもの (ライブラリと、メモリから外したワークスペースのファイル)。定義と use と名前の集合だけ
    pub slim: bool,
    /// (self 型の頭, trait の頭): `impl Tr for S` と `#[derive(Tr)] struct S`
    pub impls: Vec<(String, String)>,
    /// slim のとき: 出てくる識別子の名前のハッシュ (整列済み。参照を探すときに読み直すファイルを選ぶ)
    names: Vec<u64>,
}

/// ファイルの中の位置 (ws の番号, 名前の byte 範囲)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loc {
    pub file: usize,
    pub start: usize,
    pub end: usize,
}

/// `impl Tr for S<T>` / `impl S` の self 型の名前 (`S`)
fn impl_self(name: &str) -> String {
    let n = name.strip_prefix("impl ").unwrap_or(name);
    let n = match n.rfind(" for ") {
        Some(i) => &n[i + 5..],
        None => n,
    };
    let n = n.trim_start_matches('&').trim_start_matches("mut ").trim();
    let n = n.split('<').next().unwrap_or(n);
    n.rsplit("::").next().unwrap_or(n).trim().to_string()
}

impl FileIndex {
    pub fn new(text: String, module: Vec<String>) -> FileIndex {
        let toks: Vec<(u8, u32, u32)> = tokenize(text.as_bytes()).into_iter().map(|(a, b, c)| (a, b as u32, c as u32)).collect();
        let syms = document_symbols(&text);
        let mut defs = vec![];
        // (囲むもの・モジュールのパスは同じものを共有する: ライブラリや大きいワークスペースの定義の数だけ持つので)
        fn walk(v: &[Symbol], owner: &str, container: &Arc<str>, module: &Arc<[String]>, out: &mut Vec<Def>) {
            for s in v {
                {
                    out.push(Def {
                        name: s.name.clone(),
                        kind: s.kind,
                        owner: if s.kind == SymKind::Impl { String::new() } else { owner.to_string() },
                        container: container.clone(),
                        module: module.clone(),
                        start: s.name_start,
                        end: s.name_end,
                        item_start: s.start,
                        item_end: s.end,
                        ty: match s.kind {
                            SymKind::Field | SymKind::Const | SymKind::Static => s.detail.trim().to_string(),
                            SymKind::TypeAlias => alias_parts(&s.detail).1,
                            SymKind::Function | SymKind::Method => {
                                let t = ret_type(&s.detail);
                                // (impl の中の Self は impl の self 型)
                                if container.starts_with("impl") { subst(&t, &["Self".to_string()], &[impl_self_full(&container[..]).to_string()]) } else { t }
                            }
                            SymKind::Variant => owner.to_string(),
                            _ => String::new(),
                        },
                        params: if s.kind == SymKind::TypeAlias { alias_parts(&s.detail).0 } else { vec![] },
                        pos: [0; 4],
                    });
                }
                if s.children.is_empty() {
                    continue;
                }
                let c: Arc<str> = Arc::from(s.name.as_str());
                match s.kind {
                    SymKind::Impl => walk(&s.children, &impl_self(&s.name), &c, module, out),
                    SymKind::Module => {
                        let mut m = module.to_vec();
                        m.push(s.name.clone());
                        walk(&s.children, "", &c, &Arc::from(m), out);
                    }
                    _ => walk(&s.children, &s.name, &c, module, out),
                }
            }
        }
        walk(&syms, "", &Arc::from(""), &Arc::from(module.clone()), &mut defs);
        let mut idents: FxMap<String, Vec<u32>> = FxMap::default();
        for (i, t) in toks.iter().enumerate() {
            if t.0 == 0 {
                idents.entry(text[t.1 as usize..t.2 as usize].to_string()).or_default().push(i as u32);
            }
        }
        let mut f = FileIndex { text, lib: false, in_src: true, krate: 0, module, defs, toks, idents, uses: vec![], use_at: vec![], macros: vec![], slim: false, names: vec![], impls: vec![] };
        (f.uses, f.use_at) = f.collect_uses(false).into_iter().map(|(a, b, c)| ((a, b), c)).unzip();
        f.macros = f.macro_bodies();
        f.impls = f.collect_impls();
        f
    }

    /// トークン k を囲む構造体の式 `T { .. }` の T
    fn struct_lit_at(&self, k: usize) -> Option<String> {
        let mut d = 0;
        let mut j = k;
        while j > 0 {
            j -= 1;
            if self.sym(j, b'}') || self.sym(j, b')') || self.sym(j, b']') {
                d += 1;
            } else if self.sym(j, b'(') || self.sym(j, b'[') {
                if d == 0 {
                    return None;
                }
                d -= 1;
            } else if self.sym(j, b'{') {
                if d == 0 {
                    return (j >= 1 && self.ident(j - 1) && self.tok(j - 1).starts_with(|c: char| c.is_ascii_uppercase())).then(|| self.tok(j - 1).to_string());
                }
                d -= 1;
            }
        }
        None
    }

    fn collect_impls(&self) -> Vec<(String, String)> {
        let mut out = vec![];
        for d in &self.defs {
            if d.kind == SymKind::Impl {
                let n = d.name.strip_prefix("impl ").unwrap_or(&d.name);
                if let Some(i) = n.rfind(" for ") {
                    let s = type_head(&n[i + 5..]);
                    let t = type_head(n[..i].trim_start_matches('!'));
                    // (`impl<T: X> Tr for T` のような全部への impl は除く)
                    if !s.is_empty() && !t.is_empty() && !(s.len() == 1 && s.starts_with(|c: char| c.is_ascii_uppercase())) {
                        out.push((s, t));
                    }
                }
            }
        }
        // `#[derive(A, B)]` の後の struct / enum / union
        let n = self.toks.len();
        let mut k = 0;
        while k < n {
            if self.ident(k) && self.tok(k) == "derive" && self.sym(k + 1, b'(') && k >= 2 && self.sym(k - 1, b'[') {
                let mut j = k + 2;
                let mut names = vec![];
                while j < n && !self.sym(j, b')') {
                    if self.ident(j) && !self.sym(j + 1, b':') {
                        names.push(self.tok(j).to_string());
                    }
                    j += 1;
                }
                // (次のアイテムの名前: 属性・pub を飛ばす)
                let mut q = j + 1;
                while q < n && !(self.ident(q) && matches!(self.tok(q), "struct" | "enum" | "union")) && q < j + 64 {
                    q += 1;
                }
                if q + 1 < n && self.ident(q + 1) {
                    let s = self.tok(q + 1).to_string();
                    for t in names {
                        out.push((s.clone(), t));
                    }
                }
                k = j;
            }
            k += 1;
        }
        out
    }

    /// ライブラリのファイル: 定義と use だけ (本文・字句の列・名前の表は捨てる。位置は先に行と桁にしておく)
    pub fn library(text: String, module: Vec<String>) -> FileIndex {
        let mut f = FileIndex::new(text, module);
        f.lib = true;
        f.toks = vec![];
        f.idents = FxMap::default();
        // (ほかのファイルから見えるのは `pub use` (再エクスポート) だけ)
        f.uses = f.collect_uses(true).into_iter().map(|(a, b, _)| (a, b)).collect();
        f.make_slim();
        f.names = vec![];
        f
    }

    /// 本文・字句の列・名前の表を捨てる (定義の位置は行と桁にしておく。名前はハッシュの集合だけ残す)
    pub fn make_slim(&mut self) {
        if self.slim {
            return;
        }
        let mut names: Vec<u64> = self.idents.keys().map(|k| name_hash(k)).collect();
        names.sort_unstable();
        names.dedup();
        self.names = names;
        let li = crate::symbols::LineIndex::new(&self.text);
        for d in self.defs.iter_mut() {
            let (a, b) = (li.pos(&self.text, d.start), li.pos(&self.text, d.end));
            d.pos = [a.0, a.1, b.0, b.1];
        }
        self.text = String::new();
        self.toks = vec![];
        self.idents = FxMap::default();
        self.macros = vec![];
        self.use_at = vec![];
        self.uses.shrink_to_fit();
        self.defs.shrink_to_fit();
        self.slim = true;
    }

    /// slim なファイルを本文から作り直したもの (モジュール・クレートは同じ)
    pub fn reload(&self, text: String) -> FileIndex {
        let mut f = FileIndex::new(text, self.module.clone());
        f.lib = self.lib;
        f.in_src = self.in_src;
        f.krate = self.krate;
        f
    }

    /// name の識別子が出てくるかもしれないか (slim でなければ名前の表で、slim ならハッシュで)
    fn may_have(&self, name: &str) -> bool {
        if self.slim { self.names.binary_search(&name_hash(name)).is_ok() } else { self.idents.contains_key(name) }
    }

    /// ヒープの大きさの見積もり (byte): (定義, 本文と字句の列と名前の表, use とその他)
    pub fn heap_bytes(&self) -> (usize, usize, usize) {
        let vs = |v: &Vec<String>| v.capacity() * 24 + v.iter().map(|x| x.capacity()).sum::<usize>();
        let defs = self.defs.capacity() * std::mem::size_of::<Def>()
            + self.defs.iter().map(|d| d.name.capacity() + d.owner.capacity() + d.ty.capacity() + vs(&d.params)).sum::<usize>();
        let body = self.text.capacity() + self.toks.capacity() * 12 + self.idents.iter().map(|(k, v)| k.capacity() + v.capacity() * 4 + 48).sum::<usize>();
        let other = self.uses.iter().map(|(a, b)| a.capacity() + vs(b) + 48).sum::<usize>()
            + self.impls.iter().map(|(a, b)| a.capacity() + b.capacity() + 48).sum::<usize>()
            + self.names.capacity() * 8
            + self.module.capacity() * 24;
        (defs, body, other)
    }

    /// 本文の大きさ (メモリの見積もり)
    pub fn text_len(&self) -> usize {
        self.text.len()
    }

    /// ライブラリのファイルの定義の名前の端の (行, UTF-16 の桁)
    pub fn lib_pos(&self, at: usize) -> Option<(u32, u32)> {
        self.defs.iter().find_map(|d| {
            if d.start == at {
                Some((d.pos[0], d.pos[1]))
            } else if d.end == at {
                Some((d.pos[2], d.pos[3]))
            } else {
                None
            }
        })
    }

    fn tok(&self, k: usize) -> &str {
        let t = self.toks[k];
        &self.text[t.1 as usize..t.2 as usize]
    }
    fn sym(&self, k: usize, c: u8) -> bool {
        k < self.toks.len() && self.toks[k].0 == 1 && self.text.as_bytes()[self.toks[k].1 as usize] == c
    }
    fn ident(&self, k: usize) -> bool {
        k < self.toks.len() && self.toks[k].0 == 0
    }
    /// k の前が `::` か
    fn after_path(&self, k: usize) -> bool {
        k >= 2 && self.sym(k - 1, b':') && self.sym(k - 2, b':')
    }
    /// k の後ろが `::` か
    fn before_path(&self, k: usize) -> bool {
        self.sym(k + 1, b':') && self.sym(k + 2, b':')
    }
    /// byte 位置にある識別子のトークン
    fn tok_at(&self, off: usize) -> Option<usize> {
        let i = self.toks.partition_point(|t| (t.2 as usize) < off);
        for k in [i, i + 1, i.wrapping_sub(1)] {
            if k < self.toks.len() && self.toks[k].0 == 0 && (self.toks[k].1 as usize) <= off && off <= (self.toks[k].2 as usize) {
                return Some(k);
            }
        }
        None
    }

    /// `use a::b::{C, D as E, f::*};` を (名前, パス) の列に
    fn macro_bodies(&self) -> Vec<(usize, usize)> {
        let mut out = vec![];
        let n = self.toks.len();
        for k in 0..n {
            if self.tok(k) == "macro_rules" && self.sym(k + 1, b'!') && self.ident(k + 2) && (self.sym(k + 3, b'{') || self.sym(k + 3, b'(')) {
                let mut d = 0i32;
                let mut j = k + 3;
                while j < n {
                    if self.toks[j].0 == 1 {
                        match self.text.as_bytes()[self.toks[j].1 as usize] {
                            b'{' | b'(' | b'[' => d += 1,
                            b'}' | b')' | b']' => {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    j += 1;
                }
                let end = if j < n { self.toks[j].2 as usize } else { self.text.len() };
                out.push(((self.toks[k + 3].1 as usize), end));
            }
        }
        out
    }

    fn collect_uses(&self, only_pub: bool) -> Vec<(String, Vec<String>, usize)> {
        let mut out = vec![];
        let n = self.toks.len();
        let mut k = 0;
        while k < n {
            if self.tok(k) == "use" && (!only_pub || (k >= 1 && self.tok(k - 1) == "pub")) && (k == 0 || self.sym(k - 1, b';') || self.sym(k - 1, b'}') || self.sym(k - 1, b'{') || self.sym(k - 1, b']') || self.tok(k - 1) == "pub" || self.sym(k - 1, b')')) {
                let mut e = k + 1;
                while e < n && !self.sym(e, b';') {
                    e += 1;
                }
                self.use_tree(k + 1, e, &mut vec![], &mut out);
                k = e;
            }
            k += 1;
        }
        out
    }
    fn use_tree(&self, a: usize, e: usize, prefix: &mut Vec<String>, out: &mut Vec<(String, Vec<String>, usize)>) {
        let mut path = prefix.clone();
        let mut k = a;
        while k < e {
            if self.ident(k) {
                let w = self.tok(k).to_string();
                if w == "as" && self.ident(k + 1) {
                    out.push((self.tok(k + 1).to_string(), path.clone(), (self.toks[k - 1].1 as usize)));
                    path = prefix.clone();
                    k += 2;
                    continue;
                }
                path.push(w);
                k += 1;
                if !(self.sym(k, b':') && self.sym(k + 1, b':')) && !(self.ident(k) && self.tok(k) == "as") {
                    out.push((path.last().cloned().unwrap_or_default(), path.clone(), (self.toks[k - 1].1 as usize)));
                    path = prefix.clone();
                }
            } else if self.sym(k, b'{') {
                // (対応する `}`)
                let mut d = 0;
                let mut c = k;
                while c < e {
                    if self.sym(c, b'{') {
                        d += 1;
                    } else if self.sym(c, b'}') {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    c += 1;
                }
                let mut p = path.clone();
                // (中はカンマで区切った木)
                let mut s = k + 1;
                let mut j = k + 1;
                let mut dd = 0;
                while j <= c {
                    if self.sym(j, b'{') {
                        dd += 1;
                    } else if self.sym(j, b'}') {
                        dd -= 1;
                    }
                    if (self.sym(j, b',') && dd == 0) || j == c {
                        self.use_tree(s, j, &mut p, out);
                        s = j + 1;
                    }
                    j += 1;
                }
                path = prefix.clone();
                k = c + 1;
            } else if self.sym(k, b'*') {
                out.push(("*".to_string(), path.clone(), usize::MAX));
                path = prefix.clone();
                k += 1;
            } else {
                k += 1;
            }
        }
    }

    /// off を含むいちばん内側の関数の定義
    fn enclosing_fn(&self, off: usize) -> Option<&Def> {
        self.defs
            .iter()
            .filter(|d| matches!(d.kind, SymKind::Function | SymKind::Method) && d.item_start <= off && off < d.item_end)
            .min_by_key(|d| d.item_end - d.item_start)
    }
    /// off を囲む impl の self 型 (メソッドの持ち主)
    fn enclosing_owner(&self, off: usize) -> Option<String> {
        self.enclosing_fn(off).filter(|d| !d.owner.is_empty()).map(|d| d.owner.clone())
    }
    /// off を囲むモジュール (ファイルの中の `mod x { .. }` も)
    fn module_at(&self, off: usize) -> Vec<String> {
        let mut m = self.module.clone();
        let mut best: Option<&Def> = None;
        for d in &self.defs {
            if d.kind == SymKind::Module && d.item_start <= off && off < d.item_end && best.is_none_or(|b| d.item_end - d.item_start < b.item_end - b.item_start) {
                best = Some(d);
            }
        }
        if let Some(b) = best {
            m = b.module.to_vec();
            m.push(b.name.clone());
        }
        m
    }

    /// トークン k (名前 name) のローカルの束縛 (トークンの番号)。関数の中で、カーソルより前でいちばん近いもの
    fn local(&self, k: usize, name: &str) -> Option<usize> {
        self.live(k).into_iter().rev().find(|&j| self.tok(j) == name && j != k)
    }

    /// トークン k の位置で見えるローカルの束縛 (トークンの番号、外側から順に)
    fn live(&self, k: usize) -> Vec<usize> {
        let Some(f) = self.enclosing_fn(self.toks[k].1 as usize) else { return vec![] };
        // (関数の名前のトークンから)
        let mut i = self.toks.partition_point(|t| (t.1 as usize) < f.start);
        let is_bind = |j: usize| -> bool {
            let w = self.tok(j);
            self.ident(j)
                && w.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
                && !matches!(w, "mut" | "ref" | "box" | "in" | "if" | "let" | "true" | "false" | "self" | "_")
                && !self.after_path(j)
                && !self.before_path(j)
                && !self.sym(j + 1, b'(')
                && !self.sym(j + 1, b'!')
                && !(j > 0 && self.sym(j - 1, b'.'))
        };
        // 有効な束縛: (名前, トークン, 消える深さ, カンマで消えるか, ; で消えるか)
        let mut live: Vec<(usize, i32, u8)> = vec![];
        // (次のブロックで有効になるもの: for / if let / while let)
        let mut pend_block: Vec<usize> = vec![];
        // (文の終わり `;` で有効になるもの: let)
        let mut pend_let: Vec<(usize, i32)> = vec![];
        let mut depth = 0i32;
        // (関数の引数: 名前の後の `(` .. `)` の `x:`)
        i += 1;
        if self.sym(i, b'<') {
            let mut d = 0;
            while i < k {
                if self.sym(i, b'<') {
                    d += 1;
                } else if self.sym(i, b'>') {
                    d -= 1;
                    if d == 0 {
                        i += 1;
                        break;
                    }
                }
                i += 1;
            }
        }
        if self.sym(i, b'(') {
            let mut d = 0;
            let mut seg_start = i + 1;
            while i < k {
                if self.sym(i, b'(') || self.sym(i, b'[') {
                    d += 1;
                } else if self.sym(i, b')') || self.sym(i, b']') {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                if d == 1 && self.sym(i, b':') && !self.sym(i + 1, b':') && !(i > 0 && self.sym(i - 1, b':')) {
                    for j in seg_start..i {
                        if is_bind(j) {
                            pend_block.push(j);
                        }
                    }
                }
                if d == 1 && self.sym(i, b',') {
                    seg_start = i + 1;
                }
                i += 1;
            }
        }
        // (本体)
        let mut stmt_kind: u8 = 0; // 1: let のパターン, 2: for / if let / while let のパターン, 3: クロージャの引数
        let mut pat_start = 0usize;
        let mut arm_start = i;
        while i < k {
            let t = self.toks[i];
            if t.0 == 1 {
                match self.text.as_bytes()[t.1 as usize] {
                    b'{' => {
                        depth += 1;
                        for j in pend_block.drain(..) {
                            live.push((j, depth, 0));
                        }
                        arm_start = i + 1;
                        if stmt_kind == 2 {
                            stmt_kind = 0;
                        }
                    }
                    b'(' | b'[' => depth += 1,
                    b'}' | b')' | b']' => {
                        depth -= 1;
                        live.retain(|l| l.1 <= depth);
                        if self.sym(i, b'}') {
                            arm_start = i + 1;
                        }
                    }
                    b';' => {
                        live.retain(|l| !(l.2 == 2 && l.1 == depth));
                        for (j, d) in pend_let.drain(..) {
                            live.push((j, d, 0));
                        }
                        stmt_kind = 0;
                        arm_start = i + 1;
                    }
                    b',' => {
                        live.retain(|l| !(l.2 == 1 && l.1 == depth));
                        arm_start = i + 1;
                    }
                    b'=' if self.sym(i + 1, b'>') => {
                        // match の腕のパターン
                        for j in arm_start..i {
                            if is_bind(j) && !self.sym(j + 1, b'{') {
                                live.push((j, depth, 1));
                            }
                        }
                        i += 1;
                    }
                    b'=' if !self.sym(i + 1, b'=') && !(i > 0 && matches!(self.text.as_bytes()[self.toks[i - 1].1 as usize], b'=' | b'!' | b'<' | b'>' | b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^')) => {
                        if stmt_kind == 1 || stmt_kind == 2 {
                            for j in pat_start..i {
                                if is_bind(j) {
                                    if stmt_kind == 1 {
                                        pend_let.push((j, depth));
                                    } else {
                                        pend_block.push(j);
                                    }
                                }
                            }
                            if stmt_kind == 1 {
                                stmt_kind = 4; // (型注釈の後の初期化式)
                            } else {
                                stmt_kind = 0;
                            }
                        }
                    }
                    b':' if stmt_kind == 1 && !self.sym(i + 1, b':') && !(i > 0 && self.sym(i - 1, b':')) => {
                        // `let x: T = ..` の型注釈の前までがパターン
                        for j in pat_start..i {
                            if is_bind(j) {
                                pend_let.push((j, depth));
                            }
                        }
                        stmt_kind = 4;
                    }
                    b'|' if stmt_kind != 3 && (i == 0 || !self.sym(i - 1, b'|')) && !self.sym(i + 1, b'|') && self.closure_start(i) => {
                        stmt_kind = 3;
                        pat_start = i + 1;
                    }
                    b'|' if stmt_kind == 3 => {
                        for j in pat_start..i {
                            if is_bind(j) && !(j > 0 && self.sym(j - 1, b':')) {
                                live.push((j, depth, 2));
                            }
                        }
                        stmt_kind = 0;
                    }
                    _ => {}
                }
            } else if t.0 == 0 {
                match self.tok(i) {
                    "let" => {
                        // (`if let` / `while let` は次のブロックで有効)
                        let cond = i > 0 && matches!(self.tok(i - 1), "if" | "while");
                        stmt_kind = if cond { 2 } else { 1 };
                        pat_start = i + 1;
                    }
                    "for" if !self.sym(i + 1, b'<') => {
                        // `for PAT in`
                        let mut j = i + 1;
                        while j < k && self.tok(j) != "in" {
                            if is_bind(j) {
                                pend_block.push(j);
                            }
                            j += 1;
                        }
                        i = j;
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        live.extend(pend_block.iter().map(|&j| (j, depth, 0)));
        live.into_iter().map(|l| l.0).collect()
    }

    /// k が `#[derive(..)]` の中の名前か
    fn in_derive(&self, k: usize) -> bool {
        let mut j = k;
        while j > 0 && (self.ident(j - 1) || self.sym(j - 1, b',') || self.sym(j - 1, b':')) {
            j -= 1;
        }
        j >= 4 && self.sym(j - 1, b'(') && self.ident(j - 2) && self.tok(j - 2) == "derive" && self.sym(j - 3, b'[') && self.sym(j - 4, b'#')
    }

    /// byte 位置 at が macro_rules! の本体の中か (名前を除く)
    fn in_macro_body(&self, at: usize) -> bool {
        self.macros.iter().any(|&(a, b)| a <= at && at < b) || self.defs.iter().any(|d| d.kind == SymKind::Macro && d.item_start <= at && at < d.item_end && at != d.start)
    }

    /// `|` がクロージャの引数の始まりか (前が式の始まりになれる記号か `move`)
    fn closure_start(&self, i: usize) -> bool {
        if i == 0 {
            return true;
        }
        let p = i - 1;
        if self.ident(p) {
            return self.tok(p) == "move" || self.tok(p) == "return";
        }
        self.toks[p].0 == 1 && matches!(self.text.as_bytes()[self.toks[p].1 as usize], b'(' | b',' | b'=' | b'{' | b';' | b'[' | b':')
    }

    /// ローカルの束縛 b の型の名前 (型注釈・`T::..` / `T { .. }` / `T(..)` の初期化・引数の型)
    fn local_type(&self, b: usize) -> Option<String> {
        let n = self.toks.len();
        let mut j = b + 1;
        if self.sym(j, b':') && !self.sym(j + 1, b':') {
            j += 1;
            while j < n && (self.sym(j, b'&') || self.tok(j) == "mut" || self.toks[j].0 == 2) {
                j += 1;
            }
            // (`a::b::T<..>` の最後)
            while self.ident(j) && self.before_path(j) {
                j += 3;
            }
            if self.ident(j) {
                return Some(self.tok(j).to_string());
            }
            return None;
        }
        if self.sym(j, b'=') && !self.sym(j + 1, b'=') {
            j += 1;
            while j < n && (self.sym(j, b'&') || self.tok(j) == "mut") {
                j += 1;
            }
            let first = j;
            while self.ident(j) && self.before_path(j) && self.ident(j + 3) && self.tok(j + 3).starts_with(|c: char| c.is_ascii_uppercase()) {
                j += 3;
            }
            if self.ident(j) && self.tok(j).starts_with(|c: char| c.is_ascii_uppercase()) && (self.before_path(j) || self.sym(j + 1, b'{') || self.sym(j + 1, b'(')) {
                let _ = first;
                return Some(self.tok(j).to_string());
            }
        }
        None
    }
}

fn loc(file: usize, d: &Def) -> Loc {
    Loc { file, start: d.start, end: d.end }
}

const ITEM_KINDS: [SymKind; 10] = [
    SymKind::Function,
    SymKind::Const,
    SymKind::Static,
    SymKind::Struct,
    SymKind::Enum,
    SymKind::Union,
    SymKind::Trait,
    SymKind::TypeAlias,
    SymKind::Macro,
    SymKind::Module,
];

/// 候補が一つなら答える。いくつもあれば同じファイルのものが一つならそれ
fn pick(c: Vec<(usize, &Def)>, file: usize) -> Vec<Loc> {
    if c.len() == 1 {
        return vec![loc(c[0].0, c[0].1)];
    }
    let same: Vec<&(usize, &Def)> = c.iter().filter(|x| x.0 == file).collect();
    if same.len() == 1 {
        return vec![loc(same[0].0, same[0].1)];
    }
    vec![]
}

/// ワークスペースの候補があればそれ、なければライブラリの候補 (モジュールのパスが短いものを先に: `sys` の下などの
/// 中のものより、公開されているもの)
fn prefer_ws(ws: &[&FileIndex], c: Vec<(usize, &Def)>, file: usize) -> Vec<Loc> {
    let (w, l): (Vec<(usize, &Def)>, Vec<(usize, &Def)>) = c.into_iter().partition(|x| !ws[x.0].lib);
    if !w.is_empty() {
        return pick(w, file);
    }
    if l.is_empty() {
        return vec![];
    }
    let score = lib_rank;
    let best = l.iter().map(|x| score(x.1)).min().unwrap();
    pick(l.into_iter().filter(|x| score(x.1) == best).collect(), file)
}

/// ライブラリの定義の順位 (小さいほど先): モジュールのパスが短いもの。`sys` / `os` の下や、既定でない `nonpoison` などは後
fn lib_rank(d: &Def) -> usize {
    let bad = d.module.first().is_some_and(|m| m == "sys" || m == "os" || m.starts_with("sys_")) || d.module.iter().any(|m| m == "nonpoison" || m == "unix" || m == "windows" || m == "wasi");
    d.module.len() + if bad { 100 } else { 0 }
}

/// `use` のパス (最後の名前を除いたモジュールの部分) → (モジュールのパス, クレートの中のパスか)
fn use_module(here: &[String], path: &[String]) -> (Vec<String>, bool) {
    match path.first().map(|s| s.as_str()) {
        Some("crate") => (path[1..].to_vec(), true),
        Some("self") => {
            let mut m = here.to_vec();
            m.extend(path[1..].iter().cloned());
            (m, true)
        }
        Some("super") => {
            let mut m = here.to_vec();
            let mut i = 0;
            while i < path.len() && path[i] == "super" {
                m.pop();
                i += 1;
            }
            m.extend(path[i..].iter().cloned());
            (m, true)
        }
        // (ほかのクレート `std::sync` / `crustc_core::driver2`: クレートの名前を外す)
        Some(_) => (path[1..].to_vec(), false),
        None => (vec![], true),
    }
}

/// d のモジュールが target に合うか (小さいほど良い)。exact なら同じパスだけ。そうでなければ後ろが同じか、順に含む
fn mod_score(d: &[String], target: &[String], exact: bool) -> Option<usize> {
    if d == target {
        return Some(0);
    }
    if exact {
        return None;
    }
    if d.ends_with(target) {
        return Some(1 + d.len() - target.len());
    }
    let mut it = d.iter();
    if target.iter().all(|t| it.any(|x| x == t)) {
        return Some(10 + d.len());
    }
    None
}

/// 候補を `use` のモジュールで絞る (いちばん合うもの)
fn by_use<'a>(cands: &[(usize, &'a Def)], target: &[String], exact: bool) -> Vec<(usize, &'a Def)> {
    let scored: Vec<(usize, (usize, &'a Def))> = cands.iter().filter_map(|c| mod_score(&c.1.module[..], target, exact).map(|s| (s, *c))).collect();
    let Some(best) = scored.iter().map(|x| x.0).min() else { return vec![] };
    scored.into_iter().filter(|x| x.0 == best).map(|x| x.1).collect()
}

/// off (byte) にある識別子の定義
pub fn definition(ws: &[&FileIndex], file: usize, off: usize) -> Vec<Loc> {
    clear_memo();
    def_at(ws, file, off)
}

fn def_at(ws: &[&FileIndex], file: usize, off: usize) -> Vec<Loc> {
    if ws.is_empty() {
        return vec![];
    }
    let f = ws[file];
    let Some(k) = f.tok_at(off) else { return vec![] };
    let name = f.tok(k).to_string();
    let start = f.toks[k].1 as usize;
    // (組み込みの型には定義がない)
    if PRIMS.contains(&name.as_str()) {
        return vec![];
    }
    // 定義の名前の上
    // (impl の名前の範囲は self 型の名前なので、impl は飛ばす: `impl P` の P は struct P)
    if let Some(d) = f.defs.iter().find(|d| d.start == start && d.kind != SymKind::Impl) {
        return vec![loc(file, d)];
    }
    // (見える定義: ライブラリと src の下のファイルと、このファイル)
    let kr = f.krate;
    let all = || ws.iter().enumerate().filter(move |(i, w)| *i == file || w.in_src || w.lib || w.krate == kr).flat_map(|(i, w)| w.defs.iter().map(move |d| (i, d)));
    // メソッド・フィールド `x.name`
    if k >= 1 && f.sym(k - 1, b'.') && !(k >= 2 && f.sym(k - 2, b'.')) {
        let call = f.sym(k + 1, b'(') || (f.before_path(k) && f.sym(k + 3, b'<'));
        let kinds: &[SymKind] = if call { &[SymKind::Method] } else { &[SymKind::Field] };
        let owner = k.checked_sub(2).and_then(|r| expr_type(ws, file, r, 0)).filter(|t| !type_head(t).is_empty() && type_head(t) != "_");
        let (owners, concrete) = owner.as_ref().map(|o| owners_of2(ws, file, o)).unwrap_or_default();
        let c: Vec<(usize, &Def)> = all().filter(|(_, d)| d.name == name && kinds.contains(&d.kind) && (owner.is_none() || owners.contains(&d.owner))).collect();
        let c = prefer_concrete(c, &concrete);
        if owner.is_none() {
            // (受け手の型が分からないときは、ライブラリも含めて一つだけのときだけ)
            return pick(c, file);
        }
        return prefer_ws(ws, c, file);
    }
    // パス `Q::name`
    if f.after_path(k) && k >= 3 && f.ident(k - 3) {
        let q = f.tok(k - 3).to_string();
        let mut q = if q == "Self" { f.enclosing_owner(off).unwrap_or(q) } else { q };
        // (`T { .., ..Default::default() }` の default は T の)
        if (q == "Default" && name == "default") && k >= 5 && f.sym(k - 4, b'.') && f.sym(k - 5, b'.') {
            if let Some(t) = f.struct_lit_at(k - 5) {
                q = t;
            }
        }
        // (`T { field: Default::default() }` の default はフィールドの型の)
        if (q == "Default" && name == "default") && k >= 5 && f.sym(k - 4, b':') && !f.sym(k - 5, b':') && f.ident(k - 5) {
            if let Some(t) = f.struct_lit_at(k - 5) {
                let field = f.tok(k - 5);
                if let Some(ty) = all().map(|x| x.1).find(|d| d.kind == SymKind::Field && d.owner == t && d.name == field).map(|d| type_head(&d.ty)).filter(|h| !h.is_empty()) {
                    q = ty;
                }
            }
        }
        if !matches!(q.as_str(), "crate" | "super" | "self") {
            // (型のメンバか、モジュールの中のアイテム)
            let (qs, nc) = owner_split(ws, file, &q);
            let c: Vec<(usize, &Def)> = all().filter(|(_, d)| d.name == name && (qs.contains(&d.owner) || (d.owner.is_empty() && d.module.last() == Some(&q)))).collect();
            let c = prefer_concrete(c, &qs[..nc]);
            // (見つからない: `use std::sync::Mutex` のように再エクスポートされた名前。下の use でたどる)
            if !c.is_empty() {
                return prefer_ws(ws, c, file);
            }
        }
    }
    if name == "Self" {
        if let Some(o) = f.enclosing_owner(off) {
            let c: Vec<(usize, &Def)> = all().filter(|(_, d)| d.name == o && d.owner.is_empty() && matches!(d.kind, SymKind::Struct | SymKind::Enum | SymKind::Union | SymKind::TypeAlias)).collect();
            return prefer_ws(ws, c, file);
        }
        return vec![];
    }
    // `#[derive(Name)]` の Name は derive のマクロ
    if f.in_derive(k) {
        let c: Vec<(usize, &Def)> = all().filter(|(_, d)| d.name == name && d.kind == SymKind::Macro).collect();
        return prefer_ws(ws, c, file);
    }
    // (`let x` / `let mut x` の x はそれ自身)
    if k >= 1
        && (f.tok(k - 1) == "let" || (f.tok(k - 1) == "mut" && k >= 2 && f.tok(k - 2) == "let"))
        && !name.starts_with(|c: char| c.is_ascii_uppercase())
        && !f.sym(k + 1, b'(')
        && !f.sym(k + 1, b'{')
        && !(f.sym(k + 1, b':') && f.sym(k + 2, b':'))
    {
        return vec![Loc { file, start, end: (f.toks[k].2 as usize) }];
    }
    // ローカル
    if !f.after_path(k) {
        if let Some(b) = f.local(k, &name) {
            return vec![Loc { file, start: (f.toks[b].1 as usize), end: (f.toks[b].2 as usize) }];
        }
    }
    // アイテム: 同じモジュール → use → ワークスペースで一つ → ライブラリ (prelude の Option / Some など)
    let mut cands: Vec<(usize, &Def)> = all().filter(|(_, d)| d.name == name && d.owner.is_empty() && ITEM_KINDS.contains(&d.kind)).collect();
    if cands.is_empty() {
        // (`Some` / `None` / `Ok` / `Err`: prelude のバリアント)
        cands = all().filter(|(i, d)| ws[*i].lib && d.name == name && d.kind == SymKind::Variant && (d.owner == "Option" || d.owner == "Result")).collect();
    }
    if cands.is_empty() {
        return vec![];
    }
    let here = f.module_at(off);
    // (`use` の中の名前 (`use a::Ty as PTy` の Ty): その use のパスで引く)
    if let Some(j) = f.use_at.iter().position(|&a| a == start) {
        let one = [(name.clone(), f.uses[j].1.clone())];
        return resolve_use(ws, file, &one, &here, &name, &cands, 0).unwrap_or_else(|| prefer_ws(ws, cands, file));
    }
    let same: Vec<(usize, &Def)> = cands.iter().filter(|(i, d)| !ws[*i].lib && d.module[..] == here[..]).cloned().collect();
    if !same.is_empty() {
        return pick(same, file);
    }
    // (use: 名前を取り込んだもの、glob で取り込んだモジュールのもの、そのモジュールが use で取り込んだもの)
    if let Some(r) = resolve_use(ws, file, &f.uses, &here, &name, &cands, 0) {
        return r;
    }
    prefer_ws(ws, cands, file)
}

/// uses (here のモジュールの use) で name を引く。depth: glob で取り込んだモジュールの use をたどった段数
fn resolve_use(ws: &[&FileIndex], file: usize, uses: &[(String, Vec<String>)], here: &[String], name: &str, cands: &[(usize, &Def)], depth: u32) -> Option<Vec<Loc>> {
    for (local, path) in uses {
        if local == name && path.len() >= 2 {
            let (m, exact) = use_module(here, &path[..path.len() - 1]);
            let c = by_use(cands, &m, exact);
            // (候補がモジュールの下に二つ以上: そのモジュールの `pub use` をたどる。std::sync::Mutex → sync::poison::mutex)
            if c.len() > 1 && !exact {
                if let Some(r) = reexport(ws, file, &m, &path[path.len() - 1], cands, 0) {
                    return Some(r);
                }
            }
            if !c.is_empty() {
                return Some(prefer_ws(ws, c, file));
            }
        }
    }
    for (local, path) in uses {
        if local == "*" && !path.is_empty() {
            let (m, exact) = use_module(here, path);
            let c = by_use(cands, &m, exact);
            if !c.is_empty() {
                return Some(prefer_ws(ws, c, file));
            }
            // (そのモジュールのファイルが use で取り込んだもの: `use super::*` で親の use も見える)
            if depth < 2 && exact {
                for w in ws.iter().filter(|w| !w.lib && w.module == m) {
                    if let Some(r) = resolve_use(ws, file, &w.uses, &m, name, cands, depth + 1) {
                        return Some(r);
                    }
                }
            }
        }
    }
    None
}

/// モジュール m (のファイル) が `use` で取り込んだ name をたどる
fn reexport(ws: &[&FileIndex], file: usize, m: &[String], name: &str, cands: &[(usize, &Def)], depth: u32) -> Option<Vec<Loc>> {
    if depth > 4 {
        return None;
    }
    for w in ws.iter().filter(|w| w.module == m) {
        for (local, path) in &w.uses {
            let (base, last) = if local == name && path.len() >= 2 {
                (&path[..path.len() - 1], path[path.len() - 1].as_str())
            } else if local == "*" && !path.is_empty() {
                (&path[..], name)
            } else {
                continue;
            };
            let (m2, exact) = use_module(m, base);
            if !exact {
                continue;
            }
            let c = by_use(cands, &m2, true);
            if !c.is_empty() {
                return Some(pick(c, file));
            }
            if let Some(r) = reexport(ws, file, &m2, last, cands, depth + 1) {
                return Some(r);
            }
        }
    }
    None
}

/// off にある識別子の参照 (同じ定義に解決される同じ名前の識別子)。include_decl なら定義の名前も入れる
pub fn references(ws: &[&FileIndex], file: usize, off: usize, include_decl: bool, load: Load) -> Vec<Loc> {
    clear_memo();
    let d = def_at(ws, file, off);
    let Some(&target) = d.first() else { return vec![] };
    let tf = ws[target.file];
    let name = match tf.defs.iter().find(|x| x.start == target.start) {
        Some(d) if tf.slim => d.name.as_str(),
        _ => &tf.text[target.start..target.end],
    };
    // (ローカルなら定義のある関数の中だけ)
    let is_local = !tf.defs.iter().any(|x| x.start == target.start && x.kind != SymKind::Impl);
    let mut out = vec![];
    // (ライブラリの索引はトークンを持たないので、宣言はここで足す)
    if include_decl && tf.lib {
        out.push(target);
    }
    for (i, &w0) in ws.iter().enumerate() {
        if is_local && i != target.file {
            continue;
        }
        if w0.lib || !w0.may_have(name) {
            continue;
        }
        // (メモリから外したファイル: 読み直す。そのファイルの中の識別子の解決には、読み直したものを使う)
        let held;
        let mut ws2: Vec<&FileIndex> = vec![];
        let (w, wsx): (&FileIndex, &[&FileIndex]) = if w0.slim {
            let Some(r) = load(i) else { continue };
            held = r;
            ws2.extend_from_slice(ws);
            ws2[i] = &held;
            (&held, &ws2)
        } else {
            (w0, ws)
        };
        let Some(ks) = w.idents.get(name) else { continue };
        for &k in ks {
            let at = w.toks[k as usize].1 as usize;
            // (macro_rules! の本体の中は名前を解決しない: rust-analyzer と同じ)
            if w.in_macro_body(at) {
                continue;
            }
            if !include_decl && i == target.file && at == target.start {
                continue;
            }
            if i == target.file && at == target.start {
                out.push(target);
                continue;
            }
            if def_at(wsx, i, at).first() == Some(&target) {
                out.push(Loc { file: i, start: at, end: (w.toks[k as usize].2 as usize) });
            }
        }
    }
    out
}

/// slim なファイル (ws の番号) を読み直す
pub type Load<'a> = &'a dyn Fn(usize) -> Option<Rc<FileIndex>>;

fn name_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// 型の書き方から参照・`dyn`・`impl`・寿命を外したもの
fn strip_ref(t: &str) -> &str {
    let mut t = t.trim();
    loop {
        let before = t;
        t = t.trim_start_matches('&').trim_start();
        if t.starts_with('\'') {
            t = t.find(' ').map(|i| t[i..].trim_start()).unwrap_or("");
        }
        for kw in ["mut ", "dyn ", "impl ", "const "] {
            if let Some(r) = t.strip_prefix(kw) {
                t = r.trim_start();
            }
        }
        if t == before {
            break;
        }
    }
    t
}

/// 型の書き方の頭の名前 (`&'a mut a::Vec<T>` → Vec、`impl Iterator<..>` → Iterator、`[T]` / `[T; N]` → `[T]`、タプル → 空)
pub fn type_head(t: &str) -> String {
    let t = strip_ref(t);
    if t.starts_with('[') {
        return "[T]".to_string();
    }
    let end = t.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')).unwrap_or(t.len());
    let p = &t[..end];
    p.rsplit("::").next().unwrap_or("").to_string()
}

/// 型の引数 (`HashMap<K, V>` → [K, V]、`[T]` / `[T; N]` → [T]、寿命は外す)
fn targs(t: &str) -> Vec<String> {
    let t = strip_ref(t);
    let (inner, sep_semi) = if let Some(r) = t.strip_prefix('[') {
        (r.strip_suffix(']').unwrap_or(r), true)
    } else {
        match t.find('<') {
            Some(i) if !t[..i].contains('(') => {
                let r = &t[i + 1..];
                // (対応する `>` まで)
                let mut d = 0;
                let mut e = r.len();
                for (j, c) in r.char_indices() {
                    match c {
                        '<' | '(' | '[' => d += 1,
                        ')' | ']' => d -= 1,
                        '>' if j > 0 && r.as_bytes()[j - 1] == b'-' => {}
                        '>' => {
                            if d == 0 {
                                e = j;
                                break;
                            }
                            d -= 1;
                        }
                        _ => {}
                    }
                }
                (&r[..e], false)
            }
            _ => return vec![],
        }
    };
    let mut out = vec![];
    let mut d = 0;
    let mut s = 0;
    let b = inner.as_bytes();
    for j in 0..=b.len() {
        let c = if j < b.len() { b[j] } else { b',' };
        match c {
            b'<' | b'(' | b'[' => d += 1,
            b'>' if j > 0 && b[j - 1] == b'-' => {}
            b'>' | b')' | b']' => d -= 1,
            b',' | b';' if d == 0 => {
                let a = inner[s..j].trim();
                if !a.is_empty() && !a.starts_with('\'') {
                    out.push(a.to_string());
                }
                s = j + 1;
                if c == b';' && sep_semi {
                    break;
                }
            }
            _ => {}
        }
    }
    out
}

/// t の中の識別子 params[i] を args[i] に置き換える
fn subst(t: &str, params: &[String], args: &[String]) -> String {
    if params.is_empty() {
        return t.to_string();
    }
    let b = t.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let w = &t[s..i];
            let prev_colon = s >= 2 && &t[s - 2..s] == "::";
            match params.iter().position(|p| p == w) {
                Some(k) if !prev_colon && k < args.len() => out.push_str(&args[k]),
                _ => out.push_str(w),
            }
        } else {
            let c = t[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// 型の別名の detail (`<K, V> = HashMap<K, V>` / `HashMap<..>`) → (型引数の名前, 右の型)
fn alias_parts(detail: &str) -> (Vec<String>, String) {
    let d = detail.trim();
    if !d.starts_with('<') {
        return (vec![], d.to_string());
    }
    let b = d.as_bytes();
    let mut depth = 0;
    let mut e = 0;
    for i in 0..b.len() {
        match b[i] {
            b'<' => depth += 1,
            b'>' if i > 0 && b[i - 1] == b'-' => {}
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    e = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let params = targs(&format!("X{}", &d[..=e])).into_iter().map(|p| p.split([':', '=', ' ']).next().unwrap_or("").to_string()).filter(|p| !p.is_empty()).collect();
    let rhs = d[e + 1..].trim_start().trim_start_matches('=').trim().to_string();
    (params, rhs)
}

/// 型の別名なら右の型にする (型引数も置き換える: `LockResult<G>` → `Result<G, PoisonError<G>>`)。Deref する包み
/// (`MutexGuard<T>` / `Box<T>` / `Rc<T>` / `Ref<T>` ..) は中身
fn expand(ws: &[&FileIndex], file: usize, t: &str) -> Vec<String> {
    let kr = ws[file].krate;
    let mut out = vec![t.to_string()];
    let mut cur = t.to_string();
    for _ in 0..4 {
        let h = type_head(&cur);
        let next = if let Some(d) = ws
            .iter()
            .enumerate()
            .filter(|(i, w)| *i == file || w.in_src || w.lib || w.krate == kr)
            .flat_map(|(_, w)| w.defs.iter())
            .find(|d| d.kind == SymKind::TypeAlias && d.name == h && d.owner.is_empty() && !d.ty.is_empty() && type_head(&d.ty) != h)
        {
            subst(&d.ty, &d.params, &targs(&cur))
        } else if matches!(h.as_str(), "MutexGuard" | "RwLockReadGuard" | "RwLockWriteGuard" | "Box" | "Rc" | "Arc" | "Ref" | "RefMut" | "ManuallyDrop") {
            match targs(&cur).into_iter().next() {
                Some(a) => a,
                None => break,
            }
        } else {
            break;
        };
        if out.contains(&next) {
            break;
        }
        out.push(next.clone());
        cur = next;
    }
    out
}

/// impl の名前 (`impl Tr for S<T>` / `impl S<T>`) の self 型 (`S<T>`)
fn impl_self_full(container: &str) -> &str {
    let n = container.strip_prefix("impl ").unwrap_or(container);
    match n.rfind(" for ") {
        Some(i) => n[i + 5..].trim(),
        None => n.trim(),
    }
}

/// 関数のシグネチャ (`fn f(..) -> T where ..`) の戻り値の型
fn ret_type(sig: &str) -> String {
    let b = sig.as_bytes();
    let mut d = 0i32;
    let mut i = 0;
    while i + 1 < b.len() {
        match b[i] {
            b'(' | b'[' | b'<' => d += 1,
            b')' | b']' => d -= 1,
            b'>' => d -= 1,
            b'-' if b[i + 1] == b'>' && d == 0 => {
                let r = &sig[i + 2..];
                let r = match r.find(" where ") {
                    Some(w) => &r[..w],
                    None => r,
                };
                return r.trim().to_string();
            }
            b'-' if b[i + 1] == b'>' => i += 1,
            _ => {}
        }
        i += 1;
    }
    String::new()
}

/// 繰り返しの要素の型 (`for x in v` の x)
fn elem_type(t: &str) -> Option<String> {
    let h = type_head(t);
    let a = targs(t);
    match h.as_str() {
        "Chars" => Some("char".to_string()),
        "Bytes" => Some("u8".to_string()),
        "CharIndices" => Some("(usize, char)".to_string()),
        "Vec" | "VecDeque" | "[T]" | "HashSet" | "BTreeSet" | "BinaryHeap" | "Iter" | "IterMut" | "IntoIter" | "Drain" | "Option" | "Range" | "Rev" | "Skip" | "Take" | "Peekable" | "Box" | "Split" | "Lines" | "SplitWhitespace" => {
            match h.as_str() {
                "Split" | "Lines" | "SplitWhitespace" => Some("&str".to_string()),
                _ => a.into_iter().next(),
            }
        }
        _ => None,
    }
}

/// `t[..]` の型 (range なら同じ並び)
fn index_type(t: &str, range: bool) -> Option<String> {
    let h = type_head(t);
    let a = targs(t);
    if h == "String" || h == "str" {
        return Some("str".to_string());
    }
    if h.ends_with("Map") {
        return a.get(1).cloned();
    }
    if h == "Vec" || h == "VecDeque" || h == "[T]" || h.ends_with("Vec") {
        let e = a.into_iter().next()?;
        return Some(if range { format!("[{e}]") } else { e });
    }
    None
}

thread_local! {
    /// owner_names の結果 (問い合わせ 1 回の間だけ: definition / references / hover / complete の入口で消す)
    static OWNERS: RefCell<FxMap<(String, u32), (Vec<String>, usize)>> = RefCell::new(FxMap::default());
}

fn clear_memo() {
    OWNERS.with(|m| m.borrow_mut().clear());
}

/// 型 o (頭の名前) のメンバを探すときの持ち主の名前: o 自身、型の別名の右、Vec → スライス・String → str (Deref の先)、
/// その型に impl・derive された trait
fn owner_names(ws: &[&FileIndex], file: usize, o: &str) -> Vec<String> {
    owner_split(ws, file, o).0
}

/// owner_names と、そのうち trait でないもの (型自身・別名・Deref の先) の数
fn owner_split(ws: &[&FileIndex], file: usize, o: &str) -> (Vec<String>, usize) {
    let kr = ws[file].krate;
    let key = (o.to_string(), kr);
    if let Some(v) = OWNERS.with(|m| m.borrow().get(&key).cloned()) {
        return v;
    }
    let vis = || ws.iter().enumerate().filter(move |(i, w)| *i == file || w.in_src || w.lib || w.krate == kr).map(|x| x.1);
    let mut v = vec![o.to_string()];
    let mut cur = o.to_string();
    for _ in 0..3 {
        let next = vis()
            .flat_map(|w| w.defs.iter())
            .find(|d| d.kind == SymKind::TypeAlias && d.name == cur && d.owner.is_empty() && !d.ty.is_empty())
            .map(|d| type_head(&d.ty))
            .filter(|n| !n.is_empty() && *n != cur);
        match next {
            Some(n) if !v.contains(&n) => {
                v.push(n.clone());
                cur = n;
            }
            _ => break,
        }
    }
    let mut extra: Vec<String> = vec![];
    for x in &v {
        match x.as_str() {
            "Vec" => extra.push("[T]".to_string()),
            "String" => extra.push("str".to_string()),
            _ => {}
        }
    }
    v.extend(extra);
    let nc = v.len();
    let mut traits: Vec<String> = vec![];
    for w in vis() {
        for (s, t) in &w.impls {
            if v.contains(s) && !v.contains(t) && !traits.contains(t) {
                traits.push(t.clone());
            }
        }
    }
    v.extend(traits);
    OWNERS.with(|m| m.borrow_mut().insert(key, (v.clone(), nc)));
    (v, nc)
}

/// 候補のうち、持ち主が型そのもの (concrete) のものがあればそれだけ (`FxMap::default` は trait の宣言より HashMap の impl)
fn prefer_concrete<'a>(c: Vec<(usize, &'a Def)>, concrete: &[String]) -> Vec<(usize, &'a Def)> {
    if c.iter().any(|x| concrete.contains(&x.1.owner)) {
        c.into_iter().filter(|x| concrete.contains(&x.1.owner) || x.1.owner.is_empty()).collect()
    } else {
        c
    }
}

/// 型 t のメンバの持ち主の名前 (別名・Deref の先も)
fn owners_of(ws: &[&FileIndex], file: usize, t: &str) -> Vec<String> {
    owners_of2(ws, file, t).0
}

/// owners_of と、そのうち trait でないもの
fn owners_of2(ws: &[&FileIndex], file: usize, t: &str) -> (Vec<String>, Vec<String>) {
    let mut v: Vec<String> = vec![];
    let mut c: Vec<String> = vec![];
    for x in expand(ws, file, t) {
        let (all, nc) = owner_split(ws, file, &type_head(&x));
        for (i, o) in all.into_iter().enumerate() {
            if i < nc && !c.contains(&o) {
                c.push(o.clone());
            }
            if !v.contains(&o) {
                v.push(o);
            }
        }
    }
    (v, c)
}

/// 型 o の (頭の名前で探した) メンバ name の型。impl の型引数を o の引数で置き換える (`Option<Sig>` の unwrap → Sig)
fn member_type(ws: &[&FileIndex], file: usize, o: &str, name: &str, kinds: &[SymKind]) -> Option<String> {
    expand(ws, file, o).iter().find_map(|t| member_type1(ws, file, t, name, kinds))
}

fn member_type1(ws: &[&FileIndex], file: usize, o: &str, name: &str, kinds: &[SymKind]) -> Option<String> {
    let os = owner_names(ws, file, &type_head(o));
    let kr = ws[file].krate;
    // (ワークスペースのものを先に。ライブラリは lib_rank の順)
    let d = ws
        .iter()
        .enumerate()
        .filter(|(i, w)| *i == file || w.in_src || w.lib || w.krate == kr)
        .flat_map(|(_, w)| w.defs.iter().map(move |d| (w.lib, d)))
        .filter(|(_, d)| d.name == name && kinds.contains(&d.kind) && os.contains(&d.owner) && !d.ty.is_empty())
        .min_by_key(|(lib, d)| (*lib, if *lib { lib_rank(d) } else { 0 }))?
        .1;
    let params = if d.container.starts_with("impl") { targs(impl_self_full(&d.container)) } else { vec![] };
    let args = targs(o);
    let t = subst(&d.ty, &params, &args);
    // (trait に書かれたメソッドの Self は受け手の型)
    Some(subst(&t, &["Self".to_string()], &[o.to_string()]))
}

/// トークン k で終わる式の型 (分かる範囲で: ローカル・self・フィールド・メソッドや関数の戻り値・構造体の式・添字・`?`)
fn expr_type(ws: &[&FileIndex], file: usize, k: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    if depth > 32 || k >= f.toks.len() {
        return None;
    }
    let kr = f.krate;
    let all = || ws.iter().enumerate().filter(move |(i, w)| *i == file || w.in_src || w.lib || w.krate == kr).flat_map(|(i, w)| w.defs.iter().map(move |d| (i, d)));
    let self_ty = |at: usize| -> Option<String> { f.enclosing_fn(at).filter(|d| !d.owner.is_empty()).map(|d| impl_self_full(&d.container).to_string()) };
    let at = f.toks[k].1 as usize;
    if f.ident(k) {
        let r = f.tok(k);
        if k >= 1 && f.ident(k - 1) && f.tok(k - 1) == "as" {
            return Some(r.to_string());
        }
        // `a.b`: フィールド
        if k >= 1 && f.sym(k - 1, b'.') && !(k >= 2 && f.sym(k - 2, b'.')) {
            let o = expr_type(ws, file, k - 2, depth + 1)?;
            return member_type(ws, file, &o, r, &[SymKind::Field]);
        }
        if r == "self" {
            return self_ty(at);
        }
        if f.after_path(k) {
            // `Q::CONST` / `Enum::Variant`
            let q = f.tok(k.checked_sub(3)?).to_string();
            let q = if q == "Self" { self_ty(at)? } else { q };
            return member_type(ws, file, &q, r, &[SymKind::Const, SymKind::Variant]).or(Some(q).filter(|_| r.starts_with(|c: char| c.is_ascii_uppercase())));
        }
        if let Some(b) = f.local(k, r) {
            return binding_type(ws, file, b, depth + 1);
        }
        // (const / static)
        return all().map(|x| x.1).find(|d| d.name == r && d.owner.is_empty() && matches!(d.kind, SymKind::Const | SymKind::Static) && !d.ty.is_empty()).map(|d| d.ty.clone());
    }
    if f.sym(k, b'?') && k >= 1 {
        let t = expr_type(ws, file, k - 1, depth + 1)?;
        return expand(ws, file, &t).iter().find_map(|t| match type_head(t).as_str() {
            "Option" | "Result" => targs(t).into_iter().next(),
            _ => None,
        });
    }
    if f.sym(k, b')') || f.sym(k, b'}') || f.sym(k, b']') {
        let close = f.text.as_bytes()[f.toks[k].1 as usize];
        let open = match close {
            b')' => b'(',
            b'}' => b'{',
            _ => b'[',
        };
        let mut d = 0;
        let mut p = k;
        let mut range = false;
        loop {
            if f.sym(p, close) {
                d += 1;
            } else if f.sym(p, open) {
                d -= 1;
                if d == 0 {
                    break;
                }
            } else if d == 1 && f.sym(p, b'.') && p >= 1 && f.sym(p - 1, b'.') {
                range = true;
            }
            if p == 0 {
                return None;
            }
            p -= 1;
        }
        let m = p.checked_sub(1)?;
        // マクロ `vec![..]` / `format!(..)`
        if f.sym(m, b'!') && m >= 1 && f.ident(m - 1) {
            return match f.tok(m - 1) {
                "vec" => Some("Vec<_>".to_string()),
                "format" => Some("String".to_string()),
                _ => None,
            };
        }
        if open == b'[' {
            // 添字 `a[i]` (前が式の終わりのとき)
            if f.ident(m) || f.sym(m, b')') || f.sym(m, b']') {
                let t = expr_type(ws, file, m, depth + 1)?;
                return expand(ws, file, &t).iter().find_map(|t| index_type(t, range));
            }
            return None;
        }
        if !f.ident(m) {
            return None;
        }
        let name = f.tok(m);
        if open == b'{' {
            // 構造体の式 `T { .. }` (大文字で始まる名前だけ: ブロックと分ける)
            if name == "Self" {
                return self_ty(at);
            }
            return Some(name.to_string()).filter(|n| n.starts_with(|c: char| c.is_ascii_uppercase()));
        }
        if m >= 1 && f.sym(m - 1, b'.') {
            let o = expr_type(ws, file, m - 2, depth + 1)?;
            return member_type(ws, file, &o, name, &[SymKind::Method]);
        }
        if f.after_path(m) && m >= 3 && f.ident(m - 3) {
            let q = f.tok(m - 3).to_string();
            let q = if q == "Self" { self_ty(at)? } else { q };
            return member_type(ws, file, &q, name, &[SymKind::Method, SymKind::Function, SymKind::Variant]).or_else(|| name.starts_with(|c: char| c.is_ascii_uppercase()).then(|| q.clone()));
        }
        if name == "Some" {
            return Some("Option<_>".to_string());
        }
        if name == "Ok" || name == "Err" {
            return Some("Result<_, _>".to_string());
        }
        if name.starts_with(|c: char| c.is_ascii_uppercase()) {
            return Some(name.to_string());
        }
        return all().map(|x| x.1).find(|d| d.name == name && d.owner.is_empty() && d.kind == SymKind::Function && !d.ty.is_empty()).map(|d| d.ty.clone());
    }
    if f.toks[k].0 == 2 && f.text.as_bytes()[f.toks[k].1 as usize] == b'"' {
        return Some("&str".to_string());
    }
    None
}

/// 文の終わり (`;` / `{` / `else`、括弧の外) の前のトークン。a から
fn expr_end(f: &FileIndex, a: usize, stop_brace: bool) -> Option<usize> {
    let n = f.toks.len();
    let mut j = a;
    let mut d = 0;
    while j < n {
        if f.sym(j, b'{') && d == 0 && stop_brace {
            break;
        }
        if f.sym(j, b'(') || f.sym(j, b'[') || f.sym(j, b'{') {
            d += 1;
        } else if f.sym(j, b')') || f.sym(j, b']') || f.sym(j, b'}') {
            if d == 0 {
                break;
            }
            d -= 1;
        } else if d == 0 && (f.sym(j, b';') || (f.ident(j) && f.tok(j) == "else")) {
            break;
        }
        j += 1;
    }
    (j > a).then(|| j - 1)
}

/// ローカルの束縛 b の型 (型注釈・初期化の式・引数の型・`for x in ..` の要素・`if let Some(x) = ..` の中身)
fn binding_type(ws: &[&FileIndex], file: usize, b: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    let n = f.toks.len();
    if f.sym(b + 1, b':') && !f.sym(b + 2, b':') {
        let a = f.toks.get(b + 2)?.1 as usize;
        // (型注釈: `=` / `,` / `)` / `;` まで)
        let mut j = b + 2;
        let mut d = 0;
        while j < n {
            if f.sym(j, b'<') || f.sym(j, b'(') || f.sym(j, b'[') {
                d += 1;
            } else if (f.sym(j, b'>') && !(j > 0 && f.sym(j - 1, b'-'))) || f.sym(j, b')') || f.sym(j, b']') {
                if d == 0 {
                    break;
                }
                d -= 1;
            } else if d == 0 && (f.sym(j, b'=') || f.sym(j, b',') || f.sym(j, b';') || f.sym(j, b'{') || f.sym(j, b'|')) {
                break;
            }
            j += 1;
        }
        let e = if j > b + 2 { f.toks[j - 1].2 as usize } else { a };
        let t = f.text[a..e].to_string();
        if type_head(&t) == "Self" {
            return f.enclosing_fn(a).filter(|d| !d.owner.is_empty()).map(|d| impl_self_full(&d.container).to_string());
        }
        return (!type_head(&t).is_empty() || strip_ref(&t).starts_with('(')).then_some(t);
    }
    // `for x in EXPR {`
    if b >= 1 && f.tok(b - 1) == "for" && f.ident(b + 1) && f.tok(b + 1) == "in" {
        let e = expr_end(f, b + 2, true)?;
        let t = expr_type(ws, file, e, depth + 1)?;
        return elem_type(&t).or_else(|| {
            // (`.iter()` などの名前で: 型の引数を追えないとき)
            None
        });
    }
    // `if let Some(x) = EXPR` / `while let Some(x) = EXPR` / `let Some(x) = EXPR else`
    if b >= 2 && f.sym(b - 1, b'(') && f.ident(b - 2) && matches!(f.tok(b - 2), "Some" | "Ok") && f.sym(b + 1, b')') && f.sym(b + 2, b'=') && !f.sym(b + 3, b'=') {
        let e = expr_end(f, b + 3, true)?;
        let t = expr_type(ws, file, e, depth + 1)?;
        return match type_head(&t).as_str() {
            "Option" | "Result" => targs(&t).into_iter().next(),
            _ => None,
        };
    }
    // `let x = EXPR;`
    if f.sym(b + 1, b'=') && !f.sym(b + 2, b'=') && b >= 1 && (f.tok(b - 1) == "let" || f.tok(b - 1) == "mut") {
        if let Some(e) = expr_end(f, b + 2, false) {
            if let Some(t) = expr_type(ws, file, e, depth + 1) {
                return Some(t);
            }
        }
    }
    f.local_type(b)
}

const PRIMS: [&str; 17] = ["u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64", "bool", "char", "str"];

/// 補完の候補
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub label: String,
    /// 定義の種類 (ローカルと予約語は None)
    pub kind: Option<SymKind>,
    pub local: bool,
    pub detail: String,
}

const KEYWORDS: [&str; 38] = [
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
    "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while",
];

/// prelude の名前 (`use` しなくても見える std のもの) と、よく使うマクロ
const PRELUDE: [&str; 31] = [
    "Option", "Some", "None", "Result", "Ok", "Err", "Vec", "String", "Box", "ToString", "ToOwned", "Clone", "Copy", "Default", "Iterator", "IntoIterator", "Extend",
    "Drop", "Fn", "FnMut", "FnOnce", "PartialEq", "Eq", "PartialOrd", "Ord", "Send", "Sync", "Sized", "From", "Into", "AsRef",
];
const MACROS: [&str; 22] = [
    "println", "print", "eprintln", "eprint", "format", "vec", "assert", "assert_eq", "assert_ne", "panic", "write", "writeln", "unreachable", "todo", "unimplemented",
    "matches", "debug_assert", "debug_assert_eq", "include_str", "concat", "stringify", "dbg",
];

/// off (byte) での補完: `x.|` (受け手の型のメンバ)、`Q::|` (型のメンバ・モジュールのアイテム)、それ以外 (ローカル・アイテム・
/// prelude・予約語)。カーソルの前の識別子の途中までで絞る。返り値は (候補, 打ち切ったか)
pub fn complete(ws: &[&FileIndex], file: usize, off: usize) -> (Vec<Item>, bool) {
    clear_memo();
    let f = ws[file];
    let b = f.text.as_bytes();
    let off = off.min(b.len());
    let mut ps = off;
    while ps > 0 && (b[ps - 1].is_ascii_alphanumeric() || b[ps - 1] == b'_' || b[ps - 1] >= 0x80) {
        ps -= 1;
    }
    let prefix = &f.text[ps..off];
    let mut q = ps;
    while q > 0 && (b[q - 1] == b' ' || b[q - 1] == b'\t') {
        q -= 1;
    }
    let kr = f.krate;
    let all = || ws.iter().enumerate().filter(move |(i, w)| *i == file || w.in_src || w.lib || w.krate == kr).flat_map(|(i, w)| w.defs.iter().map(move |d| (i, d)));
    let mut out: Vec<(u32, u8, Item)> = vec![];
    let mut dot_known = false;
    let mut seen: FxMap<String, ()> = FxMap::default();
    let mut add = |out: &mut Vec<(u32, u8, Item)>, prio: u8, name: &str, kind: Option<SymKind>, local: bool, detail: String, loose: bool| {
        let Some(sc) = crate::symbols::match_score(prefix, name) else { return };
        if !loose && sc > 2 && !prefix.is_empty() {
            return;
        }
        if seen.insert(name.to_string(), ()).is_some() {
            return;
        }
        out.push((sc, prio, Item { label: name.to_string(), kind, local, detail }));
    };
    // (直前のトークン: `.` / `::`)
    let tk = f.toks.partition_point(|t| (t.2 as usize) <= q);
    let before = tk.checked_sub(1);
    let dot = before.filter(|&k| f.sym(k, b'.') && !(k >= 1 && f.sym(k - 1, b'.')) && f.toks[k].2 as usize == q);
    let path = before.filter(|&k| k >= 2 && f.sym(k, b':') && f.sym(k - 1, b':') && f.ident(k - 2) && f.toks[k].2 as usize == q);
    if let Some(k) = dot {
        let kinds = [SymKind::Method, SymKind::Field];
        match k.checked_sub(1).and_then(|r| expr_type(ws, file, r, 0)).filter(|t| !type_head(t).is_empty() && type_head(t) != "_") {
            Some(o) => {
                dot_known = true;
                let os = owners_of(ws, file, &o);
                for (i, d) in all() {
                    if kinds.contains(&d.kind) && os.contains(&d.owner) {
                        add(&mut out, if ws[i].lib { 1 } else { 0 }, &d.name, Some(d.kind), false, d.container.to_string(), true);
                    }
                }
            }
            None => {
                // (型が分からない: ワークスペースのメソッドとフィールドの名前)
                for (i, d) in all() {
                    if !ws[i].lib && kinds.contains(&d.kind) {
                        add(&mut out, 0, &d.name, Some(d.kind), false, d.container.to_string(), false);
                    }
                }
            }
        }
    } else if let Some(k) = path {
        let q = f.tok(k - 2).to_string();
        let q = if q == "Self" { f.enclosing_owner(off).unwrap_or(q) } else { q };
        let os = owner_names(ws, file, &q);
        for (i, d) in all() {
            let mem = os.contains(&d.owner) && d.kind != SymKind::Impl;
            let in_mod = d.owner.is_empty() && d.kind != SymKind::Impl && d.module.last() == Some(&q);
            if mem || in_mod {
                add(&mut out, if ws[i].lib { 1 } else { 0 }, &d.name, Some(d.kind), false, d.container.to_string(), true);
            }
        }
        // (モジュール q が use で取り込んだ (再エクスポートした) 名前: `std::sync::Mutex`)
        for w in ws.iter().filter(|w| w.module.last() == Some(&q)) {
            for (local, _) in &w.uses {
                if local != "*" && local != "self" {
                    add(&mut out, 1, local, None, false, String::new(), true);
                }
            }
        }
    } else {
        // ローカル
        let ck = f.toks.partition_point(|t| (t.1 as usize) < ps).min(f.toks.len().saturating_sub(1));
        if !f.toks.is_empty() {
            for j in f.live(ck).into_iter().rev() {
                if f.toks[j].1 as usize != ps {
                    add(&mut out, 0, f.tok(j), None, true, String::new(), true);
                }
            }
        }
        // アイテム (ワークスペース、use したもの、prelude)
        let used: Vec<&str> = f.uses.iter().map(|u| u.0.as_str()).collect();
        for (i, d) in all() {
            if d.owner.is_empty() && ITEM_KINDS.contains(&d.kind) && (!ws[i].lib || used.contains(&d.name.as_str())) {
                add(&mut out, if ws[i].lib { 2 } else { 1 }, &d.name, Some(d.kind), false, d.container.to_string(), false);
            }
        }
        for u in &used {
            if *u != "*" && *u != "self" {
                add(&mut out, 2, u, None, false, String::new(), false);
            }
        }
        for p in PRELUDE {
            add(&mut out, 2, p, None, false, String::new(), false);
        }
        for m in MACROS {
            add(&mut out, 3, m, Some(SymKind::Macro), false, String::new(), false);
        }
        for kw in KEYWORDS {
            add(&mut out, 3, kw, None, false, "keyword".to_string(), false);
        }
    }
    out.sort_by(|a, b| (a.0, a.1, a.2.label.len(), &a.2.label).cmp(&(b.0, b.1, b.2.label.len(), &b.2.label)));
    // (受け手の型が分かったメンバは全部、ほかは 200 個まで)
    let cap = if dot_known { 2000 } else { 200 };
    let cut = out.len() > cap;
    out.truncate(cap);
    (out.into_iter().map(|x| x.2).collect(), cut)
}

/// off の識別子の hover: 定義のシグネチャ (と囲むもの・doc コメント)、ローカルなら束縛と型
pub fn hover(ws: &[&FileIndex], file: usize, off: usize, load: Load) -> Option<String> {
    clear_memo();
    // (組み込みの型)
    if let Some(k) = ws[file].tok_at(off) {
        let w = ws[file].tok(k);
        if PRIMS.contains(&w) {
            return Some(format!("```rust\n{w}\n```"));
        }
    }
    let d = def_at(ws, file, off);
    let t = *d.first()?;
    let tf = ws[t.file];
    let held;
    let text: &str = if tf.slim {
        held = load(t.file)?;
        &held.text
    } else {
        &tf.text
    };
    if let Some(def) = tf.defs.iter().find(|x| x.start == t.start && x.kind != SymKind::Impl) {
        let head = if !def.owner.is_empty() {
            def.container.to_string()
        } else {
            def.module.join("::")
        };
        let sig = signature(text.get(def.item_start..def.item_end.min(text.len()))?, def.kind);
        let docs = doc_comment(text, def.item_start);
        let mut s = String::from("```rust\n");
        if !head.is_empty() {
            s.push_str(&head);
            s.push('\n');
        }
        s.push_str(&sig);
        s.push_str("\n```");
        if !docs.is_empty() {
            s.push_str("\n\n---\n\n");
            s.push_str(&docs);
        }
        return Some(s);
    }
    // ローカル
    let name = text.get(t.start..t.end)?;
    let b = tf.toks.partition_point(|x| (x.1 as usize) < t.start);
    let ty = if b < tf.toks.len() { binding_type(ws, t.file, b, 0) } else { None };
    Some(match ty {
        Some(ty) => format!("```rust\n{name}: {ty}\n```"),
        None => format!("```rust\n{name}\n```"),
    })
}

/// アイテムの本文からシグネチャ (属性を外し、本体の `{` の前まで。空白はまとめる)
fn signature(item: &str, kind: SymKind) -> String {
    let mut s = item.trim_start();
    while s.starts_with("#[") || s.starts_with("#![") {
        let mut d = 0;
        let mut cut = s.len();
        for (i, c) in s.char_indices() {
            if c == '[' {
                d += 1;
            } else if c == ']' {
                d -= 1;
                if d == 0 {
                    cut = i + 1;
                    break;
                }
            }
        }
        s = s[cut..].trim_start();
    }
    let body = matches!(kind, SymKind::Function | SymKind::Method | SymKind::Struct | SymKind::Enum | SymKind::Union | SymKind::Trait | SymKind::Module | SymKind::Impl | SymKind::Macro);
    let mut end = s.len();
    if body {
        let mut d = 0i32;
        for (i, c) in s.char_indices() {
            match c {
                '(' | '[' | '<' => d += 1,
                ')' | ']' | '>' => d -= 1,
                '{' if d <= 0 => {
                    end = i;
                    break;
                }
                ';' if d <= 0 => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
    } else if kind == SymKind::Const || kind == SymKind::Static {
        end = s.find(';').unwrap_or(s.len());
    }
    let mut out = String::new();
    for w in s[..end].split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    if out.len() > 400 {
        let mut c = 400;
        while !out.is_char_boundary(c) {
            c -= 1;
        }
        out.truncate(c);
        out.push_str(" …");
    }
    out
}

/// at (アイテムの始まり) の前の `///` の行
fn doc_comment(text: &str, at: usize) -> String {
    let mut lines: Vec<&str> = vec![];
    let head = &text[..text[..at.min(text.len())].rfind('\n').map(|i| i + 1).unwrap_or(0)];
    for l in head.lines().rev() {
        let t = l.trim();
        if let Some(d) = t.strip_prefix("///") {
            lines.push(d.strip_prefix(' ').unwrap_or(d));
        } else if t.starts_with("#[") {
            continue;
        } else {
            break;
        }
    }
    lines.reverse();
    lines.join("\n")
}

/// ワークスペースの根からの相対パス → モジュールのパス (`crates/x/src/a/b.rs` → [a, b]、`src/a/mod.rs` → [a])
pub fn module_of(rel: &str) -> Vec<String> {
    let parts: Vec<&str> = rel.split('/').collect();
    let from = parts.iter().rposition(|p| *p == "src").map(|i| i + 1).unwrap_or(0);
    let mut m: Vec<String> = parts[from..].iter().map(|s| s.to_string()).collect();
    if let Some(l) = m.pop() {
        let l = l.trim_end_matches(".rs");
        if !matches!(l, "mod" | "lib" | "main") {
            m.push(l.to_string());
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(src: &str, pat: &str, nth: usize) -> usize {
        src.match_indices(pat).nth(nth).unwrap().0
    }

    fn def_text(ws: &[&FileIndex], file: usize, off: usize) -> Option<(usize, String, usize)> {
        let d = definition(ws, file, off);
        d.first().map(|l| (l.file, ws[l.file].text[l.start..l.end].to_string(), l.start))
    }

    #[test]
    fn locals_paths_methods() {
        let src = "struct P { x: i64 }\nimpl P {\n    fn new(x: i64) -> P { P { x } }\n    fn get(&self) -> i64 { self.x }\n    fn twice(&self) -> i64 { self.get() * 2 }\n}\n\
                   fn f(a: i64) -> i64 {\n    let p = P::new(a);\n    let a = a + 1;\n    for i in 0..3 { let _ = i; }\n    match Some(a) { Some(v) => v, None => p.get() }\n}\n";
        let f = FileIndex::new(src.to_string(), vec![]);
        let ws = [&f];
        // P::new → impl P の new
        assert_eq!(def_text(&ws, 0, at(src, "new(a)", 0)).unwrap().2, at(src, "new(x", 0));
        // self.get() → impl P の get、self.x → フィールド x
        assert_eq!(def_text(&ws, 0, at(src, "get() * 2", 0)).unwrap().2, at(src, "get(&self)", 0));
        assert_eq!(def_text(&ws, 0, at(src, "x }\n    fn twice", 0) + 0).unwrap().2, at(src, "x: i64 }", 0));
        // `let a = a + 1` の右の a は引数の a、`Some(a)` の a は let の a
        assert_eq!(def_text(&ws, 0, at(src, "a + 1", 0)).unwrap().2, at(src, "a: i64) -> i64 {\n    let p", 0));
        assert_eq!(def_text(&ws, 0, at(src, "a) { Some", 0)).unwrap().2, at(src, "a = a + 1", 0));
        // for の i、match の腕の v、p.get() (p の型は P::new から)
        assert_eq!(def_text(&ws, 0, at(src, "i; }", 0)).unwrap().2, at(src, "i in", 0));
        assert_eq!(def_text(&ws, 0, at(src, "v,", 0)).unwrap().2, at(src, "v) =>", 0));
        assert_eq!(def_text(&ws, 0, at(src, "get() }\n}", 0)).unwrap().2, at(src, "get(&self)", 0));
        // 参照: get は定義と 2 か所
        assert_eq!(references(&ws, 0, at(src, "get(&self)", 0), true, &|_| None).len(), 3);
        assert_eq!(references(&ws, 0, at(src, "get(&self)", 0), false, &|_| None).len(), 2);
    }

    #[test]
    fn across_files_and_uses() {
        let a = "pub fn helper() -> u32 { 1 }\npub struct Thing;\n".to_string();
        let b = "use crate::util::helper;\nfn helper2() {}\nfn main() { helper(); let t: Thing = Thing; }\n".to_string();
        let fa = FileIndex::new(a, module_of("src/util.rs"));
        let fb = FileIndex::new(b.clone(), module_of("src/main.rs"));
        let ws = [&fa, &fb];
        assert_eq!(def_text(&ws, 1, at(&b, "helper();", 0)).map(|x| (x.0, x.1)), Some((0, "helper".to_string())));
        assert_eq!(def_text(&ws, 1, at(&b, "Thing;", 0)).map(|x| x.0), Some(0));
        // (定義・use・呼び出し: rust-analyzer と同じく use も参照)
        assert_eq!(references(&ws, 0, 7, true, &|_| None).len(), 3);
        assert_eq!(module_of("crates/x/src/a/mod.rs"), vec!["a".to_string()]);
    }
}
