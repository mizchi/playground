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
use crustc_syntax::fxhash::{FxMap, FxSet};
use crustc_syntax::lexer::tokenize;
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
    /// 関数・メソッドの引数のうちクロージャを取るもの: (引数の番号 (self を除く), 型引数の名前 (`F`、`impl Fn` なら空),
    /// クロージャの戻り値の形 (`U` / `Option<B>`), クロージャの引数の形 (`T` / `&Self::Item` / `B, Self::Item`))。
    /// `fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Option<U>` → [(0, F, U, T)]
    pub fnret: Vec<(u8, String, String, String)>,
    /// メソッドが self を取るか (`.` の後の補完に出すもの)
    pub has_self: bool,
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
    /// workspace/symbol に出す use: 別名 `use a::B as C` (C) と再エクスポート `pub use a::B` (B)。
    /// (名前, 行, UTF-16 の桁, 名前の長さ (UTF-16)) (slim にしても残す)
    pub use_syms: Vec<(String, u32, u32, u32)>,
    /// 本文と字句の列を捨てたもの (ライブラリと、メモリから外したワークスペースのファイル)。定義と use と名前の集合だけ
    pub slim: bool,
    /// (self 型の頭, trait の頭): `impl Tr for S` と `#[derive(Tr)] struct S`
    pub impls: Vec<(String, String)>,
    /// `#[derive(Tr)] struct S` の (S, Tr, Tr の名前の byte 位置)
    pub derives: Vec<(String, String, u32)>,
    /// cfg で外れたアイテムの byte 範囲 (apply_cfg で決める。中の定義は defs から除く)
    inactive: Vec<(usize, usize)>,
    /// apply_cfg に渡した有効な feature (読み直したときにもう一度当てる)
    features: Option<Vec<String>>,
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

/// 文字列のリテラルの中の `{name}` / `{name:..}` (書式の引数の取り込み) の (リテラルの中の byte 位置, 名前)
fn captures(lit: &str) -> Vec<(usize, &str)> {
    let b = lit.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'{' {
            if i + 1 < b.len() && b[i + 1] == b'{' {
                i += 2;
                continue;
            }
            let a = i + 1;
            let mut e = a;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                e += 1;
            }
            if e > a && e < b.len() && (b[e] == b'}' || b[e] == b':') && !b[a].is_ascii_digit() {
                out.push((a, &lit[a..e]));
            }
            i = e;
            continue;
        }
        i += 1;
    }
    out
}

/// 名前の後 (byte at から) の型引数の宣言 `<'a, T: X, const N: usize>` の型の引数の名前 ([T, N])
fn generic_names(text: &str, at: usize) -> Vec<String> {
    let rest = text.get(at..).unwrap_or("");
    let r = rest.trim_start();
    if !r.starts_with('<') {
        return vec![];
    }
    let mut d = 0;
    let mut end = r.len();
    for (i, c) in r.char_indices() {
        match c {
            '<' => d += 1,
            '>' if i > 0 && r.as_bytes()[i - 1] != b'-' => {
                d -= 1;
                if d == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    targs(&format!("X{}>", &r[..end]))
        .into_iter()
        .filter(|p| !p.starts_with('\''))
        .map(|p| p.trim_start_matches("const ").split([':', '=', ' ']).next().unwrap_or("").to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// 関数のシグネチャ `fn m<B, F: FnMut(T) -> B>(self, x: u8, f: F) -> R where F: ..` の、クロージャを取る引数
/// (引数の番号 (self を除く), 型引数の名前, クロージャの戻り値の形)
fn fn_bounds(sig: &str) -> Vec<(u8, String, String, String)> {
    // (型引数 `<..>` と where の中の `X: Fn*(..) -> R` / `X: Fn*(..)`: (X, 戻り値, 引数))
    let mut bounds: Vec<(String, String, String)> = vec![];
    let fn_ret = |b: &str| -> Option<(String, String)> {
        let b = b.trim().trim_start_matches("[const]").trim_start_matches("~const").trim();
        let rest = ["FnOnce", "FnMut", "Fn"].iter().find_map(|k| b.strip_prefix(k))?;
        let rest = rest.trim_start();
        if !rest.starts_with('(') {
            return None;
        }
        // (対応する `)` の後の `-> R`)
        let mut d = 0;
        let mut e = None;
        for (i, c) in rest.char_indices() {
            match c {
                '(' | '<' | '[' => d += 1,
                ')' | ']' => {
                    d -= 1;
                    if d == 0 {
                        e = Some(i + 1);
                        break;
                    }
                }
                '>' if i > 0 && rest.as_bytes()[i - 1] != b'-' => d -= 1,
                _ => {}
            }
        }
        // (閉じていない (長いシグネチャを `…` で切ったもの) なら分からない)
        let e = e?;
        let args = rest.get(1..e - 1)?.trim().to_string();
        let after = rest[e..].trim_start();
        Some((args, match after.strip_prefix("->") {
            Some(r) => {
                // (戻り値の型は次の `+` / `,` (深さ 0) まで)
                let r = r.trim();
                let mut d = 0;
                let mut end = r.len();
                for (i, c) in r.char_indices() {
                    match c {
                        '<' | '(' | '[' => d += 1,
                        '>' if i > 0 && r.as_bytes()[i - 1] != b'-' => d -= 1,
                        ')' | ']' => d -= 1,
                        '+' | ',' if d == 0 => {
                            end = i;
                            break;
                        }
                        _ => {}
                    }
                }
                r[..end].trim().to_string()
            }
            None => "()".to_string(),
        }))
    };
    let split_top = |s: &str| -> Vec<String> {
        let mut out = vec![];
        let mut d = 0;
        let mut a = 0;
        let b = s.as_bytes();
        for i in 0..b.len() {
            match b[i] {
                b'<' | b'(' | b'[' => d += 1,
                b'>' if i > 0 && b[i - 1] != b'-' => d -= 1,
                b')' | b']' => d -= 1,
                b',' if d == 0 => {
                    out.push(s[a..i].trim().to_string());
                    a = i + 1;
                }
                _ => {}
            }
        }
        if a < s.len() && !s[a..].trim().is_empty() {
            out.push(s[a..].trim().to_string());
        }
        out
    };
    let Some(name_end) = sig.find(|c: char| c == '<' || c == '(') else { return vec![] };
    let mut rest = &sig[name_end..];
    let mut gnames: Vec<String> = vec![];
    if rest.starts_with('<') {
        let g = targs(&format!("X{rest}"));
        for p in &g {
            let n = p.split([':', '=']).next().unwrap_or("").trim();
            if !n.is_empty() && !n.starts_with('\'') && !n.starts_with("const ") {
                gnames.push(n.to_string());
            }
        }
        for p in g {
            if let Some((n, b)) = p.split_once(':') {
                for part in b.split('+') {
                    if let Some((a, r)) = fn_ret(part) {
                        bounds.push((n.trim().to_string(), r, a));
                    }
                }
            }
        }
        // (`>` の後の `(`)
        let mut d = 0;
        let mut cut = rest.len();
        for (i, c) in rest.char_indices() {
            match c {
                '<' => d += 1,
                '>' if i > 0 && rest.as_bytes()[i - 1] != b'-' => {
                    d -= 1;
                    if d == 0 {
                        cut = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = rest[cut..].trim_start();
    }
    if let Some(w) = sig.rfind(" where ") {
        for p in split_top(&sig[w + 7..]) {
            if let Some((n, b)) = p.split_once(':') {
                for part in b.split('+') {
                    if let Some((a, r)) = fn_ret(part) {
                        bounds.push((n.trim().to_string(), r, a));
                    }
                }
            }
        }
    }
    // (引数の並び)
    let Some(inner) = rest.strip_prefix('(') else { return vec![] };
    let mut d = 0;
    let mut e = inner.len();
    for (i, c) in inner.char_indices() {
        match c {
            '(' | '[' | '<' => d += 1,
            '>' if i > 0 && inner.as_bytes()[i - 1] != b'-' => d -= 1,
            ']' => d -= 1,
            ')' => {
                if d == 0 {
                    e = i;
                    break;
                }
                d -= 1;
            }
            _ => {}
        }
    }
    let mut out = vec![];
    let mut idx = 0u8;
    for p in split_top(&inner[..e]) {
        let Some((_, ty)) = p.split_once(':') else {
            // (`self` / `&self` / `mut self`)
            continue;
        };
        if p.trim_start().trim_start_matches('&').trim_start_matches("mut ").trim_start().starts_with("self") && !p.contains("self:") {
            continue;
        }
        if p.trim_start().starts_with("self:") || p.trim_start().starts_with("mut self:") {
            continue;
        }
        let ty = ty.trim();
        if let Some((a, r)) = ty.strip_prefix("impl ").and_then(|b| b.split('+').find_map(|x| fn_ret(x))) {
            out.push((idx, String::new(), r, a));
        } else if let Some((_, r, a)) = bounds.iter().find(|(n, _, _)| n == ty.trim_start_matches("&mut ").trim_start_matches('&')) {
            out.push((idx, ty.to_string(), r.clone(), a.clone()));
        } else if gnames.iter().any(|g| g == ty) {
            // (`other: U` のような型引数そのものの引数: 呼び出しの引数の式の型で決まる。印は "=")
            out.push((idx, ty.to_string(), "=".to_string(), String::new()));
        }
        idx += 1;
    }
    out
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
        fn walk(text: &str, v: &[Symbol], owner: &str, container: &Arc<str>, module: &Arc<[String]>, outer: &[String], out: &mut Vec<Def>) {
            for s in v {
                // (`extern "C" { .. }` のブロック: 中の宣言はそのままこの階層のアイテム)
                if s.kind == SymKind::Extern {
                    walk(text, &s.children, owner, container, module, outer, out);
                    continue;
                }
                // (型引数の名前: 型の別名はその型引数、struct / enum / union はその型引数、フィールド・バリアントは持ち主の型の)
                let params = match s.kind {
                    SymKind::TypeAlias => alias_parts(&s.detail).0,
                    SymKind::Struct | SymKind::Enum | SymKind::Union => generic_names(text, s.name_end),
                    SymKind::Field | SymKind::Variant => outer.to_vec(),
                    _ => vec![],
                };
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
                        params: params.clone(),
                        fnret: if matches!(s.kind, SymKind::Function | SymKind::Method) { fn_bounds(&s.detail) } else { vec![] },
                        has_self: s.kind == SymKind::Method && crate::symbols::takes_self(&s.detail),
                        pos: [0; 4],
                    });
                }
                if s.children.is_empty() {
                    continue;
                }
                let c: Arc<str> = Arc::from(s.name.as_str());
                match s.kind {
                    SymKind::Impl => walk(text, &s.children, &impl_self(&s.name), &c, module, &[], out),
                    SymKind::Module => {
                        let mut m = module.to_vec();
                        m.push(s.name.clone());
                        walk(text, &s.children, "", &c, &Arc::from(m), &[], out);
                    }
                    // (関数の本体の中のアイテム: 持ち主のないアイテム)
                    SymKind::Function | SymKind::Method => walk(text, &s.children, "", &c, module, &[], out),
                    _ => walk(text, &s.children, &s.name, &c, module, &params, out),
                }
            }
        }
        walk(&text, &syms, "", &Arc::from(""), &Arc::from(module.clone()), &[], &mut defs);
        let mut idents: FxMap<String, Vec<u32>> = FxMap::default();
        for (i, t) in toks.iter().enumerate() {
            if t.0 == 0 {
                idents.entry(text[t.1 as usize..t.2 as usize].to_string()).or_default().push(i as u32);
            }
        }
        let mut f = FileIndex { text, lib: false, in_src: true, krate: 0, module, defs, toks, idents, uses: vec![], use_at: vec![], macros: vec![], use_syms: vec![], slim: false, names: vec![], impls: vec![], derives: vec![], inactive: vec![], features: None };
        (f.uses, f.use_at) = f.collect_uses(false).into_iter().map(|(a, b, c)| ((a, b), c)).unzip();
        f.use_syms = f.collect_use_syms();
        f.macros = f.macro_bodies();
        f.derives = f.collect_derives();
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
                    // (宣言の本体 `enum E {` / `struct S {` / `impl T {` / `trait T {` / `impl X for T {` は構造体の式ではない)
                    let decl = j >= 2 && self.ident(j - 2) && matches!(self.tok(j - 2), "enum" | "struct" | "union" | "trait" | "impl" | "for" | "mod" | "dyn");
                    return (j >= 1 && !decl && self.ident(j - 1) && self.tok(j - 1).starts_with(|c: char| c.is_ascii_uppercase())).then(|| self.tok(j - 1).to_string());
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
                    // (`impl<T: X> Tr for T` のような全部への impl (ブランケット) は "*" に: どの型のメンバにも Tr のメソッドが入る)
                    if !s.is_empty() && !t.is_empty() {
                        let blanket = s.len() == 1 && s.starts_with(|c: char| c.is_ascii_uppercase());
                        out.push((if blanket { "*".to_string() } else { s }, t));
                    }
                }
            }
        }
        for (t, tr, _) in &self.derives {
            out.push((t.clone(), tr.clone()));
        }
        out
    }

    /// `#[derive(A, B)]` の後の struct / enum / union と、derive の名前の位置
    fn collect_derives(&self) -> Vec<(String, String, u32)> {
        let mut out = vec![];
        let n = self.toks.len();
        let mut k = 0;
        while k < n {
            if self.ident(k) && self.tok(k) == "derive" && self.sym(k + 1, b'(') && k >= 2 && self.sym(k - 1, b'[') && !self.in_inactive(self.toks[k].1 as usize) {
                let mut j = k + 2;
                let mut names = vec![];
                while j < n && !self.sym(j, b')') {
                    if self.ident(j) && !self.sym(j + 1, b':') {
                        names.push((self.tok(j).to_string(), self.toks[j].1));
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
                    for (t, at) in names {
                        out.push((s.clone(), t, at));
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
        // (ほかのファイルから見えるのは `pub use` (再エクスポート) だけ。字句の列を捨てる前に集める)
        f.uses = f.collect_uses(true).into_iter().map(|(a, b, _)| (a, b)).collect();
        f.toks = vec![];
        f.idents = FxMap::default();
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

    /// ライブラリのファイルの本文だけを持つもの (hover / signatureHelp がシグネチャと doc コメントを読むのに使う。定義は
    /// もとの索引のものを使うので、字句の列も定義も作らない)
    pub fn text_only(&self, text: String) -> FileIndex {
        FileIndex { text, lib: self.lib, in_src: self.in_src, krate: self.krate, module: self.module.clone(), defs: vec![], toks: vec![], idents: FxMap::default(), uses: vec![], use_at: vec![], macros: vec![], use_syms: vec![], slim: false, names: vec![], impls: vec![], derives: vec![], inactive: vec![], features: None }
    }

    /// slim なファイルを本文から作り直したもの (モジュール・クレートは同じ)
    pub fn reload(&self, text: String) -> FileIndex {
        let mut f = FileIndex::new(text, self.module.clone());
        f.lib = self.lib;
        f.in_src = self.in_src;
        f.krate = self.krate;
        if let Some(fs) = &self.features {
            f.apply_cfg(fs.clone());
        }
        f
    }

    /// `#[cfg(..)]` で外れるアイテムを除く (features: そのクレートで有効な feature)。slim にする前に呼ぶ。
    /// 分かるのは feature = ".." / test / debug_assertions / unix / windows / target_os / target_arch などと all / any / not。
    /// ほかの cfg は (rustc と同じく) 成り立たないとする
    pub fn apply_cfg(&mut self, features: Vec<String>) {
        if self.toks.is_empty() && !self.text.is_empty() {
            return;
        }
        let mut inactive: Vec<(usize, usize)> = vec![];
        for d in &self.defs {
            if inactive.iter().any(|&(a, b)| a <= d.item_start && d.item_end <= b) {
                continue;
            }
            // (アイテムの始まりから名前までの属性)
            let a = self.toks.partition_point(|t| (t.1 as usize) < d.item_start);
            let e = self.toks.partition_point(|t| (t.1 as usize) < d.start);
            let mut k = a;
            while k + 3 < e {
                if self.sym(k, b'#') && self.sym(k + 1, b'[') && self.tok(k + 2) == "cfg" && self.sym(k + 3, b'(') {
                    let mut j = k + 4;
                    if !self.cfg_pred(&mut j, &features) {
                        inactive.push((d.item_start, d.item_end));
                        break;
                    }
                }
                k += 1;
            }
        }
        self.inactive = inactive;
        if !self.inactive.is_empty() {
            let ina = std::mem::take(&mut self.inactive);
            self.defs.retain(|d| !ina.iter().any(|&(a, b)| a <= d.item_start && d.item_end <= b));
            self.inactive = ina;
            self.derives = self.collect_derives();
            self.impls = self.collect_impls();
        }
        self.features = Some(features);
    }

    /// cfg の述語 (トークン *j から。読んだ後へ進める)
    fn cfg_pred(&self, j: &mut usize, fs: &[String]) -> bool {
        if !self.ident(*j) {
            return true;
        }
        let w = self.tok(*j).to_string();
        *j += 1;
        if self.sym(*j, b'(') {
            *j += 1;
            let mut vals = vec![];
            while *j < self.toks.len() && !self.sym(*j, b')') {
                vals.push(self.cfg_pred(j, fs));
                if self.sym(*j, b',') {
                    *j += 1;
                }
            }
            *j += 1;
            return match w.as_str() {
                "all" => vals.iter().all(|&v| v),
                "any" => vals.iter().any(|&v| v),
                "not" => !vals.first().copied().unwrap_or(true),
                _ => true,
            };
        }
        if self.sym(*j, b'=') && *j + 1 < self.toks.len() && self.toks[*j + 1].0 == 2 {
            let v = self.tok(*j + 1).trim_matches('"').to_string();
            *j += 2;
            return match w.as_str() {
                "feature" => fs.contains(&v),
                "target_os" => v == std::env::consts::OS,
                "target_family" => v == std::env::consts::FAMILY,
                "target_arch" => v == std::env::consts::ARCH,
                "target_pointer_width" => v == (usize::BITS).to_string(),
                "target_endian" => v == if cfg!(target_endian = "little") { "little" } else { "big" },
                "target_env" => v == if cfg!(target_env = "gnu") { "gnu" } else if cfg!(target_env = "musl") { "musl" } else if cfg!(target_env = "msvc") { "msvc" } else { "" },
                "target_vendor" => v == if cfg!(target_vendor = "apple") { "apple" } else if cfg!(target_vendor = "pc") { "pc" } else { "unknown" },
                "target_has_atomic" => matches!(v.as_str(), "8" | "16" | "32" | "64" | "ptr"),
                "panic" => v == "unwind",
                // (rustc と同じく、設定されていない cfg は成り立たない)
                _ => false,
            };
        }
        match w.as_str() {
            "unix" => cfg!(unix),
            "windows" => cfg!(windows),
            // (rust-analyzer はワークスペースのクレートを test と debug_assertions ありで見る)
            "test" | "debug_assertions" => true,
            // (設定されていない cfg: `no_global_oom_handling` / `miri` / `doc` / `bootstrap` など)
            _ => false,
        }
    }

    fn in_inactive(&self, at: usize) -> bool {
        self.inactive.iter().any(|&(a, b)| a <= at && at < b)
    }

    /// トークン k が束縛のパターンの中の名前か (`let (a, b) =` / `for (i, x) in` / `if let Some(x) =` / `|a, b|`)
    fn pat_binding(&self, k: usize) -> bool {
        let w = self.tok(k);
        if !w.starts_with(|c: char| c.is_ascii_lowercase() || c == '_') || PRIMS.contains(&w) || matches!(w, "self" | "mut" | "ref") {
            return false;
        }
        if self.sym(k + 1, b'(') || self.sym(k + 1, b'!') || (self.sym(k + 1, b':') && self.sym(k + 2, b':')) || self.after_path(k) || (k > 0 && self.sym(k - 1, b'.')) {
            return false;
        }
        let mut j = k;
        for _ in 0..64 {
            if j == 0 {
                return false;
            }
            j -= 1;
            if self.ident(j) {
                match self.tok(j) {
                    "let" | "for" => return true,
                    "in" | "if" | "match" | "return" | "else" | "while" | "fn" | "move" | "as" => return false,
                    _ => continue,
                }
            }
            if self.sym(j, b'|') {
                return self.closure_start(j) && !(j > 0 && self.sym(j - 1, b'|'));
            }
            if self.toks[j].0 == 1 && matches!(self.text.as_bytes()[self.toks[j].1 as usize], b',' | b'(' | b')' | b'[' | b']' | b'&' | b'{' | b'}' | b'@') {
                continue;
            }
            // (`Foo::Bar(x)` の `::` と、クロージャの引数の型 `|a: T, b|` の `:`)
            if self.sym(j, b':') {
                continue;
            }
            return false;
        }
        false
    }

    /// トークン k が関数の引数の名前か (`fn f(a: T, mut b: U)`)
    fn fn_param(&self, k: usize) -> bool {
        if !(self.sym(k + 1, b':') && !self.sym(k + 2, b':')) || k == 0 || !(self.sym(k - 1, b'(') || self.sym(k - 1, b',') || self.tok(k - 1) == "mut") {
            return false;
        }
        let Some(f) = self.enclosing_fn(self.toks[k].1 as usize) else { return false };
        // (本体の `{` より前)
        let mut j = self.toks.partition_point(|t| (t.1 as usize) < f.start);
        let mut d = 0;
        while j < self.toks.len() && j < k {
            if self.sym(j, b'(') || self.sym(j, b'[') {
                d += 1;
            } else if self.sym(j, b')') || self.sym(j, b']') {
                d -= 1;
            } else if d == 0 && self.sym(j, b'{') {
                return false;
            }
            j += 1;
        }
        true
    }

    /// byte 位置 at が `format!("{name}")` の中の名前の上なら (リテラルのトークン, その名前のローカルの束縛のトークン, 名前の始め)
    fn capture_at(&self, at: usize) -> Option<(usize, usize, usize)> {
        let i = self.toks.partition_point(|t| (t.2 as usize) <= at);
        let t = *self.toks.get(i)?;
        if t.0 != 2 || (t.1 as usize) > at || !self.text[t.1 as usize..].starts_with('"') {
            return None;
        }
        let lit = &self.text[t.1 as usize..t.2 as usize];
        let (a, name) = captures(lit).into_iter().map(|(a, n)| (t.1 as usize + a, n)).find(|&(a, n)| a <= at && at <= a + n.len())?;
        let b = self.local(i, name)?;
        Some((i, b, a))
    }

    /// 閉じ括弧 p に対応する開き括弧
    fn open_of(&self, p: usize) -> Option<usize> {
        let mut d = 0i32;
        let mut j = p;
        loop {
            if self.toks[j].0 == 1 {
                match self.text.as_bytes()[self.toks[j].1 as usize] {
                    b')' | b']' | b'}' => d += 1,
                    b'(' | b'[' | b'{' => {
                        d -= 1;
                        if d == 0 {
                            return Some(j);
                        }
                    }
                    _ => {}
                }
            }
            if j == 0 {
                return None;
            }
            j -= 1;
        }
    }

    /// match の腕の `=>` (トークン arrow は `=`) のパターンのトークンの範囲 (ガード `if ..` は除く)
    fn arm_pat(&self, arrow: usize) -> (usize, usize) {
        let mut j = arrow;
        let mut start = 0;
        while j > 0 {
            let p = j - 1;
            if self.toks[p].0 == 1 {
                match self.text.as_bytes()[self.toks[p].1 as usize] {
                    b',' | b';' | b'{' | b'(' | b'[' => {
                        start = j;
                        break;
                    }
                    b'>' if p >= 1 && self.sym(p - 1, b'=') => {
                        start = j;
                        break;
                    }
                    b')' | b']' | b'}' => {
                        let Some(o) = self.open_of(p) else { break };
                        // (`}` は構造体のパターン `Path { .. }` か、前の腕の本体のブロック)
                        if self.sym(p, b'}') && !(o >= 1 && self.ident(o - 1) && self.tok(o - 1).starts_with(|c: char| c.is_ascii_uppercase())) {
                            start = j;
                            break;
                        }
                        j = o;
                        continue;
                    }
                    _ => {}
                }
            }
            j = p;
        }
        let mut end = arrow;
        let mut d = 0i32;
        for q in start..arrow {
            if self.sym(q, b'(') || self.sym(q, b'[') || self.sym(q, b'{') {
                d += 1;
            } else if self.sym(q, b')') || self.sym(q, b']') || self.sym(q, b'}') {
                d -= 1;
            } else if d == 0 && self.ident(q) && self.tok(q) == "if" {
                end = q;
                break;
            }
        }
        (start, end)
    }

    /// トークン k が match の腕のパターンの中の束縛か (`Some(x) =>` / `T { a, b: c } =>`)
    fn arm_binding(&self, k: usize) -> bool {
        let w = self.tok(k);
        if !w.starts_with(|c: char| c.is_ascii_lowercase() || c == '_') || PRIMS.contains(&w) || matches!(w, "self" | "mut" | "ref" | "_") {
            return false;
        }
        if self.sym(k + 1, b'(') || self.sym(k + 1, b'!') || self.sym(k + 1, b'.') || (self.sym(k + 1, b':') && self.sym(k + 2, b':')) || self.after_path(k) || (k > 0 && self.sym(k - 1, b'.')) {
            return false;
        }
        if self.sym(k + 1, b':') {
            return false;
        }
        let n = self.toks.len();
        let mut j = k + 1;
        let mut d = 0i32;
        while j < n && j < k + 256 {
            if self.toks[j].0 == 1 {
                match self.text.as_bytes()[self.toks[j].1 as usize] {
                    b'(' | b'[' => d += 1,
                    b'{' => {
                        if d == 0 && !(j >= 1 && self.ident(j - 1) && self.tok(j - 1).starts_with(|c: char| c.is_ascii_uppercase())) {
                            return false;
                        }
                        d += 1;
                    }
                    b')' | b']' | b'}' => d -= 1,
                    b';' => return false,
                    b'=' if self.sym(j + 1, b'>') => {
                        let (a, e) = self.arm_pat(j);
                        return a <= k && k < e;
                    }
                    b'=' => return false,
                    _ => {}
                }
            }
            j += 1;
        }
        false
    }

    /// at を囲むアイテム (関数・型・trait・impl) の型引数の宣言の中の name のトークン (内側から)
    fn generic_param(&self, at: usize, name: &str) -> Option<usize> {
        let mut ds: Vec<&Def> = self
            .defs
            .iter()
            .filter(|d| matches!(d.kind, SymKind::Function | SymKind::Method | SymKind::Struct | SymKind::Enum | SymKind::Union | SymKind::Trait | SymKind::Impl | SymKind::TypeAlias) && d.item_start <= at && at < d.item_end)
            .collect();
        ds.sort_by_key(|d| d.item_end - d.item_start);
        let n = self.toks.len();
        for d in ds {
            // (`<` は名前の直後か、impl なら `impl` の直後)
            let mut j = self.toks.partition_point(|t| (t.1 as usize) < d.start);
            if d.kind == SymKind::Impl {
                let mut q = self.toks.partition_point(|t| (t.1 as usize) < d.item_start);
                while q < j && self.tok(q) != "impl" {
                    q += 1;
                }
                j = q;
            }
            j += 1;
            if !self.sym(j, b'<') {
                continue;
            }
            let mut dd = 0;
            let mut sep = false;
            while j < n {
                if self.sym(j, b'<') {
                    dd += 1;
                    sep = dd == 1;
                } else if self.sym(j, b'>') && !self.sym(j - 1, b'-') {
                    dd -= 1;
                    if dd == 0 {
                        break;
                    }
                } else if dd == 1 && self.sym(j, b',') {
                    sep = true;
                } else if dd == 1 && sep && self.ident(j) && self.tok(j) == "const" {
                } else {
                    if dd == 1 && sep && self.ident(j) && self.tok(j) == name {
                        return Some(j);
                    }
                    sep = false;
                }
                j += 1;
            }
        }
        None
    }

    /// at を囲むアイテムの型引数 name の束縛の trait の名前 (`<T: Clone + Debug>` と `where T: Hash`)
    fn param_bounds(&self, at: usize, name: &str) -> Vec<String> {
        let Some(j) = self.generic_param(at, name) else { return vec![] };
        let mut out = vec![];
        let n = self.toks.len();
        // (`T:` の後: 深さ 0 の `,` / `>` まで。`+` で区切った各 trait の頭の名前)
        let mut take = |mut q: usize, out: &mut Vec<String>| {
            let mut d = 0;
            let mut first = true;
            while q < n {
                if self.sym(q, b'<') || self.sym(q, b'(') {
                    d += 1;
                } else if (self.sym(q, b'>') && !self.sym(q - 1, b'-')) || self.sym(q, b')') {
                    if d == 0 {
                        break;
                    }
                    d -= 1;
                } else if d == 0 && (self.sym(q, b',') || self.sym(q, b'{') || self.sym(q, b';') || self.sym(q, b'=')) {
                    break;
                } else if d == 0 && self.sym(q, b'+') {
                    first = true;
                } else if d == 0 && first && self.ident(q) && !self.before_path(q) && !matches!(self.tok(q), "dyn" | "const" | "for") && !(q >= 1 && self.sym(q - 1, b'?')) {
                    out.push(self.tok(q).to_string());
                    first = false;
                }
                q += 1;
            }
        };
        if self.sym(j + 1, b':') && !self.sym(j + 2, b':') {
            take(j + 2, &mut out);
        }
        // (囲むアイテムの where: 本体の `{` の前)
        for d in self.defs.iter().filter(|d| matches!(d.kind, SymKind::Function | SymKind::Method | SymKind::Impl | SymKind::Struct | SymKind::Enum | SymKind::Trait) && d.item_start <= at && at < d.item_end) {
            let mut q = self.toks.partition_point(|t| (t.1 as usize) < d.item_start);
            while q < n && (self.toks[q].1 as usize) < d.item_end && !self.sym(q, b'{') {
                if self.ident(q) && self.tok(q) == "where" {
                    let mut w = q + 1;
                    while w < n && !self.sym(w, b'{') && !self.sym(w, b';') {
                        if self.ident(w) && self.tok(w) == name && self.sym(w + 1, b':') && !self.sym(w + 2, b':') {
                            take(w + 2, &mut out);
                        }
                        w += 1;
                    }
                    break;
                }
                q += 1;
            }
        }
        out
    }

    /// `use a::B as C` で取り込んだ別名 C の、もとの名前 B
    fn use_alias(&self, name: &str) -> Option<&str> {
        self.uses.iter().find(|u| u.0 == name && u.1.len() >= 2 && u.1.last().is_some_and(|l| l != name)).map(|u| u.1[u.1.len() - 1].as_str())
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
            + self.derives.iter().map(|(a, b, _)| a.capacity() + b.capacity() + 56).sum::<usize>()
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

    /// use_syms を作る (別名の付いた use と pub use。glob と `self` は除く)
    fn collect_use_syms(&self) -> Vec<(String, u32, u32, u32)> {
        let pubs: Vec<usize> = self.collect_uses(true).into_iter().map(|x| x.2).collect();
        let mut out = vec![];
        for ((local, path), &at) in self.uses.iter().zip(&self.use_at) {
            if at == usize::MAX || local == "self" || local == "_" {
                continue;
            }
            let alias = path.last().is_some_and(|l| l != local);
            if !(alias || pubs.contains(&at)) {
                continue;
            }
            // (別名なら `as` の後の名前の位置)
            let a = if alias { self.text[at..].find(&format!(" {local}")).map(|i| at + i + 1).unwrap_or(at) } else { at };
            let ls = self.text[..a].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let line = self.text[..ls].matches('\n').count() as u32;
            out.push((local.clone(), line, self.text[ls..a].encode_utf16().count() as u32, local.encode_utf16().count() as u32));
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
                        // match の腕のパターン (`T { x: a, .. }` の x はフィールドの名前)
                        let (pa, pe) = self.arm_pat(i);
                        for j in pa.max(arm_start.min(pa))..pe {
                            // (`(m, 0) | (0, m)` の 2 つめの m は 1 つめと同じ束縛: rust-analyzer と同じく 1 つめに)
                            if is_bind(j) && !self.sym(j + 1, b'{') && !(self.sym(j + 1, b':') && !self.sym(j + 2, b':')) && !(pa..j).any(|q| self.ident(q) && self.tok(q) == self.tok(j)) {
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
                    // match の腕のガード `P(x) if x > 0 =>`: パターンの束縛はガードから見える
                    "if" => {
                        if let Some(arrow) = self.guard_arrow(i) {
                            let (pa, pe) = self.arm_pat(arrow);
                            if pe == i {
                                for j in pa..pe {
                                    if is_bind(j) && !self.sym(j + 1, b'{') && !(self.sym(j + 1, b':') && !self.sym(j + 2, b':')) {
                                        live.push((j, depth, 1));
                                    }
                                }
                            }
                        }
                    }
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

    /// `if` (トークン i) が match の腕のガードなら、その後の `=>` の `=` (ブロック `{` や `;` の前にあるもの)
    fn guard_arrow(&self, i: usize) -> Option<usize> {
        let mut d = 0i32;
        for j in i + 1..(i + 128).min(self.toks.len()) {
            if self.toks[j].0 != 1 {
                continue;
            }
            match self.text.as_bytes()[self.toks[j].1 as usize] {
                b'(' | b'[' => d += 1,
                b')' | b']' => {
                    d -= 1;
                    if d < 0 {
                        return None;
                    }
                }
                b'{' | b'}' | b';' => return None,
                b',' if d == 0 => return None,
                b'=' if d == 0 && self.sym(j + 1, b'>') => return Some(j),
                _ => {}
            }
        }
        None
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
            // (`&` / `mut` / 寿命 `'a` (字句では `'` の後の識別子) を飛ばす)
            while j < n && (self.sym(j, b'&') || self.tok(j) == "mut" || self.toks[j].0 == 2 || (self.ident(j) && self.toks[j].1 >= 1 && self.text.as_bytes()[self.toks[j].1 as usize - 1] == b'\'')) {
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
            let up = |j: usize| self.tok(j).starts_with(|c: char| c.is_ascii_uppercase());
            // (`a::b::T` のモジュールの部分を飛ばす。`T::new` の T で止まる)
            while self.ident(j) && self.before_path(j) && self.ident(j + 3) && !(up(j) && !up(j + 3)) {
                j += 3;
            }
            // (`T::..` / `T { .. }` / `T(..)`、単位構造体 `a::T;` (全部大文字の定数は除く))
            if self.ident(j) && up(j) && (self.before_path(j) || self.sym(j + 1, b'{') || self.sym(j + 1, b'(') || (self.sym(j + 1, b';') && (self.tok(j).len() == 1 || self.tok(j).contains(|c: char| c.is_ascii_lowercase())))) {
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

/// derive で作られるメソッド (`default` / `clone` / `eq` / `fmt` / ..) の、持ち主の型 (owners のどれか) の derive の名前の位置
fn derive_method(ws: &[&FileIndex], file: usize, owners: &[String], name: &str) -> Option<Loc> {
    let tr = match name {
        "default" => "Default",
        "clone" | "clone_from" => "Clone",
        "eq" | "ne" => "PartialEq",
        "fmt" => "Debug",
        "partial_cmp" | "lt" | "le" | "gt" | "ge" => "PartialOrd",
        "cmp" | "max" | "min" | "clamp" => "Ord",
        "hash" => "Hash",
        _ => return None,
    };
    let kr = ws[file].krate;
    ws.iter().enumerate().filter(|(i, w)| !w.lib && (*i == file || w.in_src || w.krate == kr)).find_map(|(i, w)| {
        w.derives.iter().find(|(t, x, _)| x == tr && owners.contains(t)).map(|&(_, _, at)| Loc { file: i, start: at as usize, end: at as usize + tr.len() })
    })
}

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
    let l: Vec<(usize, &Def)> = l.into_iter().filter(|x| score(x.1) == best).collect();
    // (同じファイルの同じ名前のメソッドがいくつも: 型引数の違う trait の impl (`Extend<T>` / `Extend<&T>` for Vec)。
    // 先のもの (ふつうは総称的なもの) にする)
    // (別のファイルにあってもよい: `impl From<String> for Arc<str>` (string.rs) / `impl<T> From<T> for Arc<T>` (sync.rs))
    if l.len() > 1 && l.iter().all(|x| x.1.kind == SymKind::Method && x.1.name == l[0].1.name && x.1.owner == l[0].1.owner) {
        return vec![loc(l[0].0, l[0].1)];
    }
    pick(l, file)
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
/// off の `Self` を囲む impl / trait の (見出しの型・trait の名前のトークン, アイテムの範囲) (rust-analyzer の `Self` の行き先)
fn self_owner(f: &FileIndex, off: usize) -> Option<(usize, usize, usize)> {
    let k = f.tok_at(off)?;
    if f.tok(k) != "Self" {
        return None;
    }
    let d = f.defs.iter().filter(|d| matches!(d.kind, SymKind::Impl | SymKind::Trait) && d.item_start <= off && off < d.item_end).min_by_key(|d| d.item_end - d.item_start)?;
    if d.kind == SymKind::Trait {
        return Some((f.toks.partition_point(|t| (t.1 as usize) < d.start), d.item_start, d.item_end));
    }
    // (impl の見出し `impl<T> Tr for a::S<T> where .. {` の、`{` / where の前で深さ 0 の最後の識別子)
    let mut j = f.toks.partition_point(|t| (t.1 as usize) < d.item_start);
    while j < f.toks.len() && f.tok(j) != "impl" {
        j += 1;
    }
    let mut dd = 0;
    let mut last = None;
    j += 1;
    while j < f.toks.len() && (f.toks[j].1 as usize) < d.item_end {
        if f.sym(j, b'<') {
            dd += 1;
        } else if f.sym(j, b'>') && !f.sym(j - 1, b'-') {
            dd -= 1;
        } else if dd == 0 && (f.sym(j, b'{') || f.tok(j) == "where") {
            break;
        } else if dd == 0 && f.ident(j) && !matches!(f.tok(j), "for" | "dyn" | "mut" | "const" | "unsafe") {
            last = Some(j);
        }
        j += 1;
    }
    Some((last?, d.item_start, d.item_end))
}

/// impl / trait の中の `Self` の参照: 見出しの名前と、その中の `Self` (rust-analyzer と同じ)
fn self_refs(ws: &[&FileIndex], file: usize, off: usize, include_decl: bool) -> Option<Vec<Loc>> {
    let f = ws[file];
    let (h, a, e) = self_owner(f, off)?;
    let mut out = vec![];
    if include_decl {
        out.push(Loc { file, start: f.toks[h].1 as usize, end: f.toks[h].2 as usize });
    }
    for &k in f.idents.get("Self").map(|v| v.as_slice()).unwrap_or(&[]) {
        let at = f.toks[k as usize].1 as usize;
        if a <= at && at < e && !f.in_macro_body(at) {
            out.push(Loc { file, start: at, end: f.toks[k as usize].2 as usize });
        }
    }
    Some(out)
}

/// アイテムのキーワード (`fn` / `struct` / `enum` / `trait` ..) の上なら、その後の名前の位置 (rust-analyzer はキーワードの上でも
/// そのアイテムに答える)
fn kw_to_name(f: &FileIndex, off: usize) -> usize {
    if let Some(k) = f.tok_at(off) {
        if matches!(f.tok(k), "fn" | "struct" | "enum" | "union" | "trait" | "type" | "const" | "static" | "mod") && f.ident(k + 1) {
            let a = f.toks[k + 1].1 as usize;
            if f.defs.iter().any(|d| d.start == a) {
                return a;
            }
        }
    }
    off
}

pub fn definition(ws: &[&FileIndex], file: usize, off: usize) -> Vec<Loc> {
    let off = kw_to_name(ws[file], off);
    if let Some((h, _, _)) = self_owner(ws[file], off) {
        let f = ws[file];
        return vec![Loc { file, start: f.toks[h].1 as usize, end: f.toks[h].2 as usize }];
    }
    clear_memo();
    let d: Vec<Loc> = def_at(ws, file, off).into_iter().map(|l| mod_file(ws, l).unwrap_or(l)).collect();
    // (構造体の省略形 `T { x }` の x: ローカルとフィールドの両方 (rust-analyzer と同じくローカルが先))
    if let (Some(k), [local]) = (ws[file].tok_at(off), d.as_slice()) {
        if !ws[local.file].defs.iter().any(|x| x.start == local.start) {
            if let Some(&fl) = shorthand_field(ws, file, k).first() {
                return vec![*local, fl];
            }
        }
    }
    d
}

/// `recv.into()` (into のトークン k、受け手の型 recv) の行き先: 期待する型 T (`let x: T = ..` の注釈) の `impl From<R> for T` の
/// from (R が受け手の型に合うものを先に)
fn into_from(ws: &[&FileIndex], file: usize, k: usize, recv: Option<&str>) -> Option<Loc> {
    let f = ws[file];
    // (文の始めまで戻って `let NAME: TYPE =`)
    let mut j = k;
    let mut d = 0;
    let mut eq = None;
    while j > 0 {
        j -= 1;
        if f.sym(j, b')') || f.sym(j, b']') {
            d += 1;
        } else if f.sym(j, b'(') || f.sym(j, b'[') {
            if d == 0 {
                return None;
            }
            d -= 1;
        } else if d == 0 && (f.sym(j, b';') || f.sym(j, b'{') || f.sym(j, b'}')) {
            return None;
        } else if d == 0 && f.sym(j, b'=') && !f.sym(j + 1, b'=') && !f.sym(j - 1, b'=') {
            eq = Some(j);
        } else if d == 0 && f.ident(j) && f.tok(j) == "let" {
            break;
        }
    }
    let eq = eq?;
    let c = (j + 1..eq).find(|&q| f.sym(q, b':') && !f.sym(q + 1, b':') && !f.sym(q - 1, b':'))?;
    let ty = type_head(&f.text[f.toks[c + 1].1 as usize..f.toks[eq - 1].2 as usize]);
    let rh = recv.map(type_head);
    let mut best: Option<(bool, Loc)> = None;
    for (i, d) in named(ws, file, "from") {
        if d.owner != ty || !d.container.starts_with("impl") {
            continue;
        }
        let Some(tr) = d.container.strip_prefix("impl ").and_then(|n| n.split(" for ").next()) else { continue };
        if type_head(tr) != "From" {
            continue;
        }
        let fits = rh.as_ref().is_some_and(|r| targs(tr).first().is_some_and(|a| type_head(a) == *r));
        if best.is_none_or(|b| fits && !b.0) {
            best = Some((fits, loc(i, d)));
        }
    }
    best.map(|b| b.1)
}

/// トークン k が構造体の省略形 `T { x }` の x で、ローカルの x を指すなら、そのフィールドの定義
fn shorthand_pair(ws: &[&FileIndex], file: usize, k: usize) -> Option<Loc> {
    let f = ws[file];
    let l = *def_at(ws, file, f.toks[k].1 as usize).first()?;
    if ws[l.file].defs.iter().any(|x| x.start == l.start) {
        return None;
    }
    shorthand_field(ws, file, k).first().copied()
}

/// `mod x;` (ファイルのモジュール) の定義の行き先はそのファイルの始め (rust-analyzer と同じ)
fn mod_file(ws: &[&FileIndex], l: Loc) -> Option<Loc> {
    let d = ws[l.file].defs.iter().find(|d| d.start == l.start && d.kind == SymKind::Module)?;
    let i = ws.iter().position(|w| !w.lib && w.module.len() == d.module.len() + 1 && w.module.starts_with(&d.module) && w.module.last() == Some(&d.name))?;
    Some(Loc { file: i, start: 0, end: 0 })
}

fn def_at(ws: &[&FileIndex], file: usize, off: usize) -> Vec<Loc> {
    if ws.is_empty() {
        return vec![];
    }
    let f = ws[file];
    if let Some(k) = f.tok_at(off) {
        if f.in_inactive(f.toks[k].1 as usize) {
            return inactive_def_at(ws, file, k);
        }
    }
    def_at1(ws, file, off)
}

/// cfg で外れたコードの中のトークン k: rust-analyzer と同じく、外側のスコープから見えるアイテムへのパス
/// (`used()` / `P` / `sub::deep` / `crate::P`) だけを解決する。ローカル・フィールド・メソッド・関連アイテム (`P::new`)・
/// 外れたアイテムの名前には答えない
fn inactive_def_at(ws: &[&FileIndex], file: usize, k: usize) -> Vec<Loc> {
    let f = ws[file];
    let decl = k > 0 && f.ident(k - 1) && matches!(f.tok(k - 1), "fn" | "struct" | "enum" | "union" | "trait" | "type" | "const" | "static" | "mod" | "let" | "mut");
    if decl || (k > 0 && f.sym(k - 1, b'.')) || (f.sym(k + 1, b':') && !f.sym(k + 2, b':') && f.struct_lit_at(k).is_some()) {
        return vec![];
    }
    // (`T::name` の T が型 (大文字で始まる・Self) なら関連アイテム)
    if f.after_path(k) && k >= 3 && (f.tok(k - 3) == "Self" || f.tok(k - 3).starts_with(|c: char| c.is_ascii_uppercase())) {
        return vec![];
    }
    let r = def_at1(ws, file, f.toks[k].1 as usize);
    r.into_iter()
        .filter(|l| ws[l.file].defs.iter().any(|d| d.start == l.start && (d.owner.is_empty() || d.kind == SymKind::Variant) && d.kind != SymKind::Field) || (l.start == 0 && l.end == 0))
        .collect()
}

fn def_at1(ws: &[&FileIndex], file: usize, off: usize) -> Vec<Loc> {
    let f = ws[file];
    let Some(k) = f.tok_at(off) else { return f.capture_at(off).map(|(_, b, _)| vec![Loc { file, start: f.toks[b].1 as usize, end: f.toks[b].2 as usize }]).unwrap_or_default() };
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
        let (mut owners, concrete) = owner.as_ref().map(|o| owners_of2(ws, file, o)).unwrap_or_default();
        // (受け手の型が型引数 `T` なら、その束縛の trait (`impl<T: Clone>` / `where T: Clone`) のメソッド)
        if let Some(o) = owner.as_deref() {
            owners.extend(f.param_bounds(start, strip_ref(o)));
        }
        let c: Vec<(usize, &Def)> = named(ws, file, &*name).into_iter().filter(|(_, d)| kinds.contains(&d.kind) && (owner.is_none() || owners.contains(&d.owner))).collect();
        let mut c = prefer_concrete(c, &concrete);
        // (`let h: Hidden = n.into();`: 期待する型 Hidden の `impl From<..> for Hidden` の from (rust-analyzer と同じ))
        if call && name == "into" {
            if let Some(l) = into_from(ws, file, k, owner.as_deref()) {
                return vec![l];
            }
        }
        // (derive で作られるメソッド: 具体的な型の持ち主の候補がなければ、ライブラリの trait の宣言より derive の名前)
        if owner.is_some() && !c.iter().any(|x| concrete.contains(&x.1.owner)) {
            if let Some(l) = derive_method(ws, file, &concrete, &name) {
                return vec![l];
            }
        }
        if c.is_empty() && owner.is_some() {
            // (ブランケットの impl の trait のメソッド: `"x".to_string()`)
            let bl = blanket_traits(ws, file, false);
            c = named(ws, file, &*name).into_iter().filter(|(_, d)| kinds.contains(&d.kind) && bl.contains(&d.owner)).collect();
        }
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
                q = if t == "Self" { f.enclosing_owner(off).unwrap_or(t) } else { t };
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
            let c: Vec<(usize, &Def)> = named(ws, file, &*name).into_iter().filter(|(_, d)| qs.contains(&d.owner) || (d.owner.is_empty() && d.module.last() == Some(&q))).collect();
            let c = prefer_concrete(c, &qs[..nc]);
            // (derive で作られるメソッド `Point::default()`: 具体的な型の持ち主の候補がなければ derive の名前)
            if !c.iter().any(|x| qs[..nc].contains(&x.1.owner)) {
                if let Some(l) = derive_method(ws, file, &qs[..nc], &name) {
                    return vec![l];
                }
            }
            // (見つからない: `use std::sync::Mutex` のように再エクスポートされた名前なら、下の use でたどる。
            // Q が知らない名前 (`syn::Pat` / `Delimiter::None` の外部クレートのもの) なら答えない)
            if !c.is_empty() {
                return prefer_ws(ws, c, file);
            }
            // (derive で作られるメソッド `T::default()` は derive の名前へ: rust-analyzer と同じ)
            if let Some(l) = derive_method(ws, file, &qs[..nc], &name) {
                return vec![l];
            }
            let known = ws.iter().any(|w| w.module.contains(&q) || w.defs.iter().any(|d| d.name == q && matches!(d.kind, SymKind::Module | SymKind::Struct | SymKind::Enum | SymKind::Trait | SymKind::TypeAlias | SymKind::Union)))
                || matches!(q.as_str(), "std" | "core" | "alloc");
            if !known {
                return vec![];
            }
        }
    }
    // `self::` / `super::` (パスの始め): そのモジュール
    if (name == "self" || name == "super") && f.before_path(k) {
        let mut segs = vec![name.as_str()];
        let mut j = k;
        while j >= 3 && f.after_path(j) && matches!(f.tok(j - 3), "self" | "super") {
            j -= 3;
            segs.push(f.tok(j));
        }
        if f.after_path(j) {
            return vec![];
        }
        let mut m = f.module_at(off);
        for s in segs {
            if s == "super" && m.pop().is_none() {
                return vec![];
            }
        }
        let Some(last) = m.pop() else { return vec![] };
        let c: Vec<(usize, &Def)> = all().filter(|(_, d)| d.kind == SymKind::Module && d.name == last && d.module[..] == m[..]).collect();
        return prefer_ws(ws, c, file);
    }
    if name == "Self" {
        if let Some(o) = f.enclosing_owner(off) {
            let c: Vec<(usize, &Def)> = named(ws, file, &*o).into_iter().filter(|(_, d)| d.owner.is_empty() && matches!(d.kind, SymKind::Struct | SymKind::Enum | SymKind::Union | SymKind::TypeAlias)).collect();
            return prefer_ws(ws, c, file);
        }
        return vec![];
    }
    // `#[derive(Name)]` の Name は derive のマクロ
    if f.in_derive(k) {
        let c: Vec<(usize, &Def)> = named(ws, file, &*name).into_iter().filter(|(_, d)| d.kind == SymKind::Macro).collect();
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
    // 構造体の式・パターンのフィールドの名前 `T { x: .. }` (省略形 `T { x }` は束縛・ローカルの方: rust-analyzer と同じ)
    if k >= 1 && (f.sym(k - 1, b'{') || f.sym(k - 1, b',')) && f.sym(k + 1, b':') && !f.sym(k + 2, b':') {
        if let Some(t) = f.struct_lit_at(k) {
            let t = if t == "Self" { f.enclosing_owner(off).unwrap_or(t) } else { t };
            let (os, _) = owner_split(ws, file, &t);
            let c: Vec<(usize, &Def)> = named(ws, file, &*name).into_iter().filter(|(_, d)| d.kind == SymKind::Field && os.contains(&d.owner)).collect();
            return prefer_ws(ws, c, file);
        }
    }
    // (パターン・引数の中の束縛の名前はそれ自身: `let (a, b)` / `fn f(x: T)` / `Some(x) =>`)
    if f.pat_binding(k) || f.fn_param(k) || f.arm_binding(k) {
        return vec![Loc { file, start, end: (f.toks[k].2 as usize) }];
    }
    // ローカル
    if !f.after_path(k) {
        if let Some(b) = f.local(k, &name) {
            return vec![Loc { file, start: (f.toks[b].1 as usize), end: (f.toks[b].2 as usize) }];
        }
    }
    // 型引数 (`fn f<T>(x: T)` / `impl<T> S<T>` の T)
    if name.starts_with(|c: char| c.is_ascii_uppercase()) && !f.after_path(k) {
        if let Some(g) = f.generic_param(start, &name) {
            return vec![Loc { file, start: f.toks[g].1 as usize, end: f.toks[g].2 as usize }];
        }
    }
    // アイテム: 同じモジュール → use → ワークスペースで一つ → ライブラリ (prelude の Option / Some など)
    let mut cands: Vec<(usize, &Def)> = named(ws, file, &*name).into_iter().filter(|(_, d)| d.owner.is_empty() && ITEM_KINDS.contains(&d.kind)).collect();
    if cands.is_empty() {
        // (`use a::Point as Pt` の Pt: もとの名前の定義を、その use で引く)
        if let Some(real) = f.use_alias(&name) {
            cands = named(ws, file, &*real).into_iter().filter(|(_, d)| d.owner.is_empty() && ITEM_KINDS.contains(&d.kind)).collect();
        }
    }
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
    references_in(ws, file, off, include_decl, load, None)
}

/// textDocument/documentHighlight: ファイル file の中の参照だけ (ほかのファイルは探さない)
pub fn highlights(ws: &[&FileIndex], file: usize, off: usize, load: Load) -> Vec<Loc> {
    let mut v = references_in(ws, file, off, true, load, Some(file));
    // (省略形 `T { x }` の x の上: ローカルの参照とフィールドの参照の両方 (rust-analyzer と同じ))
    if let Some(k) = ws[file].tok_at(off) {
        if let Some(fl) = shorthand_pair(ws, file, k) {
            if !ws[fl.file].slim {
                for l in references_in(ws, fl.file, fl.start, true, load, Some(file)) {
                    if l.file == file && !v.contains(&l) {
                        v.push(l);
                    }
                }
                v.sort_by_key(|l| l.start);
            }
        }
    }
    v
}

/// references の本体。only があればそのファイルの中だけを探す
/// `fn` のキーワードの上: 関数の出口 (`fn` 自身・`return ..`・`?`・本体の最後の式)。rust-analyzer の references /
/// documentHighlight と同じ
fn exit_points(f: &FileIndex, file: usize, off: usize) -> Option<Vec<Loc>> {
    let k = f.tok_at(off)?;
    if f.tok(k) != "fn" || !f.ident(k + 1) {
        return None;
    }
    let a = f.toks[k + 1].1 as usize;
    let d = f.defs.iter().find(|d| d.start == a && matches!(d.kind, SymKind::Function | SymKind::Method))?;
    let n = f.toks.len();
    // (本体の `{`: シグネチャの括弧の外の最初の `{`)
    let mut j = k + 2;
    let mut dd = 0;
    while j < n && (f.toks[j].1 as usize) < d.item_end {
        if f.sym(j, b'(') || f.sym(j, b'[') || f.sym(j, b'<') {
            dd += 1;
        } else if f.sym(j, b')') || f.sym(j, b']') || (f.sym(j, b'>') && !f.sym(j - 1, b'-')) {
            dd -= 1;
        } else if dd <= 0 && (f.sym(j, b'{') || f.sym(j, b';')) {
            break;
        }
        j += 1;
    }
    if !f.sym(j, b'{') {
        return Some(vec![Loc { file, start: f.toks[k].1 as usize, end: f.toks[k].2 as usize }]);
    }
    let open = j;
    let mut close = open;
    let mut dd = 0;
    while close < n {
        if f.sym(close, b'{') {
            dd += 1;
        } else if f.sym(close, b'}') {
            dd -= 1;
            if dd == 0 {
                break;
            }
        }
        close += 1;
    }
    let inner = |at: usize| f.defs.iter().any(|x| x.start != d.start && matches!(x.kind, SymKind::Function | SymKind::Method) && x.item_start <= at && at < x.item_end);
    let mut out = vec![Loc { file, start: f.toks[k].1 as usize, end: f.toks[k].2 as usize }];
    for q in open + 1..close.min(n) {
        let at = f.toks[q].1 as usize;
        if inner(at) || f.in_macro_body(at) {
            continue;
        }
        if f.sym(q, b'?') {
            out.push(Loc { file, start: at, end: at + 1 });
        } else if f.ident(q) && f.tok(q) == "return" {
            out.push(Loc { file, start: at, end: f.toks[q].2 as usize });
        }
    }
    // (最後の式: match / if / ブロックなら中の腕・ブロックの最後の式をたどる)
    block_tails(f, file, open, close, &mut out);
    Some(out)
}

/// ブロック (開き括弧 p) で終わる式の始め: `match x {` の match、`if c { } else {` の if、`loop {` / `unsafe {`、ただのブロック
/// なら p。`fn` の本体・`impl` などのアイテムの `{` なら None
fn block_expr_start(f: &FileIndex, p: usize) -> Option<usize> {
    let mut q = p;
    loop {
        // (`} else {` / `} else if c {`: 前のブロックへ)
        if q >= 2 && f.ident(q - 1) && f.tok(q - 1) == "else" && f.sym(q - 2, b'}') {
            q = f.open_of(q - 2)?;
            continue;
        }
        let mut r = q;
        let mut d = 0;
        while r > 0 {
            r -= 1;
            if f.sym(r, b')') || f.sym(r, b']') {
                d += 1;
            } else if f.sym(r, b'(') || f.sym(r, b'[') {
                if d == 0 {
                    return Some(q);
                }
                d -= 1;
            } else if d == 0 && (f.sym(r, b';') || f.sym(r, b'{') || f.sym(r, b'}') || f.sym(r, b',')) {
                return Some(q);
            } else if d == 0 && f.ident(r) {
                match f.tok(r) {
                    "match" | "loop" | "unsafe" | "async" => return Some(r),
                    "if" => {
                        // (`else if`: さらに前の if へ)
                        if r >= 2 && f.ident(r - 1) && f.tok(r - 1) == "else" && f.sym(r - 2, b'}') {
                            q = f.open_of(r - 2)?;
                            break;
                        }
                        return Some(r);
                    }
                    "fn" | "impl" | "struct" | "enum" | "trait" | "mod" | "union" | "where" | "while" | "for" => return None,
                    _ => {}
                }
            }
        }
        if r == 0 {
            return Some(q);
        }
    }
}

/// トークン j の開き括弧に対応する閉じ括弧
fn close_of(f: &FileIndex, j: usize) -> usize {
    let mut d = 0;
    let mut q = j;
    while q < f.toks.len() {
        if f.sym(q, b'{') || f.sym(q, b'(') || f.sym(q, b'[') {
            d += 1;
        } else if f.sym(q, b'}') || f.sym(q, b')') || f.sym(q, b']') {
            d -= 1;
            if d == 0 {
                return q;
            }
        }
        q += 1;
    }
    f.toks.len().saturating_sub(1)
}

/// ブロック `{ .. }` (open..close) の最後の式 (の出口) を out に足す
fn block_tails(f: &FileIndex, file: usize, open: usize, close: usize, out: &mut Vec<Loc>) {
    if close >= f.toks.len() || close <= open + 1 || f.sym(close - 1, b';') {
        return;
    }
    // (最後の文の始め: 深さ 0 で前が `;` か、ブロックの式 `}` の後 (`.` / `else` が続かない))
    let mut s = close - 1;
    let mut dd = 0;
    while s > open + 1 {
        if f.sym(s, b'}') || f.sym(s, b')') || f.sym(s, b']') {
            dd += 1;
        } else if f.sym(s, b'{') || f.sym(s, b'(') || f.sym(s, b'[') {
            dd -= 1;
        }
        if dd == 0 && (f.sym(s - 1, b';') || (f.sym(s - 1, b'}') && !f.sym(s, b'.') && !f.sym(s, b'?') && !(f.ident(s) && f.tok(s) == "else"))) {
            break;
        }
        s -= 1;
    }
    expr_tails(f, file, s, close - 1, out);
}

/// 式 (トークン s..=e) の出口: match の腕・if / else のブロック・ブロックならその中の最後の式、ほかは式そのもの
fn expr_tails(f: &FileIndex, file: usize, s: usize, e: usize, out: &mut Vec<Loc>) {
    let whole = |out: &mut Vec<Loc>| {
        let a = f.toks[s].1 as usize;
        if !out.iter().any(|l| l.start == a) {
            out.push(Loc { file, start: a, end: f.toks[e].2 as usize });
        }
    };
    if f.sym(s, b'{') && close_of(f, s) == e {
        block_tails(f, file, s, e, out);
    } else if f.ident(s) && matches!(f.tok(s), "while" | "for") {
        // (値のないループ: 出口ではない (rust-analyzer と同じ))
    } else if f.ident(s) && f.tok(s) == "match" {
        // (腕: `pat => 式,` / `pat => { .. }`)
        let mut b = s + 1;
        while b < e && !(f.sym(b, b'{') && !(b >= 1 && f.ident(b - 1) && f.tok(b - 1).starts_with(|c: char| c.is_ascii_uppercase()))) {
            b += 1;
        }
        if b >= e || close_of(f, b) != e {
            return whole(out);
        }
        let mut q = b + 1;
        while q < e {
            // (`=>` を探して、その後の腕の式の終わり (深さ 0 の `,` か、閉じ括弧の前))
            if f.sym(q, b'=') && f.sym(q + 1, b'>') {
                let a = q + 2;
                let mut z = a;
                let mut dd = 0;
                while z < e {
                    if f.sym(z, b'{') || f.sym(z, b'(') || f.sym(z, b'[') {
                        dd += 1;
                    } else if f.sym(z, b'}') || f.sym(z, b')') || f.sym(z, b']') {
                        dd -= 1;
                    }
                    if dd == 0 && (f.sym(z + 1, b',') || z + 1 == e || (f.sym(z, b'}') && f.sym(a, b'{'))) {
                        break;
                    }
                    z += 1;
                }
                if a <= z && z < e {
                    expr_tails(f, file, a, z, out);
                }
                q = z + 1;
            } else {
                q += 1;
            }
        }
    } else if f.ident(s) && f.tok(s) == "if" {
        // (`if c { A } else if d { B } else { C }`: 各ブロック)
        let mut q = s + 1;
        let mut any = false;
        while q <= e {
            if f.sym(q, b'{') {
                let c = close_of(f, q);
                block_tails(f, file, q, c, out);
                any = true;
                q = c + 1;
                if !(q <= e && f.ident(q) && f.tok(q) == "else") {
                    break;
                }
            }
            q += 1;
        }
        if !any {
            whole(out);
        }
    } else {
        whole(out);
    }
}

fn references_in(ws: &[&FileIndex], file: usize, off: usize, include_decl: bool, load: Load, only: Option<usize>) -> Vec<Loc> {
    clear_memo();
    if let Some(r) = exit_points(ws[file], file, off) {
        return r;
    }
    let off = kw_to_name(ws[file], off);
    if let Some(r) = self_refs(ws, file, off, include_decl) {
        return r;
    }
    let d = def_at(ws, file, off);
    let Some(&target) = d.first() else {
        if ws[file].tok_at(off).is_some_and(|k| ws[file].in_inactive(ws[file].toks[k].1 as usize) && !PRIMS.contains(&ws[file].tok(k))) {
            return vec![];
        }
        return prim_refs(ws, file, off, load, only);
    };
    let tf = ws[target.file];
    // (行き先が `#[derive(Default)]` の名前 (derive が作るメソッド): 問い合わせた名前 (`default`) で探す)
    let via_derive = !tf.slim && tf.tok_at(target.start).is_some_and(|j| tf.in_derive(j));
    let qname = ws[file].tok_at(off).map(|k| ws[file].tok(k).to_string()).unwrap_or_default();
    let name = match tf.defs.iter().find(|x| x.start == target.start) {
        _ if via_derive => qname.as_str(),
        Some(d) if tf.slim => d.name.as_str(),
        _ => &tf.text[target.start..target.end],
    };
    // (ローカルなら定義のある関数の中だけ)
    let is_local = !via_derive && !tf.defs.iter().any(|x| x.start == target.start && x.kind != SymKind::Impl);
    let mut out = vec![];
    // (ライブラリの索引はトークンを持たないので、宣言はここで足す。derive の名前も (名前が違うので下では見つからない))
    if include_decl && ((tf.lib && only.is_none()) || (via_derive && only.is_none_or(|o| o == target.file))) {
        out.push(target);
    }
    // (型なら、その impl の中の `Self` も参照: rust-analyzer と同じ)
    let tdef = tf.defs.iter().find(|x| x.start == target.start && x.kind != SymKind::Impl);
    let is_type = tdef.is_some_and(|x| matches!(x.kind, SymKind::Struct | SymKind::Enum | SymKind::Union | SymKind::TypeAlias));
    // (モジュールなら `self::` / `super::` も)
    let is_mod = tdef.is_some_and(|x| x.kind == SymKind::Module);
    let is_field = tdef.is_some_and(|x| x.kind == SymKind::Field);
    // (trait のメソッドなら、impl のメソッドとその呼び出しも: rust-analyzer と同じ)
    let mut targets = vec![target];
    if let Some(td) = tdef.filter(|x| x.kind == SymKind::Method && !x.container.starts_with("impl")) {
        for (i, w) in ws.iter().enumerate() {
            for d in &w.defs {
                if d.kind == SymKind::Method && d.name == td.name && impl_trait(&d.container) == Some(&td.container[..]) {
                    targets.push(loc(i, d));
                }
            }
        }
    }
    // (メンバなら持ち主の型の名前と、ワークスペースの具体的な型 (struct / enum / union) の名前 (型の別名・trait は除く))
    let owner: Option<String> = tdef.filter(|x| !x.owner.is_empty() && matches!(x.kind, SymKind::Method | SymKind::Function | SymKind::Const)).map(|x| x.owner.clone());
    let mut concrete: FxSet<&str> = if owner.is_some() {
        let aliases: FxSet<&str> = ws.iter().filter(|w| !w.lib).flat_map(|w| w.defs.iter()).filter(|d| matches!(d.kind, SymKind::TypeAlias | SymKind::Trait)).map(|d| d.name.as_str()).collect();
        ws.iter().filter(|w| !w.lib).flat_map(|w| w.defs.iter()).filter(|d| matches!(d.kind, SymKind::Struct | SymKind::Enum | SymKind::Union) && !aliases.contains(d.name.as_str())).map(|d| d.name.as_str()).collect()
    } else {
        FxSet::default()
    };
    // (持ち主が trait なら `P::default()` も trait のメソッドになりうる: 絞らない)
    let owner = owner.filter(|o| concrete.contains(o.as_str()));
    if owner.is_none() {
        concrete.clear();
    }
    for (i, &w0) in ws.iter().enumerate() {
        if (is_local && i != target.file) || only.is_some_and(|o| o != i) {
            continue;
        }
        if w0.lib || !(w0.may_have(name) || (is_type && w0.may_have("Self")) || (is_mod && (w0.may_have("self") || w0.may_have("super")))) {
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
        let none: Vec<u32> = vec![];
        let ks = w.idents.get(name).unwrap_or(&none);
        let selfs = if is_type { w.idents.get("Self").unwrap_or(&none) } else { &none };
        let mods = if is_mod { w.idents.get("self").into_iter().chain(w.idents.get("super")).flatten().collect() } else { vec![] };
        // (`use a::Point as Pt` の Pt は数えない: rust-analyzer と同じ。Pt の参照は Pt で引く)
        for &k in ks.iter().chain(selfs.iter()).chain(mods) {
            let at = w.toks[k as usize].1 as usize;
            // (macro_rules! の本体は名前を解決しない: rust-analyzer と同じ。cfg で外れたアイテムの中は def_at がパスだけ解決する)
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
            let here = Loc { file: i, start: at, end: (w.toks[k as usize].2 as usize) };
            // (速さのため: メンバの参照で `Q::name` の Q が持ち主でない具体的な型 (struct / enum) なら解決するまでもない)
            if let Some(o) = owner.as_deref() {
                let k = k as usize;
                if w.after_path(k) && k >= 3 && w.ident(k - 3) {
                    let q = w.tok(k - 3);
                    if q != o && q != "Self" && concrete.contains(q) {
                        continue;
                    }
                }
            }
            if def_at(wsx, i, at).first().is_some_and(|l| targets.contains(l)) {
                out.push(here);
            } else if is_field && shorthand_field(wsx, i, k as usize).first() == Some(&target) {
                // (省略形 `T { x }` の x はフィールドの参照でもある)
                out.push(here);
            }
        }
        // (ローカルなら `format!("{x}")` の中の x も)
        if is_local {
            for (t, tk) in w.toks.iter().enumerate() {
                if tk.0 != 2 || !w.text[tk.1 as usize..].starts_with('"') {
                    continue;
                }
                for (a, n) in captures(&w.text[tk.1 as usize..tk.2 as usize]) {
                    if n == name && w.local(t, n).is_some_and(|b| w.toks[b].1 as usize == target.start) {
                        let a = tk.1 as usize + a;
                        out.push(Loc { file: i, start: a, end: a + n.len() });
                    }
                }
            }
        }
    }
    out
}

/// `impl Tr for S` / `impl<T> Tr<T> for S<T>` の Tr
fn impl_trait(container: &str) -> Option<&str> {
    let n = container.strip_prefix("impl ")?;
    let i = n.rfind(" for ")?;
    let t = n[..i].trim_start_matches('!');
    Some(t.split('<').next().unwrap_or(t).rsplit("::").next().unwrap_or(t).trim())
}

/// 構造体の式・パターンの省略形 `T { x }` の x (トークン k) のフィールドの定義
fn shorthand_field(ws: &[&FileIndex], file: usize, k: usize) -> Vec<Loc> {
    let f = ws[file];
    if !(k >= 1 && (f.sym(k - 1, b'{') || f.sym(k - 1, b',')) && (f.sym(k + 1, b',') || f.sym(k + 1, b'}'))) {
        return vec![];
    }
    let Some(t) = f.struct_lit_at(k) else { return vec![] };
    let t = if t == "Self" { f.enclosing_owner(f.toks[k].1 as usize).unwrap_or(t) } else { t };
    let (os, _) = owner_split(ws, file, &t);
    let name = f.tok(k);
    let kr = f.krate;
    let c: Vec<(usize, &Def)> = ws
        .iter()
        .enumerate()
        .filter(|(i, w)| *i == file || w.in_src || w.lib || w.krate == kr)
        .flat_map(|(i, w)| w.defs.iter().map(move |d| (i, d)))
        .filter(|(_, d)| d.name == name && d.kind == SymKind::Field && os.contains(&d.owner))
        .collect();
    prefer_ws(ws, c, file)
}

/// 組み込みの型 (`u8` など) の参照: ワークスペースの同じ名前の識別子のうち、ほかの定義に解決されないもの
fn prim_refs(ws: &[&FileIndex], file: usize, off: usize, load: Load, only: Option<usize>) -> Vec<Loc> {
    let f = ws[file];
    let Some(k) = f.tok_at(off) else { return vec![] };
    let name = f.tok(k).to_string();
    if !PRIMS.contains(&name.as_str()) {
        return vec![];
    }
    let mut out = vec![];
    for (i, &w0) in ws.iter().enumerate() {
        if w0.lib || !w0.may_have(&name) || only.is_some_and(|o| o != i) {
            continue;
        }
        let held;
        let w: &FileIndex = if w0.slim {
            let Some(r) = load(i) else { continue };
            held = r;
            &held
        } else {
            w0
        };
        let Some(ks) = w.idents.get(&name) else { continue };
        for &k in ks {
            let k = k as usize;
            let at = w.toks[k].1 as usize;
            // (メソッド・フィールドの名前 `x.u8` やマクロの本体の中は除く)
            // (`0u8` の接尾辞も除く: 数のすぐ後ろ。`std::str::from_utf8` の str はモジュール)
            if w.in_macro_body(at) || w.after_path(k) || (k > 0 && (w.sym(k - 1, b'.') || (w.toks[k - 1].2 as usize == at && w.text.as_bytes()[w.toks[k - 1].1 as usize].is_ascii_digit()))) {
                continue;
            }
            // (同じ名前の定義 `fn u8(..)` の名前)
            if w.defs.iter().any(|d| d.start == at) {
                continue;
            }
            out.push(Loc { file: i, start: at, end: w.toks[k].2 as usize });
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
    /// 名前 → その名前の定義 (ファイルの番号, defs の番号)。問い合わせ 1 回の間だけ (references は同じ名前を何百回も引く)。
    /// キーはワークスペースのファイルの数と名前
    static NAMED: RefCell<FxMap<(usize, String), Vec<(u32, u32)>>> = RefCell::new(FxMap::default());
}

/// ファイル file から見える (同じクレート・src・ライブラリ) 名前 name の定義
fn named<'a>(ws: &[&'a FileIndex], file: usize, name: &str) -> Vec<(usize, &'a Def)> {
    let kr = ws[file].krate;
    let key = (ws.len(), name.to_string());
    let ids = NAMED.with(|m| m.borrow().get(&key).cloned());
    // (覚えた番号が今のワークスペースと合わなければ (読み直したファイルの差し替えなど) 引き直す)
    let ok = |ids: &[(u32, u32)]| ids.iter().all(|&(i, j)| ws.get(i as usize).and_then(|w| w.defs.get(j as usize)).is_some_and(|d| d.name == name));
    let ids = match ids {
        Some(v) if ok(&v) => v,
        _ => {
            let v: Vec<(u32, u32)> = ws.iter().enumerate().flat_map(|(i, w)| w.defs.iter().enumerate().filter(|(_, d)| d.name == name).map(move |(j, _)| (i as u32, j as u32))).collect();
            NAMED.with(|m| m.borrow_mut().insert(key, v.clone()));
            v
        }
    };
    ids.into_iter()
        .map(|(i, j)| (i as usize, &ws[i as usize].defs[j as usize]))
        .filter(|(i, _)| { let w = ws[*i]; *i == file || w.in_src || w.lib || w.krate == kr })
        .collect()
}

thread_local! {
    /// owner_names の結果 (問い合わせ 1 回の間だけ: definition / references / hover / complete の入口で消す)
    static OWNERS: RefCell<FxMap<(String, u32), (Vec<String>, usize)>> = RefCell::new(FxMap::default());
}

fn clear_memo() {
    OWNERS.with(|m| m.borrow_mut().clear());
    NAMED.with(|m| m.borrow_mut().clear());
}

/// 型 o (頭の名前) のメンバを探すときの持ち主の名前: o 自身、型の別名の右、Vec → スライス・String → str (Deref の先)、
/// その型に impl・derive された trait
fn owner_names(ws: &[&FileIndex], file: usize, o: &str) -> Vec<String> {
    owner_split(ws, file, o).0
}

/// owner_names と、そのうち trait でないもの (型自身・別名・Deref の先) の数
fn owner_split(ws: &[&FileIndex], file: usize, o: &str) -> (Vec<String>, usize) {
    // (`use a::Point as Pt` の別名はもとの名前で)
    let o = ws[file].use_alias(o).unwrap_or(o);
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
            // (数の型のメソッドは core のマクロ `uint_impl!` / `int_impl!` の中: symbols がそれを `impl __uint` / `impl __int` にする)
            "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => extra.push("__uint".to_string()),
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" => extra.push("__int".to_string()),
            _ => {}
        }
    }
    // (組み込みの型の trait (`impl Ord for usize` などは core のマクロの中で見えない): よく使う trait のメソッドを持たせる)
    let prim_traits: &[&str] = if v.iter().any(|x| matches!(x.as_str(), "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "char" | "bool")) {
        &["Ord", "PartialOrd", "PartialEq", "Eq", "Clone", "Copy", "Hash", "Default", "Debug", "Display", "ToString"]
    } else if v.iter().any(|x| matches!(x.as_str(), "f32" | "f64")) {
        &["PartialOrd", "PartialEq", "Clone", "Copy", "Default", "Debug", "Display", "ToString"]
    } else {
        &[]
    };
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
    for tr in prim_traits {
        if !traits.iter().any(|t| t == tr) {
            traits.push(tr.to_string());
        }
    }
    // (繰り返しの型: slice の Iter などの `impl Iterator` は core のマクロの中にあって見えないので、Iterator のメソッドを持たせる)
    if v.iter().any(|x| is_iter_head(x)) {
        for tr in ["Iterator", "DoubleEndedIterator", "ExactSizeIterator"] {
            if !traits.iter().any(|t| t == tr) {
                traits.push(tr.to_string());
            }
        }
    }
    v.extend(traits);
    OWNERS.with(|m| m.borrow_mut().insert(key, (v.clone(), nc)));
    (v, nc)
}

/// ブランケットの impl (`impl<T: Display> ToString for T`) の trait の名前。only_common なら補完に出すよく使うものだけ
fn blanket_traits(ws: &[&FileIndex], file: usize, only_common: bool) -> Vec<String> {
    let kr = ws[file].krate;
    let mut out: Vec<String> = vec![];
    for (i, w) in ws.iter().enumerate() {
        if !(i == file || w.in_src || w.lib || w.krate == kr) {
            continue;
        }
        for (s, t) in &w.impls {
            if s == "*" && !out.contains(t) && (!only_common || !w.lib || matches!(t.as_str(), "ToString" | "Into" | "TryInto" | "ToOwned" | "Borrow" | "BorrowMut")) {
                out.push(t.clone());
            }
        }
    }
    out
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
    member_def(ws, file, o, name, kinds).map(|x| x.0)
}

/// 受け手の型 o のメンバ name の (型, 定義)
fn member_def<'a>(ws: &[&'a FileIndex], file: usize, o: &str, name: &str, kinds: &[SymKind]) -> Option<(String, &'a Def)> {
    let os = owner_names(ws, file, &type_head(o));
    let kr = ws[file].krate;
    // (ワークスペースのものを先に。ライブラリは lib_rank の順)
    let find = |os: &[String]| {
        ws.iter()
            .enumerate()
            .filter(|(i, w)| *i == file || w.in_src || w.lib || w.krate == kr)
            .flat_map(|(_, w)| w.defs.iter().map(move |d| (w.lib, d)))
            .filter(|(_, d)| d.name == name && kinds.contains(&d.kind) && os.contains(&d.owner) && !d.ty.is_empty())
            .min_by_key(|(lib, d)| (*lib, if *lib { lib_rank(d) } else { 0 }))
            .map(|x| x.1)
    };
    // (なければブランケットの impl の trait のメソッド)
    let d = find(&os).or_else(|| if kinds.contains(&SymKind::Method) { find(&blanket_traits(ws, file, false)) } else { None })?;
    Some((def_subst(d, o, &d.ty), d))
}

/// 定義 d (受け手の型 o のメンバ) の型の書き方 s の中の型引数を、受け手の型から置き換える (impl の self 型の型引数・Self・
/// 数の型のマクロの持ち主・Iterator の impl の決まらない型引数)
fn def_subst(d: &Def, o: &str, s: &str) -> String {
    let params = if d.container.starts_with("impl") {
        targs(impl_self_full(&d.container))
    } else if d.kind == SymKind::Field {
        d.params.clone()
    } else {
        vec![]
    };
    let args = targs(o);
    let mut t = subst(s, &params, &args);
    // (impl の型引数のうち self 型から決まらないもの (`impl<B, I, F> Iterator for Map<I, F>` の B): 受け手が繰り返しなら
    // その要素の型 (Iterator の impl ではふつう Item))
    // (impl の名前は `impl<..>` の型引数を持たないので、大文字 1 文字の名前で見分ける)
    if d.container.starts_with("impl") && d.container.contains("Iterator for ") && is_iter_head(&type_head(o)) {
        // (代入の前の宣言の型から: 利用者の型 `D` などと取り違えない)
        let mut unbound: Vec<String> = d.ty.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| w.len() == 1 && w.starts_with(|c: char| c.is_ascii_uppercase()) && !params.iter().any(|p| p == w)).map(|w| w.to_string()).collect();
        unbound.dedup();
        if unbound.len() == 1 {
            if let Some(item) = iter_item(o) {
                t = subst(&t, &unbound, &[item]);
            }
        }
    }
    // (trait に書かれたメソッドの Self は受け手の型。数の型のマクロ (`uint_impl!`) の中のメソッドの持ち主も)
    subst(&t, &["Self".to_string(), "__uint".to_string(), "__int".to_string()], &[o.to_string(), o.to_string(), o.to_string()]).replace("$SelfT", o).replace("$ActualT", o)
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
    // (タプルの添字 `x.0`: 数のトークンの前が `.`)
    if f.toks[k].0 == 1 && f.text.as_bytes()[f.toks[k].1 as usize].is_ascii_digit() && k >= 2 && f.sym(k - 1, b'.') && !(k >= 3 && f.sym(k - 2, b'.')) {
        let i: usize = f.tok(k).parse().ok()?;
        let base = expr_type(ws, file, k - 2, depth + 1)?;
        let base = expand(ws, file, strip_ref(&base)).into_iter().last()?;
        return match tuple_items(strip_ref(&base)) {
            Some(v) => v.get(i).cloned(),
            // (タプル構造体 `struct P(A, B)` のフィールド: 名前のないフィールドは持たないので分からない)
            None => None,
        };
    }
    if let Some(t) = literal_type(f, k) {
        return Some(t);
    }
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
            // (`a::b::P` / `m::CONST`: モジュールの下の単位構造体・const)
            if q.starts_with(|c: char| c.is_ascii_lowercase()) {
                return all().map(|x| x.1).find(|d| d.name == r && d.owner.is_empty() && matches!(d.kind, SymKind::Struct | SymKind::Const | SymKind::Static)).map(|d| if d.kind == SymKind::Struct { r.to_string() } else { d.ty.clone() });
            }
            return member_type(ws, file, &q, r, &[SymKind::Const, SymKind::Variant]).or(Some(q).filter(|_| r.starts_with(|c: char| c.is_ascii_uppercase())));
        }
        if let Some(b) = f.local(k, r) {
            return binding_type(ws, file, b, depth + 1);
        }
        // (単位構造体の値 `K`)
        if r.starts_with(|c: char| c.is_ascii_uppercase()) && named(ws, file, r).iter().any(|(_, d)| d.kind == SymKind::Struct && d.owner.is_empty()) {
            return Some(r.to_string());
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
        // ターボフィッシュ `x.collect::<Vec<_>>()` / `x.parse::<u8>()`
        if open == b'(' && f.sym(m, b'>') {
            let mut q = m;
            let mut dd = 0;
            while q > 0 {
                if f.sym(q, b'>') && !f.sym(q - 1, b'-') {
                    dd += 1;
                } else if f.sym(q, b'<') {
                    dd -= 1;
                    if dd == 0 {
                        break;
                    }
                }
                q -= 1;
            }
            if q >= 4 && f.sym(q - 1, b':') && f.sym(q - 2, b':') && f.ident(q - 3) && f.sym(q - 4, b'.') && q >= 5 {
                let tb = f.text[f.toks[q + 1].1 as usize..f.toks[m - 1].2 as usize].to_string();
                let o = expr_type(ws, file, q - 5, depth + 1)?;
                return method_call_type_at(ws, file, &o, f.tok(q - 3), m + 1, Some(&tb), depth);
            }
            return None;
        }
        // 括弧の式 `(e)` とタプルの式 `(a, b)` (前が呼ぶもの (名前・`)`・`]`) でないとき)
        if open == b'(' && !(f.ident(m) && !KEYWORDS.contains(&f.tok(m))) && !f.sym(m, b')') && !f.sym(m, b']') && !f.sym(m, b'!') {
            let mut ends = vec![];
            let mut dd = 0;
            for q in p + 1..k {
                if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
                    dd += 1;
                } else if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
                    dd -= 1;
                } else if dd == 0 && f.sym(q, b',') {
                    ends.push(q - 1);
                }
            }
            let trailing = ends.last().is_some_and(|&e| e + 2 == k);
            if !trailing && k > p + 1 {
                ends.push(k - 1);
            }
            if ends.is_empty() {
                return Some("()".to_string());
            }
            if ends.len() == 1 && !trailing {
                return expr_type(ws, file, ends[0], depth + 1);
            }
            // (要素の型が一つでも分からなければ答えない)
            let ts: Vec<String> = ends.iter().map(|&e| expr_type(ws, file, e, depth + 1)).collect::<Option<Vec<_>>>()?;
            return Some(format!("({})", ts.join(", ")));
        }
        if open == b'[' {
            // 添字 `a[i]` (前が式の終わりのとき。`t.1[0]` のタプルの添字も)
            let tuple_idx = f.toks[m].0 == 1 && f.text.as_bytes()[f.toks[m].1 as usize].is_ascii_digit() && m >= 1 && f.sym(m - 1, b'.');
            if f.ident(m) || f.sym(m, b')') || f.sym(m, b']') || tuple_idx {
                let t = expr_type(ws, file, m, depth + 1)?;
                return expand(ws, file, &t).iter().find_map(|t| index_type(t, range));
            }
            // 配列の式 `[a, b]` / `[x; n]`: `[T; N]` (要素の型は最初の要素の)
            let mut first_end = None;
            let mut n = 0;
            let mut rep: Option<usize> = None;
            let mut dd = 0;
            for q in p + 1..k {
                if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
                    dd += 1;
                } else if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
                    dd -= 1;
                } else if dd == 0 && f.sym(q, b',') {
                    first_end.get_or_insert(q - 1);
                    n += 1;
                } else if dd == 0 && f.sym(q, b';') {
                    first_end.get_or_insert(q - 1);
                    rep = Some(q);
                }
            }
            if k == p + 1 {
                return None;
            }
            if !f.sym(k - 1, b',') && rep.is_none() {
                n += 1;
            }
            let e0 = first_end.unwrap_or(k - 1);
            let et = expr_type(ws, file, e0, depth + 1)?;
            let len = match rep {
                Some(q) => f.text[f.toks[q + 1].1 as usize..f.toks[k - 1].2 as usize].trim().to_string(),
                None => n.to_string(),
            };
            return Some(format!("[{et}; {len}]"));
        }
        // match / if / ブロックの式 (構造体の式 `T { .. }` でないもの): 腕・ブロックの最後の式のうち型の分かる最初のもの
        if open == b'{' && !(f.ident(m) && (f.tok(m).starts_with(|c: char| c.is_ascii_uppercase()) || f.tok(m) == "Self")) {
            let s = block_expr_start(f, p)?;
            let mut tails = vec![];
            expr_tails(f, file, s, k, &mut tails);
            for l in tails {
                let e = f.toks.partition_point(|t| (t.2 as usize) < l.end);
                if e < k {
                    if let Some(t) = expr_type(ws, file, e, depth + 1) {
                        return Some(t);
                    }
                }
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
            if !name.starts_with(|c: char| c.is_ascii_uppercase()) {
                return None;
            }
            // (型引数のある struct: フィールドの初期化の式から型引数を決める `Wrapper { inner: Point::new() }` → Wrapper<Point>)
            let params = all().map(|x| x.1).find(|d| d.name == name && d.owner.is_empty() && d.kind == SymKind::Struct).map(|d| d.params.clone()).unwrap_or_default();
            if params.is_empty() {
                return Some(name.to_string());
            }
            let mut args = vec!["_".to_string(); params.len()];
            let mut j = p + 1;
            while j < k {
                if f.ident(j) && (f.sym(j - 1, b'{') || f.sym(j - 1, b',')) && (f.sym(j + 1, b':') && !f.sym(j + 2, b':')) {
                    // (`field: EXPR` の EXPR の終わり)
                    let mut e = j + 2;
                    let mut d = 0;
                    while e < k {
                        if f.sym(e, b'(') || f.sym(e, b'[') || f.sym(e, b'{') {
                            d += 1;
                        } else if f.sym(e, b')') || f.sym(e, b']') || f.sym(e, b'}') {
                            d -= 1;
                        } else if d == 0 && f.sym(e, b',') {
                            break;
                        }
                        e += 1;
                    }
                    let field = f.tok(j);
                    if let Some(fd) = all().map(|x| x.1).find(|d| d.kind == SymKind::Field && d.owner == name && d.name == field) {
                        if let Some(pi) = params.iter().position(|p| *p == fd.ty) {
                            if let Some(t) = expr_type(ws, file, e - 1, depth + 1) {
                                args[pi] = t;
                            }
                        }
                    }
                    j = e;
                }
                j += 1;
            }
            return Some(format!("{name}<{}>", args.join(", ")));
        }
        if m >= 1 && f.sym(m - 1, b'.') {
            let o = expr_type(ws, file, m - 2, depth + 1)?;
            return method_call_type(ws, file, &o, name, m, depth);
        }
        if f.after_path(m) && m >= 3 && f.ident(m - 3) {
            let q = f.tok(m - 3).to_string();
            let q = if q == "Self" { self_ty(at)? } else { q };
            // (`a::b::T(..)` / `m::f(..)`: モジュールの下のタプル構造体・関数)
            if q.starts_with(|c: char| c.is_ascii_lowercase()) && q != "self" && q != "super" && q != "crate" {
                if let Some(t) = all().map(|x| x.1).find(|d| d.name == name && d.owner.is_empty() && matches!(d.kind, SymKind::Struct | SymKind::Function)).map(|d| if d.kind == SymKind::Struct { name.to_string() } else { d.ty.clone() }).filter(|t| !t.is_empty()) {
                    return Some(t);
                }
            }
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
    // `V(a, b)` / `E::V(a, b)` のパターンの中の束縛: バリアントの宣言の型
    if let Some(t) = variant_binding_type(ws, file, b, depth) {
        return Some(t);
    }
    // クロージャの引数 `x.iter().map(|d| ..)` / `opt.map(|v| ..)`: 受け手の要素の型
    if let Some(t) = closure_param_type(ws, file, b, depth) {
        return Some(t);
    }
    // `let x = EXPR;`
    if f.sym(b + 1, b'=') && !f.sym(b + 2, b'=') && b >= 1 && (f.tok(b - 1) == "let" || f.tok(b - 1) == "mut") {
        // (クロージャ `|a: T, b: U| body` → `impl Fn(T, U) -> R`)
        let c = if f.tok(b + 2) == "move" { b + 3 } else { b + 2 };
        if f.sym(c, b'|') && !f.sym(c + 1, b'|') {
            let mut j = c + 1;
            let mut tys = vec![];
            while j < n && !f.sym(j, b'|') {
                if f.sym(j, b':') {
                    let a = j + 1;
                    while j + 1 < n && !f.sym(j + 1, b',') && !f.sym(j + 1, b'|') {
                        j += 1;
                    }
                    tys.push(f.text[f.toks[a].1 as usize..f.toks[j].2 as usize].to_string());
                } else if f.sym(j, b',') && tys.len() < f.text[f.toks[c].1 as usize..f.toks[j].1 as usize].matches(',').count() {
                    tys.push("_".to_string());
                }
                j += 1;
            }
            let ret = expr_end(f, j + 1, false).and_then(|e| expr_type(ws, file, e, depth + 1)).map(|r| format!(" -> {r}")).unwrap_or_default();
            return Some(format!("impl Fn({}){ret}", tys.join(", ")));
        }
        if let Some(e) = expr_end(f, b + 2, false) {
            if let Some(t) = expr_type(ws, file, e, depth + 1) {
                return Some(t);
            }
        }
    }
    f.local_type(b)
}

/// パターンの中の束縛 b の型: `E::V(.., b, ..)` の V の宣言の、その位置の型。照合する式の型が分かれば型引数を当てる
fn variant_binding_type(ws: &[&FileIndex], file: usize, b: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    if !(b >= 1 && (f.sym(b - 1, b'(') || f.sym(b - 1, b',') || matches!(f.tok(b - 1), "ref" | "mut")) && (f.sym(b + 1, b',') || f.sym(b + 1, b')'))) {
        return None;
    }
    // (囲む `(` と、b がその中の何番目か)
    let mut j = b;
    let mut d = 0;
    let mut idx = 0;
    loop {
        if j == 0 {
            return None;
        }
        j -= 1;
        if f.sym(j, b')') || f.sym(j, b']') || f.sym(j, b'}') {
            d += 1;
        } else if f.sym(j, b'(') || f.sym(j, b'[') || f.sym(j, b'{') {
            if d == 0 {
                break;
            }
            d -= 1;
        } else if d == 0 && f.sym(j, b',') {
            idx += 1;
        } else if d == 0 && (f.sym(j, b';') || f.sym(j, b'=')) {
            return None;
        }
    }
    if !f.sym(j, b'(') || j == 0 || !f.ident(j - 1) {
        return None;
    }
    let v = f.tok(j - 1);
    if !v.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    let scrut = || scrutinee_type(ws, file, b, depth + 1);
    match v {
        "Some" | "Ok" | "Err" => {
            let t = scrut()?;
            let want = if v == "Some" { "Option" } else { "Result" };
            let t = expand(ws, file, &t).into_iter().find(|t| type_head(t) == want)?;
            return targs(&t).into_iter().nth(if v == "Err" { 1 } else { 0 }).filter(|t| t != "_");
        }
        _ => {}
    }
    let q = if f.after_path(j - 1) && j >= 4 { Some(f.tok(j - 4)) } else { None };
    let kr = f.krate;
    let (vi, vd) = ws
        .iter()
        .enumerate()
        .filter(|(i, w)| *i == file || w.in_src || w.lib || w.krate == kr)
        .flat_map(|(i, w)| w.defs.iter().map(move |d| (i, d)))
        .find(|(_, d)| d.kind == SymKind::Variant && d.name == v && q.is_none_or(|q| d.owner == q || (q == "Self" && f.enclosing_owner(f.toks[b].1 as usize).as_deref() == Some(&d.owner))))?;
    let vt = ws[vi].text.get(vd.item_start..vd.item_end)?;
    let inner = &vt[vt.find('(')? + 1..vt.rfind(')')?];
    let tys = targs(&format!("X<{inner}>"));
    let ty = tys.get(idx)?.clone();
    // (照合する式の型の型引数を、enum の型引数に当てる)
    if !vd.params.is_empty() {
        if let Some(st) = scrut() {
            if let Some(st) = expand(ws, file, &st).into_iter().find(|t| type_head(t) == vd.owner) {
                return Some(subst(&ty, &vd.params, &targs(&st)));
            }
        }
    }
    Some(ty)
}

/// パターンの中のトークン k に照合する式の型 (`match EXPR { P => }` / `let P = EXPR` / `if let P = EXPR`)
fn scrutinee_type(ws: &[&FileIndex], file: usize, k: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    let n = f.toks.len();
    let mut j = k + 1;
    let mut d = 0i32;
    while j < n && j < k + 256 {
        if f.sym(j, b'(') || f.sym(j, b'[') || f.sym(j, b'{') {
            d += 1;
        } else if f.sym(j, b')') || f.sym(j, b']') || f.sym(j, b'}') {
            d -= 1;
        } else if f.sym(j, b';') {
            return None;
        } else if d <= 0 && f.sym(j, b'=') && f.sym(j + 1, b'>') {
            // (match: 腕の始まりの前の、囲む `{` の前が照合する式)
            let (a, _) = f.arm_pat(j);
            let mut q = a;
            let mut dd = 0;
            while q > 0 {
                q -= 1;
                if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
                    dd += 1;
                } else if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
                    if dd == 0 {
                        break;
                    }
                    dd -= 1;
                }
            }
            return if f.sym(q, b'{') && q >= 1 { expr_type(ws, file, q - 1, depth + 1) } else { None };
        } else if d <= 0 && f.sym(j, b'=') && !f.sym(j + 1, b'=') {
            let e = expr_end(f, j + 1, true)?;
            return expr_type(ws, file, e, depth + 1);
        }
        j += 1;
    }
    None
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
                let mut os = owners_of(ws, file, &o);
                os.extend(blanket_traits(ws, file, true));
                for (i, d) in all() {
                    // (`self` を取らない関連関数 (`default()`) は `.` の後に出さない: rust-analyzer と同じ)
                    if kinds.contains(&d.kind) && os.contains(&d.owner) && (d.kind != SymKind::Method || d.has_self) {
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
    // (文字列・文字のリテラル: 型と値。rust-analyzer と同じ形)
    if ws[file].tok_at(off).is_none() && ws[file].capture_at(off).is_none() {
        if let Some(h) = literal_hover(ws[file], off) {
            return Some(h);
        }
    }
    // (組み込みの型)
    if let Some(k) = ws[file].tok_at(off) {
        let w = ws[file].tok(k);
        if PRIMS.contains(&w) {
            return Some(format!("```rust\n{w}\n```"));
        }
    }
    let d = def_at(ws, file, off);
    let mut t = *d.first()?;
    // (derive が作るメソッド (行き先は `#[derive(Default)]` の名前): その trait のメソッドの宣言を見せる)
    if !ws[t.file].slim && ws[t.file].tok_at(t.start).is_some_and(|j| ws[t.file].in_derive(j)) {
        let tr = ws[t.file].text[t.start..t.end].to_string();
        if let Some(name) = ws[file].tok_at(off).map(|k| ws[file].tok(k).to_string()) {
            if let Some((i, d)) = named(ws, file, &name).into_iter().find(|(_, d)| d.owner == tr) {
                t = loc(i, d);
            }
        }
    }
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

    /// 参照の数え方を rust-analyzer に合わせたところ: impl の中の `Self`、パターン・引数の束縛、知らない外部のパス、
    /// cfg で外れたアイテム、組み込みの型
    #[test]
    fn references_like_rust_analyzer() {
        let src = "fn concrete() {}
struct P { x: u8 }
impl P {
    fn new() -> Self { Self { x: 0u8 } }
}
                   fn f(concrete: bool) {
    let (a, concrete) = (1, 2);
    let _ = syn::P;
    let _ = Delimiter::None;
}
                   #[cfg(feature = \"off\")]
fn g() -> P { P::new() }
#[cfg(not(feature = \"off\"))]
fn g() -> u8 { 1 }
                   fn u8() {}
";
        let mut fi = FileIndex::new(src.to_string(), vec![]);
        fi.apply_cfg(vec![]);
        let ws = [&fi];
        let no = &|_: usize| None;
        // P: 定義・impl の名前・`Self` 2 つと、cfg で外れた g の中の 2 つ (`-> P` と `P::new`。rust-analyzer は外れた
        // アイテムの中もパスは解決する)。`syn::P` は外部のもの
        assert_eq!(references(&ws, 0, at(src, "P {", 0), true, no).len(), 6);
        // `fn f(concrete: ..)` と `let (a, concrete)` は束縛そのもの: 関数 concrete の参照ではない
        assert_eq!(references(&ws, 0, 3, true, no).len(), 1);
        assert_eq!(def_text(&ws, 0, at(src, "concrete) =", 0)).map(|x| x.2), Some(at(src, "concrete) =", 0)));
        // `Delimiter::None` は Option の None ではない (Delimiter を知らない)
        assert!(definition(&ws, 0, at(src, "None", 0)).is_empty());
        // cfg で外れた g の中の関連アイテム `P::new` には答えない (パスの P には答える)
        assert!(definition(&ws, 0, at(src, "new()", 1)).is_empty());
        assert_eq!(definition(&ws, 0, at(src, "P::new", 0)).first().map(|l| l.start), Some(at(src, "P {", 0)));
        // u8: 型の位置だけ (`0u8` の接尾辞と `fn u8` の名前は除く)
        assert_eq!(references(&ws, 0, at(src, "u8 }", 0), true, no).len(), 2);
    }
}

#[cfg(test)]
#[path = "navigate_tests.rs"]
mod unit_tests;

/// textDocument/prepareRename: off の名前が名前を変えられるものなら、その名前の範囲
pub fn prepare_rename(ws: &[&FileIndex], file: usize, off: usize) -> Result<Loc, String> {
    clear_memo();
    let f = ws[file];
    let k = f.tok_at(off).ok_or("名前の上ではない")?;
    let w = f.tok(k);
    if KEYWORDS.contains(&w) || PRIMS.contains(&w) {
        return Err(format!("`{w}` は名前を変えられない"));
    }
    let t = *def_at(ws, file, off).first().ok_or("定義が分からない")?;
    let tf = ws[t.file];
    // (ライブラリのもの: `use a::B;` の B の上でだけ、別名を付ける形で変えられる (rust-analyzer と同じ))
    if tf.lib && lib_use(f, w) != Some(f.toks[k].1 as usize) {
        return Err("ライブラリの定義は名前を変えられない (use の名前の上なら別名を付けられる)".into());
    }
    if mod_file(ws, t).is_some() {
        return Err("ファイルのモジュールの名前は変えられない (ファイルの名前も変わるので)".into());
    }
    // (trait の impl のメソッド: trait がワークスペースのものなら trait の宣言と全部の impl を変える。ライブラリ・外部の
    // クレートの trait (`impl Default for S` の default) なら断る: 宣言の方を変えられないので)
    if let Some(d) = tf.defs.iter().find(|d| d.start == t.start && d.kind == SymKind::Method) {
        if let Some(tr) = impl_trait(&d.container) {
            if trait_method(ws, tr, &d.name).is_none() {
                return Err(format!("`{tr}` (ライブラリか外部のクレートの trait) のメソッドは名前を変えられない"));
            }
        }
    }
    // (derive の名前 `#[derive(Clone)]` に解決されたもの: derive が作るメソッド)
    let is_def = tf.defs.iter().any(|d| d.start == t.start);
    if !is_def && !tf.slim && tf.tok_at(t.start).is_some_and(|j| tf.in_derive(j)) {
        return Err("derive が作るメソッドは名前を変えられない".into());
    }
    let (a, b) = (f.toks[k].1 as usize, f.toks[k].2 as usize);
    Ok(Loc { file, start: a, end: b })
}

/// ワークスペースの trait tr の宣言の中のメソッド name
fn trait_method(ws: &[&FileIndex], tr: &str, name: &str) -> Option<Loc> {
    ws.iter().enumerate().filter(|(_, w)| !w.lib).find_map(|(i, w)| {
        w.defs.iter().find(|d| d.kind == SymKind::Method && d.name == name && !d.container.starts_with("impl") && &d.container[..] == tr && w.defs.iter().any(|x| x.kind == SymKind::Trait && x.name == tr)).map(|d| loc(i, d))
    })
}

/// ファイル f の `use a::name;` (別名なし) の、パスの最後の名前の byte 位置
fn lib_use(f: &FileIndex, name: &str) -> Option<usize> {
    f.uses.iter().zip(&f.use_at).find(|(u, a)| u.0 == name && u.1.last().is_some_and(|l| l == name) && **a != usize::MAX).map(|(_, &a)| a)
}

/// 識別子として使える名前か (キーワードは除く。`r#` の付いたものは良い)
fn valid_ident(s: &str) -> bool {
    let n = s.strip_prefix("r#").unwrap_or(s);
    let mut c = n.chars();
    let Some(h) = c.next() else { return false };
    (h == '_' || h.is_alphabetic()) && c.all(|x| x == '_' || x.is_alphanumeric()) && n != "_" && (s.starts_with("r#") || !KEYWORDS.contains(&n))
}

/// textDocument/rename: 書き換え (位置, 新しい文字列) の列。構造体の省略形 `S { x }` は `S { new: x }` /
/// `S { x: new }` に書き換える
pub fn rename(ws: &[&FileIndex], file: usize, off: usize, new: &str, load: Load) -> Result<Vec<(Loc, String)>, String> {
    let at = prepare_rename(ws, file, off)?;
    if !valid_ident(new) {
        return Err(format!("`{new}` は名前にできない"));
    }
    let old = ws[file].text[at.start..at.end].to_string();
    let t = *def_at(ws, file, off).first().ok_or("定義が分からない")?;
    // `use` の名前の上: `use std::sync::Mutex;` → `use std::sync::Mutex as New;` と、このファイルの中の Mutex
    if lib_use(ws[file], &old) == Some(at.start) {
        let f = ws[file];
        let u = at.start;
        let mut out = vec![(Loc { file, start: u + old.len(), end: u + old.len() }, format!(" as {new}"))];
        for &k in f.idents.get(old.as_str()).map(|v| v.as_slice()).unwrap_or(&[]) {
            let a = f.toks[k as usize].1 as usize;
            if a != u && !f.in_inactive(a) && !f.after_path(k as usize) && def_at(ws, file, a).first() == Some(&t) {
                out.push((Loc { file, start: a, end: a + old.len() }, new.to_string()));
            }
        }
        return Ok(out);
    }
    let is_field = ws[t.file].defs.iter().any(|d| d.start == t.start && d.kind == SymKind::Field);
    // (trait の impl のメソッド: trait の宣言から引く (宣言・全部の impl・呼び出し))
    let root = ws[t.file].defs.iter().find(|d| d.start == t.start && d.kind == SymKind::Method).and_then(|d| impl_trait(&d.container).and_then(|tr| trait_method(ws, tr, &d.name)));
    let refs = match root {
        Some(r) if (r.file, r.start) != (t.file, t.start) => {
            let held;
            let mut ws2: Vec<&FileIndex> = ws.to_vec();
            if ws[r.file].slim {
                held = load(r.file).ok_or("ファイルを読めない")?;
                ws2[r.file] = &held;
            }
            references(&ws2, r.file, r.start, true, load)
        }
        _ => references(ws, file, off, true, load),
    };
    let mut held: FxMap<usize, Rc<FileIndex>> = FxMap::default();
    let mut out = vec![];
    for l in refs {
        let fi: &FileIndex = if ws[l.file].slim {
            if !held.contains_key(&l.file) {
                let Some(r) = load(l.file) else { continue };
                held.insert(l.file, r);
            }
            &held[&l.file]
        } else {
            ws[l.file]
        };
        if fi.text.get(l.start..l.end) != Some(old.as_str()) {
            continue;
        }
        // (省略形 `T { x }`: rust-analyzer と同じく書き足す形に。フィールドなら前に `new: `、ローカルなら後ろに `: new`)
        match fi.tok_at(l.start) {
            Some(k) if fi.toks[k].0 == 0 && (fi.sym(k.wrapping_sub(1), b'{') || fi.sym(k.wrapping_sub(1), b',')) && (fi.sym(k + 1, b',') || fi.sym(k + 1, b'}')) && fi.struct_lit_at(k).is_some() => {
                if is_field {
                    out.push((Loc { file: l.file, start: l.start, end: l.start }, format!("{new}: ")));
                } else {
                    out.push((Loc { file: l.file, start: l.end, end: l.end }, format!(": {new}")));
                }
            }
            _ => out.push((l, new.to_string())),
        }
    }
    out.sort_by_key(|x| (x.0.file, x.0.start));
    out.dedup_by_key(|x| (x.0.file, x.0.start));
    if out.is_empty() {
        return Err(format!("`{old}` の書き換える所が見つからない (use の別名なら、別名の付いた use の上で変える)"));
    }
    Ok(out)
}

/// textDocument/signatureHelp: off を囲む呼び出し `f(..|..)` の (シグネチャ, 引数の byte 範囲 (シグネチャの中), 今の引数)
pub fn signature_help(ws: &[&FileIndex], file: usize, off: usize, load: Load) -> Option<(String, Vec<(usize, usize)>, usize, String)> {
    clear_memo();
    let f = ws[file];
    // (off より前のトークンから戻って、閉じていない `(` を探す。数えるのは深さ 0 のカンマ)
    let mut j = f.toks.partition_point(|t| (t.1 as usize) < off);
    let mut d = 0i32;
    let mut commas = 0;
    let open = loop {
        if j == 0 {
            return None;
        }
        j -= 1;
        if f.toks[j].0 != 1 {
            continue;
        }
        match f.text.as_bytes()[f.toks[j].1 as usize] {
            b')' | b']' | b'}' => d += 1,
            b'[' => {
                // (引数の中の配列・`vec![..]`: そこまでのカンマは配列の中のもの)
                if d == 0 {
                    commas = 0;
                } else {
                    d -= 1;
                }
            }
            b'{' => {
                if d == 0 {
                    return None;
                }
                d -= 1;
            }
            b'(' => {
                if d == 0 {
                    break j;
                }
                d -= 1;
            }
            b',' if d == 0 => commas += 1,
            b';' if d == 0 => return None,
            _ => {}
        }
    };
    // (呼ぶもの: `f(` / `x.m(` / `T::f(` / `f::<T>(`)
    let mut c = open.checked_sub(1)?;
    if f.sym(c, b'>') {
        let mut dd = 0;
        while c > 0 {
            if f.sym(c, b'>') {
                dd += 1;
            } else if f.sym(c, b'<') {
                dd -= 1;
                if dd == 0 {
                    break;
                }
            }
            c -= 1;
        }
        c = c.checked_sub(3)?;
    }
    // (宣言 `fn f(..)` / `struct T(..)` の中は呼び出しではない)
    if !f.ident(c) || KEYWORDS.contains(&f.tok(c)) || (c >= 1 && f.ident(c - 1) && matches!(f.tok(c - 1), "fn" | "struct")) {
        return None;
    }
    let method_call = c >= 1 && f.sym(c - 1, b'.');
    let mut t = *def_at(ws, file, f.toks[c].1 as usize).first()?;
    // (関数を入れたローカル `let score = lib_rank;`: その関数)
    if !ws[t.file].defs.iter().any(|x| x.start == t.start) && !ws[t.file].slim {
        let tf = ws[t.file];
        let b = tf.toks.partition_point(|x| (x.1 as usize) < t.start);
        if tf.sym(b + 1, b'=') && tf.ident(b + 2) && tf.sym(b + 3, b';') {
            if let Some(&u) = def_at(ws, t.file, tf.toks[b + 2].1 as usize).first() {
                if ws[u.file].defs.iter().any(|x| x.start == u.start && matches!(x.kind, SymKind::Function | SymKind::Method)) {
                    t = u;
                }
            }
        }
    }
    let tf = ws[t.file];
    let (sig, name_at, doc) = match tf.defs.iter().find(|x| x.start == t.start) {
        Some(def) => {
            if !matches!(def.kind, SymKind::Function | SymKind::Method | SymKind::Struct | SymKind::Variant) {
                return None;
            }
            let held;
            let text: &str = if tf.slim {
                held = load(t.file)?;
                &held.text
            } else {
                &tf.text
            };
            let sig = signature(text.get(def.item_start..def.item_end.min(text.len()))?, def.kind);
            let at = sig.find(def.name.as_str()).unwrap_or(0) + def.name.len();
            (sig, at, doc_comment(text, def.item_start))
        }
        // (ローカルの束縛: クロージャ `let f = |a: T| ..` なら `impl Fn(a: T) -> R`)
        None => (closure_sig(ws, t)?, 0, String::new()),
    };
    // (引数の並び: 名前の後の最初の `(` .. `)`。型引数 `<..>` の中は飛ばす)
    let b = sig.as_bytes();
    let mut i = name_at;
    let mut dg = 0;
    while i < b.len() {
        match b[i] {
            b'<' => dg += 1,
            b'>' if i > 0 && b[i - 1] != b'-' => dg -= 1,
            b'(' if dg == 0 => break,
            _ => {}
        }
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    let mut params = vec![];
    let mut dd = 0;
    let mut s = i + 1;
    let mut k = i + 1;
    while k < b.len() {
        match b[k] {
            b'(' | b'[' | b'<' => dd += 1,
            b'>' if b[k - 1] == b'-' => {}
            b')' | b']' | b'>' if dd > 0 => dd -= 1,
            b')' => break,
            b',' if dd == 0 => {
                params.push((s, k));
                s = k + 1;
            }
            _ => {}
        }
        k += 1;
    }
    if k > s && !sig[s..k].trim().is_empty() {
        params.push((s, k));
    }
    let params: Vec<(usize, usize)> = params
        .into_iter()
        .map(|(a, e)| {
            let lead = sig[a..e].len() - sig[a..e].trim_start().len();
            (a + lead, a + sig[a..e].trim_end().len())
        })
        .collect();
    let has_self = params.first().is_some_and(|&(a, e)| {
        let p = &sig[a..e];
        p == "self" || p.ends_with("self") || p.starts_with("self:") || p.starts_with("mut self")
    });
    // (メソッドの呼び出し `x.m(..)` では self は引数に数えない: rust-analyzer と同じ)
    let params = if method_call && has_self { params[1..].to_vec() } else { params };
    Some((sig, params, commas, doc))
}

/// closure_param_type の、メソッドのシグネチャから決める方: 受け手 recv のメソッド m の、`(` (open) の後の引数のうち
/// クロージャ (`|` は j) のものの束縛の引数の形の idx 番目 (path はタプルのパターンの中の番号)
#[allow(clippy::too_many_arguments)]
fn closure_param_from_sig(ws: &[&FileIndex], file: usize, recv: &str, m: &str, open: usize, j: usize, idx: usize, path: &[usize], depth: u32) -> Option<String> {
    let f = ws[file];
    // (クロージャが何番目の引数か、と引数の始め)
    let mut starts = vec![open + 1];
    let mut dd = 0;
    for q in open + 1..j {
        if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
            dd += 1;
        } else if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
            dd -= 1;
        } else if dd == 0 && f.sym(q, b',') {
            starts.push(q + 1);
        }
    }
    let argi = starts.len() - 1;
    let (o, d) = expand(ws, file, recv).into_iter().find_map(|t| member_def(ws, file, &t, m, &[SymKind::Method]).map(|(_, d)| (t, d)))?;
    let (_, _, _, args) = d.fnret.iter().find(|x| x.0 as usize == argi && x.2 != "=")?;
    let ps = tuple_items(&format!("({args})"))?;
    let pat = ps.get(idx)?.clone();
    let mut ty = def_subst(d, &o, &pat);
    // (メソッドの型引数そのものの引数 `fold(init: B, ..)`: その引数の式の型)
    for (i, g, pat, _) in &d.fnret {
        if pat == "=" {
            if let Some(&s) = starts.get(*i as usize) {
                if let Some(at) = expr_end(f, s, false).and_then(|e| expr_type(ws, file, e, depth + 1)) {
                    ty = subst(&ty, &[g.clone()], &[at]);
                }
            }
        }
    }
    let mut ty = strip_lifetimes(&norm_proj(&ty));
    // (形の中の型引数 (大文字 1 文字) が置き換わらずに残っていれば分からない)
    let words = |s: &str| s.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| w.len() == 1 && w.starts_with(|c: char| c.is_ascii_uppercase())).map(|w| w.to_string()).collect::<Vec<_>>();
    let before = words(&pat);
    if words(&ty).iter().any(|w| before.contains(w) && !targs(&o).contains(w)) || ty.contains("::") {
        return None;
    }
    for &i in &path[1..] {
        let inner = strip_ref(&ty).to_string();
        ty = tuple_items(&inner)?.get(i)?.clone();
    }
    Some(ty)
}

/// タプルの型 `(A, B<C, D>)` の要素
fn tuple_items(t: &str) -> Option<Vec<String>> {
    let t = t.trim();
    let inner = t.strip_prefix('(')?.strip_suffix(')')?;
    let mut out = vec![];
    let mut d = 0;
    let mut a = 0;
    let b = inner.as_bytes();
    for i in 0..=b.len() {
        let c = if i < b.len() { b[i] } else { b',' };
        match c {
            b'<' | b'(' | b'[' => d += 1,
            b'>' if i > 0 && b[i - 1] != b'-' => d -= 1,
            b')' | b']' => d -= 1,
            b',' if d == 0 => {
                let x = inner[a..i].trim();
                if !x.is_empty() {
                    out.push(x.to_string());
                }
                a = i + 1;
            }
            _ => {}
        }
    }
    Some(out)
}

/// 繰り返し (イテレータ) の型か (要素の型が iter_item で分かるもの)
fn is_iter_head(h: &str) -> bool {
    matches!(
        h,
        "Iter" | "IterMut" | "IntoIter" | "Drain" | "Map" | "Filter" | "FilterMap" | "Enumerate" | "Rev" | "Skip" | "Take" | "SkipWhile" | "TakeWhile" | "MapWhile"
            | "Peekable" | "Fuse" | "StepBy" | "Chain" | "Zip" | "Cloned" | "Copied" | "Flatten" | "FlatMap" | "Inspect" | "Scan" | "Cycle" | "Chars" | "CharIndices"
            | "Bytes" | "Lines" | "Split" | "SplitWhitespace" | "SplitN" | "RSplit" | "Keys" | "Values" | "ValuesMut" | "Windows" | "Chunks" | "Range" | "RangeInclusive"
    )
}

/// 繰り返しの型 t の要素の型 (`Iter<'_, T>` → &T、`Map<I, impl FnMut() -> R>` → R、`Enumerate<I>` → (usize, 要素) ..)。
/// 繰り返しでない集まり (Vec / Option / HashMap ..) は IntoIterator の要素 (値のとき) / 参照 (`&Vec<T>` → &T)
fn iter_item(t: &str) -> Option<String> {
    let t = t.trim();
    let by_ref = t.starts_with('&');
    let h = type_head(t);
    let a = targs(t);
    let first = || a.first().cloned();
    // (クロージャの型 `impl FnMut(..) -> R` の R)
    let ret_of = |f: &str| -> Option<String> {
        let i = f.find(") -> ")?;
        Some(f[i + 5..].trim().to_string())
    };
    let deref = |s: String| s.strip_prefix('&').map(|x| x.trim_start_matches("mut ").to_string()).unwrap_or(s);
    Some(match h.as_str() {
        "Iter" if a.len() == 2 => format!("(&{}, &{})", a[0], a[1]),
        "IterMut" if a.len() == 2 => format!("(&{}, &mut {})", a[0], a[1]),
        "Iter" | "Windows" | "Chunks" => format!("&{}", if h == "Iter" { first()? } else { format!("[{}]", first()?) }),
        "IterMut" => format!("&mut {}", first()?),
        "IntoIter" | "Drain" if a.len() == 2 => format!("({}, {})", a[0], a[1]),
        "IntoIter" | "Drain" | "Range" | "RangeInclusive" => first()?,
        "Keys" => format!("&{}", first()?),
        "Values" => format!("&{}", a.get(1)?),
        "ValuesMut" => format!("&mut {}", a.get(1)?),
        "Chars" => "char".to_string(),
        "CharIndices" => "(usize, char)".to_string(),
        "Bytes" => "u8".to_string(),
        "Lines" | "Split" | "SplitWhitespace" | "SplitN" | "RSplit" => "&str".to_string(),
        "Map" | "Scan" | "MapWhile" => {
            let r = ret_of(a.get(1).or(a.last())?)?;
            if h == "Map" { r } else { targs(&r).into_iter().next()? }
        }
        "FilterMap" => targs(&ret_of(a.get(1)?)?).into_iter().next()?,
        "FlatMap" => iter_item(&ret_of(a.get(2).or(a.get(1))?)?)?,
        "Flatten" => iter_item(&iter_item(&first()?)?)?,
        "Enumerate" => format!("(usize, {})", iter_item(&first()?)?),
        "Zip" => format!("({}, {})", iter_item(a.first()?)?, iter_item(a.get(1)?)?),
        "Cloned" | "Copied" => deref(iter_item(&first()?)?),
        "Filter" | "Rev" | "Skip" | "Take" | "SkipWhile" | "TakeWhile" | "Peekable" | "Fuse" | "StepBy" | "Chain" | "Inspect" | "Cycle" => iter_item(&first()?)?,
        "Vec" | "VecDeque" | "HashSet" | "BTreeSet" | "BinaryHeap" | "Option" | "Box" | "[T]" => {
            let e = if h == "[T]" { strip_ref(t).trim_start_matches('[').split([';', ']']).next()?.trim().to_string() } else { first()? };
            if by_ref { format!("&{e}") } else { e }
        }
        "HashMap" | "BTreeMap" => {
            if by_ref { format!("(&{}, &{})", a.first()?, a.get(1)?) } else { format!("({}, {})", a.first()?, a.get(1)?) }
        }
        _ => return None,
    })
}

/// 型の書き方の中の関連型の射影 `<A as Iterator>::Item` / `A::Item` / `Self::Item` を、分かれば要素の型に置き換える
fn norm_proj(t: &str) -> String {
    let mut s = t.to_string();
    for _ in 0..6 {
        // (`::Item` (要素) と `::IntoIter` (IntoIterator: 繰り返しならそれ自身、集まりなら IntoIter<要素>))
        let (i, plen, into) = match (s.find("::Item"), s.find("::IntoIter")) {
            (Some(a), Some(b)) if b < a => (b, 10, true),
            (Some(a), _) => (a, 6, false),
            (None, Some(b)) => (b, 10, true),
            (None, None) => break,
        };
        let b = s.as_bytes();
        // (`::Item` の前の型: `<A as Tr>` か、深さ 0 で区切り (`<` / `,` / `(` / 空白) まで)
        let (a, inner) = if i >= 1 && b[i - 1] == b'>' && s[..i].contains(" as ") {
            let mut d = 0;
            let mut j = i;
            while j > 0 {
                j -= 1;
                match b[j] {
                    b'>' if j >= 1 && b[j - 1] == b'-' => {}
                    b'>' => d += 1,
                    b'<' => {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let inner = &s[j + 1..i - 1];
            (j, inner.split(" as ").next().unwrap_or("").to_string())
        } else {
            let mut d = 0;
            let mut j = i;
            while j > 0 {
                let c = b[j - 1];
                if c == b'>' && j >= 2 && b[j - 2] == b'-' {
                    // (`->` の `>` は括弧ではない)
                } else if c == b'>' {
                    d += 1;
                } else if c == b'<' {
                    if d == 0 {
                        break;
                    }
                    d -= 1;
                } else if d == 0 && (c == b',' || c == b'(' || c == b' ' || c == b'&') {
                    break;
                }
                j -= 1;
            }
            (j, s[j..i].to_string())
        };
        let rep = if into {
            if is_iter_head(&type_head(&inner)) { Some(inner.clone()) } else { iter_item(&inner).map(|e| format!("IntoIter<{e}>")) }
        } else {
            iter_item(&inner)
        };
        match rep {
            Some(item) => s = format!("{}{}{}", &s[..a], item, &s[i + plen..]),
            None => break,
        }
    }
    s
}

/// 型の形 pat (型引数の名前 vars を含む) を実際の型 act に当てはめて、vars の値を out に (`Option<U>` と `Option<char>` → U = char)
fn unify(pat: &str, act: &str, vars: &[String], out: &mut Vec<(String, String)>) {
    let p = pat.trim();
    let a = act.trim();
    if vars.iter().any(|v| v == p) {
        if !out.iter().any(|(v, _)| v == p) {
            out.push((p.to_string(), a.to_string()));
        }
        return;
    }
    if let (Some(p2), Some(a2)) = (p.strip_prefix('&'), a.strip_prefix('&')) {
        return unify(p2, a2, vars, out);
    }
    if type_head(p) == type_head(a) {
        for (x, y) in targs(p).iter().zip(targs(a).iter()) {
            unify(x, y, vars, out);
        }
    }
}

/// クロージャ (トークン s は `|` か `move`) の引数の型の並び (`|x, (a, b)|` → `&D, (u8, char)`)
fn closure_args(ws: &[&FileIndex], file: usize, s: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    let s = if f.tok(s) == "move" { s + 1 } else { s };
    let mut e = s + 1;
    while e < f.toks.len() && !f.sym(e, b'|') {
        e += 1;
    }
    // (引数ごとに: 名前ならその型、タプルのパターンなら要素の型を並べたもの)
    fn pat_ty(ws: &[&FileIndex], file: usize, a: usize, b: usize, depth: u32) -> Option<String> {
        let f = ws[file];
        let mut a = a;
        while a < b && (f.sym(a, b'&') || f.tok(a) == "mut") {
            a += 1;
        }
        if f.sym(a, b'(') {
            let c = close_of(f, a);
            let mut parts = vec![];
            let mut st = a + 1;
            let mut d = 0;
            for q in a + 1..=c {
                if f.sym(q, b'(') {
                    d += 1;
                } else if f.sym(q, b')') && q != c {
                    d -= 1;
                }
                if (d == 0 && f.sym(q, b',')) || q == c {
                    if st < q {
                        parts.push(pat_ty(ws, file, st, q, depth)?);
                    }
                    st = q + 1;
                }
            }
            return Some(format!("({})", parts.join(", ")));
        }
        if f.ident(a) && f.tok(a) != "_" {
            if f.sym(a + 1, b':') {
                return Some(f.text[f.toks.get(a + 2)?.1 as usize..f.toks.get(b - 1)?.2 as usize].trim().to_string());
            }
            return closure_param_type(ws, file, a, depth);
        }
        None
    }
    let mut out = vec![];
    let mut st = s + 1;
    let mut d = 0;
    for q in s + 1..=e {
        if f.sym(q, b'(') {
            d += 1;
        } else if f.sym(q, b')') {
            d -= 1;
        }
        if (d == 0 && f.sym(q, b',')) || q == e {
            if st < q {
                out.push(pat_ty(ws, file, st, q, depth).unwrap_or_else(|| "_".to_string()));
            }
            st = q + 1;
        }
    }
    Some(out.join(", "))
}

/// クロージャ (トークン s は `|` か `move`) の戻り値の型 (`-> T` か、本体の式の型)
fn closure_ret(ws: &[&FileIndex], file: usize, s: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    let mut j = if f.tok(s) == "move" { s + 1 } else { s };
    if !f.sym(j, b'|') {
        return None;
    }
    j += 1;
    while j < f.toks.len() && !f.sym(j, b'|') {
        j += 1;
    }
    let b = j + 1;
    if f.sym(b, b'-') && f.sym(b + 1, b'>') {
        let mut e = b + 2;
        while e < f.toks.len() && !f.sym(e, b'{') {
            e += 1;
        }
        return Some(f.text[f.toks.get(b + 2)?.1 as usize..f.toks.get(e)?.1 as usize].trim().to_string());
    }
    if f.sym(b, b'{') {
        let c = close_of(f, b);
        if c <= b + 1 || f.sym(c - 1, b';') {
            return Some("()".to_string());
        }
        return expr_type(ws, file, c - 1, depth + 1);
    }
    let e = expr_end(f, b, false)?;
    let t = expr_type(ws, file, e, depth + 1)?;
    // (`&x.name` / `&mut v`: 前の参照)
    Some(if f.sym(b, b'&') && !f.sym(b + 1, b'&') {
        if f.tok(b + 1) == "mut" { format!("&mut {t}") } else { format!("&{t}") }
    } else {
        t
    })
}

/// `recv.name(args)` (m は name のトークン、受け手の型 o) の型: メソッドの戻り値の型の型引数を、引数のクロージャの戻り値
/// から決める (`map(|x| x.n)` → Map<Iter<..>, impl FnMut() -> u8>、`opt.map(|x| ..)` → Option<R>)。関連型の射影も置き換える
fn method_call_type(ws: &[&FileIndex], file: usize, o: &str, name: &str, m: usize, depth: u32) -> Option<String> {
    method_call_type_at(ws, file, o, name, m + 1, None, depth)
}

/// method_call_type の本体: op は引数の `(` のトークン、turbo はターボフィッシュ `::<T>` の型
fn method_call_type_at(ws: &[&FileIndex], file: usize, o: &str, name: &str, op: usize, turbo: Option<&str>, depth: u32) -> Option<String> {
    let f = ws[file];
    let (ret, d) = expand(ws, file, o).iter().find_map(|t| member_def(ws, file, t, name, &[SymKind::Method]).map(|(r, d)| (r, d.fnret.clone())))?;
    let mut ret = ret;
    // (ターボフィッシュ `collect::<Vec<_>>()` / `sum::<u32>()` / `parse::<u8>()`: 戻り値の中の最初の型引数 (大文字 1 文字) に。
    // `_` は受け手の要素の型)
    if let Some(tb) = turbo {
        let tb = match iter_item(o) {
            Some(item) if tb.contains('_') => tb.replace('_', &item),
            _ => tb.to_string(),
        };
        if let Some(g) = ret.split(|c: char| !(c.is_alphanumeric() || c == '_')).find(|w| w.len() == 1 && w.starts_with(|c: char| c.is_ascii_uppercase())) {
            let g = g.to_string();
            ret = subst(&ret, &[g], &[tb]);
        }
    }
    let m = op - 1;
    if !d.is_empty() && f.sym(op, b'(') {
        // (引数の始めのトークン: 深さ 0 のカンマで区切る)
        let close = close_of(f, op);
        let mut starts = vec![op + 1];
        let mut dd = 0;
        for q in op + 1..close {
            if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
                dd += 1;
            } else if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
                dd -= 1;
            } else if dd == 0 && f.sym(q, b',') {
                starts.push(q + 1);
            }
        }
        for (idx, fty, pat, _) in &d {
            let Some(&s) = starts.get(*idx as usize) else { continue };
            // (型引数そのものの引数 `zip(other)`: 引数の式の型)
            if pat == "=" {
                if let Some(at) = expr_end(f, s, false).filter(|&e| e < close).and_then(|e| expr_type(ws, file, e, depth + 1)) {
                    ret = subst(&ret, &[fty.clone()], &[at]);
                }
                continue;
            }
            if !(f.sym(s, b'|') || f.tok(s) == "move") {
                continue;
            }
            let Some(r) = closure_ret(ws, file, s, depth + 1) else { continue };
            // (pat の中の型引数: 1 文字か大文字で始まる短い名前)
            let vars: Vec<String> = pat.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty() && w.starts_with(|c: char| c.is_ascii_uppercase()) && w.len() <= 2).map(|w| w.to_string()).collect();
            let mut b = vec![];
            unify(pat, &r, &vars, &mut b);
            let (ps, vs): (Vec<String>, Vec<String>) = b.into_iter().unzip();
            ret = subst(&ret, &ps, &vs);
            if !fty.is_empty() {
                let args = closure_args(ws, file, s, depth + 1).unwrap_or_default();
                ret = subst(&ret, &[fty.clone()], &[format!("impl FnMut({args}) -> {r}")]);
            }
        }
    }
    // (`get` / `get_mut` の `I::Output`: 並びの要素)
    if ret.contains("I::Output") {
        if let Some(e) = index_type(o, false) {
            ret = ret.replace("I::Output", &e);
        }
    }
    let _ = m;
    Some(strip_lifetimes(&norm_proj(&ret)))
}

/// 参照の寿命の名前を外す (`&'a str` → `&str`。`'static` は残す: rust-analyzer の表示と同じ)
fn strip_lifetimes(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut it = t.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c == '&' && t[i + 1..].starts_with('\'') && !t[i + 1..].starts_with("'static") {
            out.push('&');
            // (`'a ` を飛ばす)
            while let Some(&(_, d)) = it.peek() {
                it.next();
                if d == ' ' {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// クロージャの引数 b (`recv.m(|a, b| ..)` の a / b) の型: 繰り返し (`Iter<T>` / `Vec<T>` / `[T]` / `Option<T>` ..) の要素の型。
/// `filter` / `find` / `position` ... の引数は要素への参照、`fold` は 2 つめが要素、`map_err` / `unwrap_or_else` は Result の E
fn closure_param_type(ws: &[&FileIndex], file: usize, b: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    if depth > 24 {
        return None;
    }
    // (b の前の `|`: 引数の並びの始め。その間のカンマの数が引数の番号)
    // (タプルのパターン `|(i, x)|` の中なら、そのタプルの中の番号も: path = [引数の番号, タプルの中の番号..])
    let mut j = b;
    let mut idx = 0;
    let mut path: Vec<usize> = vec![];
    let mut d = 0i32;
    loop {
        j = j.checked_sub(1)?;
        if f.sym(j, b'|') {
            break;
        }
        if f.sym(j, b')') {
            d += 1;
        } else if f.sym(j, b'(') {
            if d == 0 {
                path.push(idx);
                idx = 0;
            } else {
                d -= 1;
            }
        } else if f.sym(j, b',') && d == 0 {
            idx += 1;
        } else if !(f.ident(j) || f.sym(j, b'&') || f.sym(j, b',')) {
            return None;
        }
    }
    path.push(idx);
    path.reverse();
    let idx = path[0];
    if !f.closure_start(j) || (j >= 1 && f.sym(j - 1, b'|')) {
        return None;
    }
    // (`recv.m(` の m と、受け手の式の終わり。クロージャは後ろの引数でもよい: `fold(0, |acc, x| ..)`)
    let mut open = if j >= 1 && f.ident(j - 1) && f.tok(j - 1) == "move" { j.checked_sub(2)? } else { j.checked_sub(1)? };
    let mut d = 0;
    while !(d == 0 && f.sym(open, b'(')) {
        if f.sym(open, b')') || f.sym(open, b']') || f.sym(open, b'}') {
            d += 1;
        } else if f.sym(open, b'(') || f.sym(open, b'[') || f.sym(open, b'{') {
            if d == 0 {
                return None;
            }
            d -= 1;
        } else if d == 0 && f.sym(open, b';') {
            return None;
        }
        open = open.checked_sub(1)?;
    }
    if open < 3 || !f.ident(open - 1) || !f.sym(open - 2, b'.') {
        return None;
    }
    let m = f.tok(open - 1);
    let recv = expr_type(ws, file, open - 3, depth + 1)?;
    // (メソッドのシグネチャの束縛 `F: FnOnce(&T) -> R` の引数の形から: `cell.with(|m| ..)` / `opt.map(|x| ..)` / `iter.fold(0, |a, x| ..)`)
    if let Some(t) = closure_param_from_sig(ws, file, &recv, m, open, j, idx, &path, depth) {
        return Some(t);
    }
    let recv = expand(ws, file, &recv).into_iter().last().unwrap_or(recv);
    let h = type_head(&recv);
    let a = targs(&recv);
    let elem = match h.as_str() {
        "Result" => match m {
            "map_err" | "unwrap_or_else" | "or_else" => a.get(1).cloned(),
            _ => a.first().cloned(),
        },
        "HashMap" | "BTreeMap" | "FxHashMap" => Some(format!("(&{}, &{})", a.first()?, a.get(1)?)),
        _ if is_iter_head(&h) => iter_item(&recv),
        _ => elem_type(&recv),
    }?;
    let by_ref = matches!(m, "filter" | "find" | "position" | "skip_while" | "take_while" | "inspect" | "retain" | "sort_by_key" | "max_by_key" | "min_by_key" | "is_some_and" | "is_ok_and" | "any" | "all" | "dedup_by_key" | "binary_search_by_key" | "partition")
        && !matches!(m, "any" | "all" | "position" | "partition" | "is_some_and" | "is_ok_and");
    let ty = match (m, idx) {
        ("fold" | "try_fold" | "scan", 0) => return None,
        ("fold" | "try_fold", 1) => elem,
        ("sort_by" | "max_by" | "min_by" | "dedup_by", _) => format!("&{elem}"),
        (_, 0) => if by_ref { format!("&{elem}") } else { elem },
        _ => return None,
    };
    // (タプルのパターンの中: その番号の要素。`&(a, b)` の参照は外す)
    let mut ty = ty;
    for &i in &path[1..] {
        let inner = strip_ref(&ty).to_string();
        ty = tuple_items(&inner)?.get(i)?.clone();
    }
    Some(ty)
}

/// トークン k で終わるリテラルの型 (`"a"` → &str、`'c'` → char、`b"x"` → &[u8]、`b'x'` → u8、`1u8` → u8、`true` → bool)。
/// 式全体がリテラルのときだけ (`x + 1` の 1 は式の型ではない)。接尾辞のない数 (`0` / `1.5`) は使い方で型が決まる
/// (rust-analyzer は `let mut acc = 0; acc += x_i64` を i64 にする) ので答えない。
/// (字句では数は 1 文字ずつの記号、接尾辞は続く識別子)
fn literal_type(f: &FileIndex, k: usize) -> Option<String> {
    let b = f.text.as_bytes();
    let digit = |j: usize| f.toks[j].0 == 1 && b[f.toks[j].1 as usize].is_ascii_digit();
    let adj = |j: usize| j >= 1 && f.toks[j - 1].2 == f.toks[j].1;
    // (s から始まる式の前が、式の始まりになれる所か)
    let whole = |s: usize| -> bool {
        if s == 0 {
            return true;
        }
        let p = s - 1;
        if f.ident(p) {
            return matches!(f.tok(p), "return" | "break" | "in");
        }
        if f.sym(p, b'=') {
            return !(p >= 1 && f.toks[p - 1].2 == f.toks[p].1 && f.toks[p - 1].0 == 1 && b"=!<>+-*/%&|^".contains(&b[f.toks[p - 1].1 as usize]));
        }
        if f.sym(p, b'>') {
            return p >= 1 && f.sym(p - 1, b'=') && f.toks[p - 1].2 == f.toks[p].1;
        }
        f.toks[p].0 == 1 && matches!(b[f.toks[p].1 as usize], b'(' | b',' | b'{' | b'[' | b';' | b'|')
    };
    if f.toks[k].0 == 2 {
        let s = f.tok(k);
        if !whole(k) {
            return None;
        }
        return Some(
            if s.starts_with("b'") {
                "u8"
            } else if s.starts_with('b') {
                "&[u8]"
            } else if s.starts_with('\'') {
                "char"
            } else {
                "&str"
            }
            .to_string(),
        );
    }
    if !f.ident(k) {
        return None;
    }
    let w = f.tok(k);
    if w == "true" || w == "false" {
        return whole(k).then(|| "bool".to_string());
    }
    // (接尾辞 `1u8` / `2.0f32` / `1_000usize`: 前へ続く数字・`.`・`_` の始めが式の始め)
    // (`1_000usize` は `1` と `_000usize`)
    let suf = w.trim_start_matches(|c: char| c == '_' || c.is_ascii_digit());
    if PRIMS.contains(&suf) && adj(k) && digit(k - 1) {
        let mut j = k - 1;
        while j >= 1 && adj(j) && (digit(j - 1) || f.sym(j - 1, b'.')) {
            j -= 1;
        }
        return whole(j).then(|| suf.to_string());
    }
    None
}

/// off を含む文字列・文字のリテラルの hover (`&'static str` と値)
fn literal_hover(f: &FileIndex, off: usize) -> Option<String> {
    let i = f.toks.partition_point(|t| (t.2 as usize) <= off);
    let t = *f.toks.get(i)?;
    if t.0 != 2 || (t.1 as usize) > off {
        return None;
    }
    let s = &f.text[t.1 as usize..t.2 as usize];
    let (byte, s) = match s.strip_prefix('b') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let (ty, body) = if let Some(r) = s.strip_prefix('r') {
        // (生の文字列 `r#"..."#`: 中はそのまま)
        let h = r.len() - r.trim_start_matches('#').len();
        let inner = r.get(h + 1..r.len().checked_sub(h + 1)?)?;
        (if byte { format!("&'static [u8; {}]", inner.len()) } else { "&'static str".to_string() }, inner.to_string())
    } else if let Some(r) = s.strip_prefix('"') {
        let v = unescape(r.strip_suffix('"').unwrap_or(r));
        (if byte { format!("&'static [u8; {}]", v.len()) } else { "&'static str".to_string() }, v)
    } else if let Some(r) = s.strip_prefix('\'') {
        (if byte { "u8" } else { "char" }.to_string(), unescape(r.strip_suffix('\'').unwrap_or(r)))
    } else {
        return None;
    };
    Some(format!("```rust\n{ty}\n```\n---\n\nvalue of literal: ` {body} `"))
}

/// リテラルの中のエスケープ (`\n` / `\t` / `\\` / `\"` / `\'` / `\0` / `\u{..}` / `\x..` / 行の継続) を戻す
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('x') => {
                let h: String = it.by_ref().take(2).collect();
                if let Some(ch) = u8::from_str_radix(&h, 16).ok().map(char::from) {
                    out.push(ch);
                }
            }
            Some('u') => {
                let h: String = it.by_ref().skip_while(|&c| c == '{').take_while(|&c| c != '}').collect();
                if let Some(ch) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            // (行の継続 `\` + 改行: 次の行の頭の空白も飛ばす)
            Some('\n') => {
                while it.peek().is_some_and(|c| c.is_whitespace()) {
                    it.next();
                }
            }
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

/// ローカルの束縛 t がクロージャ (`let f = |a: T, b| -> R { .. }` / `move |..|`) なら、そのシグネチャ `impl Fn(a: T, b) -> R`
/// (戻り値の型を書いていなければ、本体の式の型)
fn closure_sig(ws: &[&FileIndex], t: Loc) -> Option<String> {
    let f = ws[t.file];
    if f.slim {
        return None;
    }
    let b = f.toks.partition_point(|x| (x.1 as usize) < t.start);
    if !(f.sym(b + 1, b'=') && !f.sym(b + 2, b'=')) {
        return None;
    }
    let mut c = b + 2;
    if f.tok(c) == "move" {
        c += 1;
    }
    if !f.sym(c, b'|') {
        return None;
    }
    let mut e = c + 1;
    while e < f.toks.len() && !f.sym(e, b'|') {
        e += 1;
    }
    if e >= f.toks.len() {
        return None;
    }
    let params = if e > c + 1 { f.text[f.toks[c].2 as usize..f.toks[e].1 as usize].split_whitespace().collect::<Vec<_>>().join(" ") } else { String::new() };
    let ret = if f.sym(e + 1, b'-') && f.sym(e + 2, b'>') {
        let mut j = e + 3;
        while j < f.toks.len() && !f.sym(j, b'{') {
            j += 1;
        }
        let r = f.text.get(f.toks.get(e + 3)?.1 as usize..f.toks.get(j)?.1 as usize)?.trim();
        format!(" -> {}", r.split_whitespace().collect::<Vec<_>>().join(" "))
    } else {
        expr_end(f, e + 1, false).and_then(|x| expr_type(ws, t.file, x, 1)).map(|r| format!(" -> {r}")).unwrap_or_default()
    };
    Some(format!("impl Fn({params}){ret}"))
}
