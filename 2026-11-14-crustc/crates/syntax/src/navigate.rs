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
    pub container: String,
    /// 囲むモジュール (ファイルのモジュールのパスの後ろに、ファイルの中の `mod x { .. }`)
    pub module: Vec<String>,
    /// 名前と、アイテム全体の byte 範囲
    pub start: usize,
    pub end: usize,
    pub item_start: usize,
    pub item_end: usize,
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
    /// ライブラリのファイル: 本文を捨てるので、定義の名前の両端の (行, UTF-16 の桁)
    lib_pos: FxMap<usize, (u32, u32)>,
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
        fn walk(v: &[Symbol], owner: &str, container: &str, module: &mut Vec<String>, out: &mut Vec<Def>) {
            for s in v {
                {
                    out.push(Def {
                        name: s.name.clone(),
                        kind: s.kind,
                        owner: if s.kind == SymKind::Impl { String::new() } else { owner.to_string() },
                        container: container.to_string(),
                        module: module.clone(),
                        start: s.name_start,
                        end: s.name_end,
                        item_start: s.start,
                        item_end: s.end,
                    });
                }
                match s.kind {
                    SymKind::Impl => walk(&s.children, &impl_self(&s.name), &s.name, module, out),
                    SymKind::Module => {
                        module.push(s.name.clone());
                        walk(&s.children, "", &s.name, module, out);
                        module.pop();
                    }
                    _ => walk(&s.children, &s.name, &s.name, module, out),
                }
            }
        }
        let mut m = module.clone();
        walk(&syms, "", "", &mut m, &mut defs);
        let mut idents: FxMap<String, Vec<u32>> = FxMap::default();
        for (i, t) in toks.iter().enumerate() {
            if t.0 == 0 {
                idents.entry(text[t.1 as usize..t.2 as usize].to_string()).or_default().push(i as u32);
            }
        }
        let mut f = FileIndex { text, lib: false, in_src: true, krate: 0, module, defs, toks, idents, uses: vec![], use_at: vec![], macros: vec![], lib_pos: FxMap::default() };
        (f.uses, f.use_at) = f.collect_uses().into_iter().map(|(a, b, c)| ((a, b), c)).unzip();
        f.macros = f.macro_bodies();
        f
    }

    /// ライブラリのファイル: 定義と use だけ (本文・字句の列・名前の表は捨てる。位置は先に行と桁にしておく)
    pub fn library(text: String, module: Vec<String>) -> FileIndex {
        let mut f = FileIndex::new(text, module);
        f.lib = true;
        f.toks = vec![];
        f.idents = FxMap::default();
        f.macros = vec![];
        f.use_at = vec![];
        let li = crate::symbols::LineIndex::new(&f.text);
        for d in &f.defs {
            for at in [d.start, d.end] {
                f.lib_pos.entry(at).or_insert_with(|| li.pos(&f.text, at));
            }
        }
        f.text = String::new();
        f.uses.shrink_to_fit();
        f.defs.shrink_to_fit();
        f
    }

    /// ライブラリのファイルの定義の名前の端の (行, UTF-16 の桁)
    pub fn lib_pos(&self, at: usize) -> Option<(u32, u32)> {
        self.lib_pos.get(&at).copied()
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

    fn collect_uses(&self) -> Vec<(String, Vec<String>, usize)> {
        let mut out = vec![];
        let n = self.toks.len();
        let mut k = 0;
        while k < n {
            if self.tok(k) == "use" && (k == 0 || self.sym(k - 1, b';') || self.sym(k - 1, b'}') || self.sym(k - 1, b'{') || self.sym(k - 1, b']') || self.tok(k - 1) == "pub" || self.sym(k - 1, b')')) {
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
            m = b.module.clone();
            m.push(b.name.clone());
        }
        m
    }

    /// トークン k (名前 name) のローカルの束縛 (トークンの番号)。関数の中で、カーソルより前でいちばん近いもの
    fn local(&self, k: usize, name: &str) -> Option<usize> {
        let f = self.enclosing_fn(self.toks[k].1 as usize)?;
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
        live.iter().rev().map(|l| l.0).find(|&j| self.tok(j) == name && j != k)
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
    let score = |d: &Def| d.module.len() + if d.module.first().is_some_and(|m| m == "sys" || m == "os" || m.starts_with("sys_")) { 100 } else { 0 };
    let best = l.iter().map(|x| score(x.1)).min().unwrap();
    pick(l.into_iter().filter(|x| score(x.1) == best).collect(), file)
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
    let scored: Vec<(usize, (usize, &'a Def))> = cands.iter().filter_map(|c| mod_score(&c.1.module, target, exact).map(|s| (s, *c))).collect();
    let Some(best) = scored.iter().map(|x| x.0).min() else { return vec![] };
    scored.into_iter().filter(|x| x.0 == best).map(|x| x.1).collect()
}

/// off (byte) にある識別子の定義
pub fn definition(ws: &[&FileIndex], file: usize, off: usize) -> Vec<Loc> {
    let f = ws[file];
    let Some(k) = f.tok_at(off) else { return vec![] };
    let name = f.tok(k).to_string();
    let start = f.toks[k].1 as usize;
    // (組み込みの型には定義がない)
    if matches!(name.as_str(), "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "f32" | "f64" | "bool" | "char" | "str") {
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
        let owner = if k >= 2 && f.ident(k - 2) {
            let r = f.tok(k - 2);
            if r == "self" {
                f.enclosing_owner(off)
            } else {
                f.local(k - 2, r).and_then(|b| f.local_type(b))
            }
        } else {
            None
        };
        let c: Vec<(usize, &Def)> = all().filter(|(_, d)| d.name == name && kinds.contains(&d.kind) && owner.as_ref().is_none_or(|o| &d.owner == o)).collect();
        if owner.is_none() {
            // (受け手の型が分からないときは、ライブラリも含めて一つだけのときだけ)
            return pick(c, file);
        }
        return prefer_ws(ws, c, file);
    }
    // パス `Q::name`
    if f.after_path(k) && k >= 3 && f.ident(k - 3) {
        let q = f.tok(k - 3).to_string();
        let q = if q == "Self" { f.enclosing_owner(off).unwrap_or(q) } else { q };
        if !matches!(q.as_str(), "crate" | "super" | "self") {
            // (型のメンバか、モジュールの中のアイテム)
            let c: Vec<(usize, &Def)> = all().filter(|(_, d)| d.name == name && (d.owner == q || (d.owner.is_empty() && d.module.last() == Some(&q)))).collect();
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
    let same: Vec<(usize, &Def)> = cands.iter().filter(|(i, d)| !ws[*i].lib && d.module == here).cloned().collect();
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
pub fn references(ws: &[&FileIndex], file: usize, off: usize, include_decl: bool) -> Vec<Loc> {
    let d = definition(ws, file, off);
    let Some(&target) = d.first() else { return vec![] };
    let tf = ws[target.file];
    let name = match tf.defs.iter().find(|x| x.start == target.start) {
        Some(d) if tf.lib => d.name.as_str(),
        _ => &tf.text[target.start..target.end],
    };
    // (ローカルなら定義のある関数の中だけ)
    let is_local = !tf.defs.iter().any(|x| x.start == target.start && x.kind != SymKind::Impl);
    let mut out = vec![];
    // (ライブラリの索引はトークンを持たないので、宣言はここで足す)
    if include_decl && tf.lib {
        out.push(target);
    }
    for (i, w) in ws.iter().enumerate() {
        if is_local && i != target.file {
            continue;
        }
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
            if definition(ws, i, at).first() == Some(&target) {
                out.push(Loc { file: i, start: at, end: (w.toks[k as usize].2 as usize) });
            }
        }
    }
    out
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
        assert_eq!(references(&ws, 0, at(src, "get(&self)", 0), true).len(), 3);
        assert_eq!(references(&ws, 0, at(src, "get(&self)", 0), false).len(), 2);
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
        assert_eq!(references(&ws, 0, 7, true).len(), 3);
        assert_eq!(module_of("crates/x/src/a/mod.rs"), vec!["a".to_string()]);
    }
}
