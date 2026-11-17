//! ソースのシンボル (アイテムとその中のメンバ) の一覧: LSP の documentSymbol / workspace/symbol が使う。
//! 構文木ではなく字句の列を読むので、編集の途中で構文が壊れていても読める所までは出す。位置はもとのテキストの byte
//! (前処理の前: マクロの展開・`mod` を平らにする前の、エディタが持っているテキストそのもの)。
//!
//! 出すもの: mod / struct / enum / union / trait / impl / fn / const / static / type / macro_rules!、
//! struct のフィールド、enum のバリアント、impl / trait / mod / extern の中のアイテム。関数の本体の中は読まない。

use crate::lexer::tokenize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymKind {
    Module,
    Struct,
    Enum,
    Union,
    Trait,
    Impl,
    Function,
    Method,
    Const,
    Static,
    TypeAlias,
    Field,
    Variant,
    Macro,
}

impl SymKind {
    /// LSP の SymbolKind の番号
    pub fn lsp(self) -> u32 {
        match self {
            SymKind::Module => 2,
            SymKind::Struct | SymKind::Union => 23,
            SymKind::Enum => 10,
            SymKind::Trait => 11,
            SymKind::Impl => 19,
            SymKind::Function | SymKind::Macro => 12,
            SymKind::Method => 6,
            SymKind::Const => 14,
            SymKind::Static => 13,
            SymKind::TypeAlias => 26,
            SymKind::Field => 8,
            SymKind::Variant => 22,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub kind: SymKind,
    /// 補足 (fn のシグネチャ、フィールドの型など。1 行にまとめたもの)
    pub detail: String,
    /// アイテム全体 (属性を含む) と名前の byte 範囲
    pub start: usize,
    pub end: usize,
    pub name_start: usize,
    pub name_end: usize,
    pub children: Vec<Symbol>,
}

type Tok = (u8, usize, usize);

struct Scan<'a> {
    s: &'a [u8],
    t: Vec<Tok>,
}

impl Scan<'_> {
    fn text(&self, k: usize) -> &[u8] {
        let (_, a, b) = self.t[k];
        &self.s[a..b]
    }
    fn is(&self, k: usize, w: &[u8]) -> bool {
        k < self.t.len() && self.text(k) == w
    }
    fn sym(&self, k: usize, c: u8) -> bool {
        k < self.t.len() && self.t[k].0 == 1 && self.s[self.t[k].1] == c
    }
    fn ident(&self, k: usize) -> bool {
        k < self.t.len() && self.t[k].0 == 0
    }
    /// k の開き括弧に対応する閉じ括弧。`{` は波括弧だけを数える (中の丸括弧が閉じていなくても本体の終わりがわかる)。
    /// 閉じていなければ、次に行頭 (字下げなし) から始まるアイテムの前まで (なければ end - 1)
    fn close(&self, k: usize, end: usize) -> usize {
        let brace = self.sym(k, b'{');
        let mut d = 0i32;
        let mut j = k;
        while j < end {
            if self.t[j].0 == 1 {
                match self.s[self.t[j].1] {
                    b'{' => d += 1,
                    b'(' | b'[' if !brace => d += 1,
                    b'}' => {
                        d -= 1;
                        if d == 0 {
                            return j;
                        }
                    }
                    b')' | b']' if !brace => {
                        d -= 1;
                        if d == 0 {
                            return j;
                        }
                    }
                    _ => {}
                }
            }
            j += 1;
        }
        let mut j = k + 1;
        while j < end {
            let a = self.t[j].1;
            let at_col0 = a == 0 || self.s[a - 1] == b'\n';
            let w = self.text(j);
            if at_col0 && matches!(w, b"fn" | b"pub" | b"struct" | b"enum" | b"union" | b"impl" | b"trait" | b"mod" | b"const" | b"static" | b"type" | b"use" | b"#" | b"macro_rules") {
                return j - 1;
            }
            j += 1;
        }
        end.saturating_sub(1)
    }

    /// k から、括弧の外の `;` か (want_brace なら) 最初の括弧の外の `{ .. }` の終わり
    fn item_end(&self, k: usize, end: usize, want_brace: bool) -> usize {
        let mut j = k;
        while j < end {
            if self.t[j].0 == 1 {
                match self.s[self.t[j].1] {
                    b';' => return j,
                    b'{' if want_brace => return self.close(j, end),
                    b'(' | b'[' | b'{' => {
                        j = self.close(j, end);
                    }
                    // (次のアイテムの始まりの `}`: 閉じていない括弧)
                    b'}' => return j.saturating_sub(1).max(k),
                    _ => {}
                }
            }
            j += 1;
        }
        end.saturating_sub(1)
    }
    /// a..=b のトークンの範囲のテキストを、空白 (改行・コメントも) をつめて 1 行にしたもの (長ければ切る)
    fn join(&self, a: usize, b: usize) -> String {
        if a >= self.t.len() || b < a {
            return String::new();
        }
        let b = b.min(self.t.len() - 1);
        let mut out = String::new();
        let mut j = a;
        while j <= b {
            if j > a && self.t[j].1 > self.t[j - 1].2 {
                out.push(' ');
            }
            out.push_str(&String::from_utf8_lossy(&self.s[self.t[j].1..self.t[j].2]));
            // (トークンの間の `'` (ライフタイム) は字句の列にないので、もとのテキストから足す)
            if j < b && self.t[j + 1].1 > self.t[j].2 {
                let gap = &self.s[self.t[j].2..self.t[j + 1].1];
                if gap.ends_with(b"'") {
                    if gap.len() > 1 {
                        out.push(' ');
                    }
                    out.push('\'');
                    j += 1;
                    out.push_str(&String::from_utf8_lossy(&self.s[self.t[j].1..self.t[j].2]));
                }
            }
            if out.len() > 120 {
                out.push('…');
                break;
            }
            j += 1;
        }
        out
    }

    /// a..end のアイテムを読む。in_impl: impl / trait の中 (fn はメソッド)
    fn items(&self, a: usize, end: usize, in_impl: bool, out: &mut Vec<Symbol>) {
        let mut k = a;
        while k < end {
            let start = k;
            // 属性 `#[..]` / `#![..]`
            let mut j = k;
            while self.sym(j, b'#') {
                let mut o = j + 1;
                if self.sym(o, b'!') {
                    o += 1;
                }
                if !self.sym(o, b'[') {
                    break;
                }
                j = self.close(o, end) + 1;
            }
            // 修飾 (pub / pub(crate) / unsafe / async / default / extern "C" (fn の前) / const (fn の前))
            loop {
                if self.is(j, b"pub") {
                    j += 1;
                    if self.sym(j, b'(') {
                        j = self.close(j, end) + 1;
                    }
                } else if self.is(j, b"unsafe") || self.is(j, b"async") || self.is(j, b"default") || (self.is(j, b"const") && self.is(j + 1, b"fn")) {
                    j += 1;
                } else if self.is(j, b"extern") && j + 1 < end && self.t[j + 1].0 == 2 && self.is(j + 2, b"fn") {
                    j += 2;
                } else {
                    break;
                }
            }
            if j >= end {
                break;
            }
            let kw = self.text(j).to_vec();
            let named = |kind: SymKind, nk: usize, e: usize, detail: String, children: Vec<Symbol>| -> Symbol {
                let (na, nb) = if nk < self.t.len() { (self.t[nk].1, self.t[nk].2) } else { (self.t[j].1, self.t[j].2) };
                Symbol {
                    name: String::from_utf8_lossy(&self.s[na..nb]).into_owned(),
                    kind,
                    detail,
                    start: self.t[start].1,
                    end: self.t[e.min(self.t.len() - 1)].2,
                    name_start: na,
                    name_end: nb,
                    children,
                }
            };
            let next = match kw.as_slice() {
                b"fn" if self.ident(j + 1) => {
                    let e = self.item_end(j, end, true);
                    // (シグネチャ: 本体の `{` か `;` の前まで)
                    let mut se = j;
                    while se < e && !self.sym(se + 1, b'{') && !self.sym(se + 1, b';') {
                        se += 1;
                    }
                    out.push(named(if in_impl { SymKind::Method } else { SymKind::Function }, j + 1, e, self.join(j, se), vec![]));
                    e + 1
                }
                b"struct" | b"union" if self.ident(j + 1) => {
                    let e = self.item_end(j, end, true);
                    let mut fields = vec![];
                    // `struct S { a: T, .. }` のフィールド
                    let mut b = j + 2;
                    while b < e && !self.sym(b, b'{') && !self.sym(b, b'(') && !self.sym(b, b';') {
                        if self.sym(b, b'<') {
                            // (型引数の中の `{` は数えない: const 引数の式は無視してよい)
                        }
                        b += 1;
                    }
                    if self.sym(b, b'{') {
                        self.fields(b + 1, self.close(b, end), &mut fields);
                    }
                    let kind = if kw == b"struct" { SymKind::Struct } else { SymKind::Union };
                    out.push(named(kind, j + 1, e, String::new(), fields));
                    e + 1
                }
                b"enum" if self.ident(j + 1) => {
                    let e = self.item_end(j, end, true);
                    let mut vars = vec![];
                    let mut b = j + 2;
                    while b < e && !self.sym(b, b'{') {
                        b += 1;
                    }
                    if self.sym(b, b'{') {
                        self.variants(b + 1, self.close(b, end), &mut vars);
                    }
                    out.push(named(SymKind::Enum, j + 1, e, String::new(), vars));
                    e + 1
                }
                b"trait" if self.ident(j + 1) => {
                    let e = self.item_end(j, end, true);
                    let mut ch = vec![];
                    let mut b = j + 2;
                    while b < e && !self.sym(b, b'{') {
                        b += 1;
                    }
                    if self.sym(b, b'{') {
                        self.items(b + 1, self.close(b, end), true, &mut ch);
                    }
                    out.push(named(SymKind::Trait, j + 1, e, String::new(), ch));
                    e + 1
                }
                b"impl" => {
                    let e = self.item_end(j, end, true);
                    let mut b = j + 1;
                    while b < e && !self.sym(b, b'{') {
                        b += 1;
                    }
                    let mut ch = vec![];
                    if self.sym(b, b'{') {
                        self.items(b + 1, self.close(b, end), true, &mut ch);
                    }
                    // (名前: `impl<T> Tr for S<T>` の型引数を除いた `Tr for S<T>` / `S`)
                    let mut h = j + 1;
                    if self.sym(h, b'<') {
                        let mut d = 0i32;
                        while h < b {
                            if self.sym(h, b'<') {
                                d += 1;
                            } else if self.sym(h, b'>') {
                                d -= 1;
                                if d == 0 {
                                    h += 1;
                                    break;
                                }
                            }
                            h += 1;
                        }
                    }
                    let mut he = b.saturating_sub(1);
                    // (`where` から後ろは名前に入れない)
                    let mut w = h;
                    while w < b {
                        if self.is(w, b"where") {
                            he = w.saturating_sub(1);
                            break;
                        }
                        w += 1;
                    }
                    let name = if h <= he { self.join(h, he) } else { String::from("impl") };
                    let (na, nb) = if h <= he && he < self.t.len() { (self.t[h].1, self.t[he].2) } else { (self.t[j].1, self.t[j].2) };
                    out.push(Symbol {
                        name: format!("impl {name}"),
                        kind: SymKind::Impl,
                        detail: String::new(),
                        start: self.t[start].1,
                        end: self.t[e.min(self.t.len() - 1)].2,
                        name_start: na,
                        name_end: nb,
                        children: ch,
                    });
                    e + 1
                }
                b"mod" if self.ident(j + 1) => {
                    let e = self.item_end(j, end, true);
                    let mut ch = vec![];
                    if self.sym(j + 2, b'{') {
                        self.items(j + 3, self.close(j + 2, end), false, &mut ch);
                    }
                    out.push(named(SymKind::Module, j + 1, e, String::new(), ch));
                    e + 1
                }
                b"extern" if self.sym(j + 1, b'{') || (j + 2 < end && self.t[j + 1].0 == 2 && self.sym(j + 2, b'{')) => {
                    // `extern "C" { fn f(); }` の中の宣言はトップレベルに並べる
                    let o = if self.sym(j + 1, b'{') { j + 1 } else { j + 2 };
                    let c = self.close(o, end);
                    self.items(o + 1, c, false, out);
                    c + 1
                }
                b"const" | b"static" if self.ident(j + 1) => {
                    let nk = if self.is(j + 1, b"mut") { j + 2 } else { j + 1 };
                    let e = self.item_end(j, end, false);
                    if self.is(nk, b"_") {
                        e + 1
                    } else {
                        // (型: `:` から `=` / `;` の前まで)
                        let mut ta = nk + 1;
                        let detail = if self.sym(ta, b':') {
                            ta += 1;
                            let mut tb = ta;
                            while tb < e && !self.sym(tb, b'=') {
                                tb += 1;
                            }
                            if tb > ta { self.join(ta, tb - 1) } else { String::new() }
                        } else {
                            String::new()
                        };
                        out.push(named(if kw == b"const" { SymKind::Const } else { SymKind::Static }, nk, e, detail, vec![]));
                        e + 1
                    }
                }
                b"type" if self.ident(j + 1) => {
                    let e = self.item_end(j, end, false);
                    out.push(named(SymKind::TypeAlias, j + 1, e, String::new(), vec![]));
                    e + 1
                }
                b"macro_rules" if self.sym(j + 1, b'!') && self.ident(j + 2) => {
                    let mut e = if self.sym(j + 3, b'{') || self.sym(j + 3, b'(') || self.sym(j + 3, b'[') { self.close(j + 3, end) } else { j + 2 };
                    if self.sym(e + 1, b';') {
                        e += 1;
                    }
                    out.push(named(SymKind::Macro, j + 2, e, String::from("macro_rules!"), vec![]));
                    e + 1
                }
                // (宣言マクロ 2.0 `macro name(..) { .. }` / `macro name { .. }`: std の derive など)
                b"macro" if self.ident(j + 1) => {
                    let mut b = j + 2;
                    if self.sym(b, b'(') {
                        b = self.close(b, end) + 1;
                    }
                    let e = if self.sym(b, b'{') { self.close(b, end) } else { self.item_end(j, end, true) };
                    out.push(named(SymKind::Macro, j + 1, e, String::from("macro"), vec![]));
                    e + 1
                }
                b"use" | b"extern" => self.item_end(j, end, false) + 1,
                _ => {
                    // (マクロの呼び出し `m! { .. }` / `m!(..);` などは読み飛ばす。わからないトークンは 1 つ進める)
                    if self.ident(j) && self.sym(j + 1, b'!') && j + 2 < end && self.t[j + 2].0 == 1 {
                        let c = self.close(j + 2, end);
                        if self.sym(c + 1, b';') { c + 2 } else { c + 1 }
                    } else if self.sym(j, b'{') {
                        self.close(j, end) + 1
                    } else {
                        j + 1
                    }
                }
            };
            k = next.max(k + 1);
        }
    }

    /// `{ a: T, pub b: U }` のフィールド
    fn fields(&self, a: usize, end: usize, out: &mut Vec<Symbol>) {
        let mut k = a;
        while k < end {
            let start = k;
            let mut j = k;
            while self.sym(j, b'#') && self.sym(j + 1, b'[') {
                j = self.close(j + 1, end) + 1;
            }
            if self.is(j, b"pub") {
                j += 1;
                if self.sym(j, b'(') {
                    j = self.close(j, end) + 1;
                }
            }
            // (次の括弧の外の `,` まで)
            let mut e = j;
            while e < end && !self.sym(e, b',') {
                if self.sym(e, b'(') || self.sym(e, b'[') || self.sym(e, b'{') {
                    e = self.close(e, end);
                } else if self.sym(e, b'<') {
                    let mut d = 0i32;
                    while e < end {
                        if self.sym(e, b'<') {
                            d += 1;
                        } else if self.sym(e, b'>') {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        e += 1;
                    }
                }
                e += 1;
            }
            if self.ident(j) && self.sym(j + 1, b':') && j + 2 < e {
                let (na, nb) = (self.t[j].1, self.t[j].2);
                out.push(Symbol {
                    name: String::from_utf8_lossy(&self.s[na..nb]).into_owned(),
                    kind: SymKind::Field,
                    detail: self.join(j + 2, e - 1),
                    start: self.t[start].1,
                    end: self.t[(e - 1).min(self.t.len() - 1)].2,
                    name_start: na,
                    name_end: nb,
                    children: vec![],
                });
            }
            k = e + 1;
        }
    }

    /// `{ A, B(T), C { x: U } = 3 }` のバリアント
    fn variants(&self, a: usize, end: usize, out: &mut Vec<Symbol>) {
        let mut k = a;
        while k < end {
            let start = k;
            let mut j = k;
            while self.sym(j, b'#') && self.sym(j + 1, b'[') {
                j = self.close(j + 1, end) + 1;
            }
            let mut e = j;
            while e < end && !self.sym(e, b',') {
                if self.sym(e, b'(') || self.sym(e, b'[') || self.sym(e, b'{') {
                    e = self.close(e, end);
                }
                e += 1;
            }
            if self.ident(j) {
                let (na, nb) = (self.t[j].1, self.t[j].2);
                let mut ch = vec![];
                if self.sym(j + 1, b'{') {
                    self.fields(j + 2, self.close(j + 1, end), &mut ch);
                }
                out.push(Symbol {
                    name: String::from_utf8_lossy(&self.s[na..nb]).into_owned(),
                    kind: SymKind::Variant,
                    detail: String::new(),
                    start: self.t[start].1,
                    end: self.t[(e.max(j + 1) - 1).min(self.t.len() - 1)].2,
                    name_start: na,
                    name_end: nb,
                    children: ch,
                });
            }
            k = e + 1;
        }
    }
}

/// ソースのシンボルの木
pub fn document_symbols(src: &str) -> Vec<Symbol> {
    let s = src.as_bytes();
    let t = tokenize(s);
    let sc = Scan { s, t };
    let mut out = vec![];
    let n = sc.t.len();
    sc.items(0, n, false, &mut out);
    out
}

/// シンボルを平らにした列: (シンボル, 囲むものの名前)
pub fn flatten(syms: &[Symbol]) -> Vec<(Symbol, String)> {
    fn go(syms: &[Symbol], parent: &str, out: &mut Vec<(Symbol, String)>) {
        for s in syms {
            let mut x = s.clone();
            x.children = vec![];
            out.push((x, parent.to_string()));
            go(&s.children, &s.name, out);
        }
    }
    let mut out = vec![];
    go(syms, "", &mut out);
    out
}

/// byte 位置 → (行, 桁)。桁は UTF-16 の単位 (LSP と同じ)
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(src: &str) -> LineIndex {
        let mut starts = vec![0];
        for (i, &c) in src.as_bytes().iter().enumerate() {
            if c == b'\n' {
                starts.push(i + 1);
            }
        }
        LineIndex { starts }
    }
    pub fn pos(&self, src: &str, at: usize) -> (u32, u32) {
        let at = at.min(src.len());
        let line = match self.starts.binary_search(&at) {
            Ok(l) => l,
            Err(l) => l - 1,
        };
        let ls = self.starts[line];
        let mut col = 0u32;
        let mut i = ls;
        while i < at {
            if !src.is_char_boundary(i) {
                i += 1;
                continue;
            }
            let c = src[i..].chars().next().unwrap_or(' ');
            col += c.len_utf16() as u32;
            i += c.len_utf8();
        }
        (line as u32, col)
    }
}

/// 名前が問い合わせに合うかと、その良さ (小さいほど良い)。大文字小文字は区別しない。
/// 0: 同じ、1: 前方一致、2: 部分一致 (語の頭)、3: 部分一致、4: 文字の順に含む (`fbar` → `foo_bar`)。空の問い合わせは全部 5
pub fn match_score(query: &str, name: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(5);
    }
    let q = query.to_lowercase();
    let n = name.to_lowercase();
    if n == q {
        return Some(0);
    }
    if n.starts_with(&q) {
        return Some(1);
    }
    if let Some(i) = n.find(&q) {
        let b = name.as_bytes();
        let word_head = i > 0 && (b[i - 1] == b'_' || b[i - 1] == b':' || b[i - 1] == b' ' || (b[i].is_ascii_uppercase() && b[i - 1].is_ascii_lowercase()));
        return Some(if word_head { 2 } else { 3 });
    }
    let mut it = n.chars();
    for c in q.chars() {
        if !it.any(|x| x == c) {
            return None;
        }
    }
    Some(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[Symbol]) -> Vec<String> {
        v.iter().map(|s| format!("{:?} {}", s.kind, s.name)).collect()
    }

    #[test]
    fn items_and_members() {
        let src = "#[derive(Debug)]\npub struct P { pub x: i64, y: Vec<u8> }\nenum E { A, B(u8), C { z: u32 } }\n\
                   impl<T: Clone> Tr for W<T> where T: Copy {\n    fn m(&self) -> u32 { 1 }\n    const K: u8 = 1;\n}\n\
                   mod m { pub fn f() {} macro_rules! mm { () => {} } }\nconst C: &str = \"{\";\nfn g() { fn inner() {} }\n";
        let s = document_symbols(src);
        assert_eq!(names(&s), ["Struct P", "Enum E", "Impl impl Tr for W<T>", "Module m", "Const C", "Function g"]);
        assert_eq!(document_symbols("impl<'a> Cx<'a> { fn f(&'a self) {} }")[0].name, "impl Cx<'a>");
        assert_eq!(names(&s[0].children), ["Field x", "Field y"]);
        assert_eq!(s[0].children[1].detail, "Vec<u8>");
        assert_eq!(names(&s[1].children), ["Variant A", "Variant B", "Variant C"]);
        assert_eq!(names(&s[1].children[2].children), ["Field z"]);
        assert_eq!(names(&s[2].children), ["Method m", "Const K"]);
        assert_eq!(s[2].children[0].detail, "fn m(&self) -> u32");
        assert_eq!(names(&s[3].children), ["Function f", "Macro mm"]);
        // (範囲は属性から、名前は名前だけ)
        assert_eq!(&src[s[0].start..s[0].start + 2], "#[");
        assert_eq!(&src[s[0].name_start..s[0].name_end], "P");
        assert!(src[s[5].start..s[5].end].ends_with('}'));
    }

    #[test]
    fn broken_source() {
        // (閉じていない fn の後ろも読めるところまで)
        let s = document_symbols("fn a() {\n    let x = (1;\n}\nstruct S;\nfn b(");
        assert_eq!(names(&s), ["Function a", "Struct S", "Function b"]);
    }

    #[test]
    fn scores() {
        assert_eq!(match_score("foo", "foo"), Some(0));
        assert_eq!(match_score("foo", "FooBar"), Some(1));
        assert_eq!(match_score("bar", "foo_bar"), Some(2));
        assert_eq!(match_score("oob", "foobar"), Some(3));
        assert_eq!(match_score("fbr", "foo_bar"), Some(4));
        assert_eq!(match_score("xyz", "foo"), None);
    }

    #[test]
    fn utf16_columns() {
        let src = "// あ\nfn ü() {}";
        let li = LineIndex::new(src);
        assert_eq!(li.pos(src, src.find('ü').unwrap()), (1, 3));
        assert_eq!(li.pos(src, src.find("()").unwrap()), (1, 4));
    }
}
