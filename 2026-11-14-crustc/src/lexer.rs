//! 超軽量 lexer: トップレベルのアイテム分割と、アイテムごとの正規化ハッシュだけを行う。
//!
//! syn でファイル全体を parse すると 4MB/s 程度しか出ないので、no-op / 1 関数編集の
//! ビルドでは syn を使わず、この lexer だけで「どのアイテムが変わったか」を判定する。
//! 空白とコメントはハッシュに入らないので、整形やコメント編集では何も再コンパイルされない。

use crate::cache::{Key, KeyHasher};

pub struct SrcItem {
    pub start: usize,
    pub end: usize,
    /// 1 始まりの開始行 (エラー位置の補正用)
    pub line: usize,
    /// 正規化トークン列のハッシュ。impl のメンバはヘッダのハッシュも含む
    pub hash: Key,
    /// アイテム内の識別子トークン (byte range)。参照関係の構文的な過大近似に使う
    pub idents: Vec<(u32, u32)>,
    pub kind: ItemKind,
}

/// `impl X { fn a() {} fn b() {} }` は 1 アイテムにせず、ヘッダ `impl X {` と
/// メンバ (a, b) に分ける。メソッドを 1 つ編集しても、そのメソッドだけが変わったことになる。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemKind {
    Normal,
    /// `impl X {` まで
    ImplHeader,
    /// impl の中のアイテム。ヘッダの index を持つ
    ImplMember(u32),
    /// `trait X {` まで
    TraitHeader,
    /// trait の中のアイテム (デフォルト実装のない宣言も含む)
    TraitMember(u32),
    /// `#![no_std]` などのクレート属性
    InnerAttr,
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}
fn is_ident_cont(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

type Cur = Option<(usize, usize, Vec<(u32, u32)>)>;

struct St<'a> {
    s: &'a [u8],
    cur: Cur,
    prev_ws: bool,
    prev_punct: bool,
    /// 正規化したトークン列。アイテム終端で 1 回だけハッシュする
    buf: Vec<u8>,
    /// 行番号はアイテム開始時だけ必要なので差分で数える
    line_cache: (usize, usize),
    /// 今のアイテムで、トップレベルの最初の識別子をもう見たか (`impl` 判定用)
    seen_ident: bool,
    /// アイテムの先頭の (pub などを除いた) 識別子 (最大 2 個、byte range)
    head: [(u32, u32); 2],
    n_head: usize,
}

impl St<'_> {
    fn line_at(&mut self, pos: usize) -> usize {
        let (p, l) = self.line_cache;
        let l = l + self.s[p..pos].iter().filter(|&&c| c == b'\n').count();
        self.line_cache = (pos, l);
        l
    }
    fn tok(&mut self, st: usize, en: usize, kind: u8) {
        if self.cur.is_none() {
            let ln = self.line_at(st);
            self.cur = Some((st, ln, vec![]));
        }
        let item = self.cur.as_mut().unwrap();

        // 記号同士が空白なしで連続しているか (`&&` と `& &` の区別) だけを入れる。
        // それ以外の空白はハッシュに影響しない
        // (`b"..".contains` は memchr になり、文字列の配置で速度が変わるので matches! にする)
        let op = kind == 5
            && matches!(
                self.s[st],
                b'&' | b'|'
                    | b'='
                    | b'<'
                    | b'>'
                    | b'!'
                    | b'-'
                    | b'+'
                    | b'*'
                    | b'/'
                    | b'%'
                    | b'^'
                    | b'.'
                    | b':'
                    | b'#'
                    | b'@'
                    | b'~'
                    | b'?'
                    | b'$'
            );
        let joint = op && self.prev_punct && !self.prev_ws;
        // (種類, 長さ, 本体) の列にする。長さを入れるので曖昧さがない
        self.buf.push(kind << 1 | joint as u8);
        self.buf
            .extend_from_slice(&((en - st) as u32).to_le_bytes());
        self.buf.extend_from_slice(&self.s[st..en]);
        self.prev_punct = op;
        if kind == 2 {
            item.2.push((st as u32, en as u32));
        }
        self.prev_ws = false;
    }
}

pub fn split(src: &str) -> Result<Vec<SrcItem>, String> {
    let s = src.as_bytes();
    let n = s.len();
    let mut i = 0;
    let mut items: Vec<SrcItem> = vec![];
    let mut depth: i32 = 0;
    let mut st = St {
        s,
        cur: None,
        prev_ws: true,
        prev_punct: false,
        buf: Vec::with_capacity(4096),
        line_cache: (0, 1),
        seen_ident: false,
        head: [(0, 0); 2],
        n_head: 0,
    };
    // `use` / `const` / `static` / `type` は `}` ではなく `;` で終わる (`use a::{b, c};` / `const P: T = T { .. };`)
    let mut semi_only = false;
    // impl ブロックの中にいるときは、メンバの区切りを depth 1 で判定する
    let mut base: i32 = 0;
    let mut impl_header: Option<u32> = None;
    let mut impl_pending = false;
    // ブロックが trait か impl か
    let mut is_trait = false;
    // `#![...]` (クレート属性) の途中か
    let mut inner_attr = false;
    macro_rules! tok {
        ($a:expr, $b:expr, $k:expr) => {
            st.tok($a, $b, $k)
        };
    }

    while i < n {
        let c = s[i];
        // 空白
        if c.is_ascii_whitespace() {
            i += 1;
            st.prev_ws = true;
            continue;
        }
        // コメント
        if c == b'/' && i + 1 < n && s[i + 1] == b'/' {
            while i < n && s[i] != b'\n' {
                i += 1;
            }
            st.prev_ws = true;
            continue;
        }
        if c == b'/' && i + 1 < n && s[i + 1] == b'*' {
            let mut d = 0;
            while i < n {
                if s[i] == b'/' && i + 1 < n && s[i + 1] == b'*' {
                    d += 1;
                    i += 2;
                } else if s[i] == b'*' && i + 1 < n && s[i + 1] == b'/' {
                    d -= 1;
                    i += 2;
                    if d == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            st.prev_ws = true;
            continue;
        }
        let start = i;
        // 文字列系リテラル (接頭辞 b / r / br / c / cr を含む)
        if let Some(end) = string_lit(s, i) {
            i = end;
            tok!(start, i, 1u8);
            continue;
        }
        if is_ident_start(c) {
            if c == b'r' && i + 2 < n && s[i + 1] == b'#' && is_ident_start(s[i + 2]) {
                i += 2; // raw ident
            }
            while i < n && is_ident_cont(s[i]) {
                i += 1;
            }
            tok!(start, i, 2u8);
            if depth == base && st.n_head < 2 {
                let w = &s[start..i];
                if !matches!(w, b"pub" | b"crate" | b"super" | b"in") {
                    st.head[st.n_head] = (start as u32, i as u32);
                    st.n_head += 1;
                    let h0 = &s[st.head[0].0 as usize..st.head[0].1 as usize];
                    semi_only = match (h0, st.n_head) {
                        (b"use" | b"static" | b"type", _) => true,
                        (b"const", 1) => true,
                        (b"const", _) => {
                            let h1 = &s[st.head[1].0 as usize..st.head[1].1 as usize];
                            !matches!(h1, b"fn" | b"unsafe" | b"extern" | b"async")
                        }
                        _ => false,
                    };
                }
            }
            if depth == 0 && !st.seen_ident {
                let w = &s[start..i];
                // `unsafe impl` / `pub trait` に備えて unsafe と pub は読み飛ばす
                if w != b"unsafe" && w != b"pub" {
                    st.seen_ident = true;
                    impl_pending = w == b"impl" || w == b"trait";
                    is_trait = w == b"trait";
                }
            }
            continue;
        }
        if c.is_ascii_digit() {
            while i < n
                && (is_ident_cont(s[i]) || (s[i] == b'.' && i + 1 < n && s[i + 1].is_ascii_digit()))
            {
                i += 1;
            }
            tok!(start, i, 3u8);
            continue;
        }
        if c == b'\'' {
            // 文字リテラル or ライフタイム
            if i + 1 < n && s[i + 1] == b'\\' {
                i += 2;
                while i < n && s[i] != b'\'' {
                    i += 1;
                }
                i += 1;
                tok!(start, i.min(n), 1u8);
                continue;
            }
            let ch_len = utf8_len(s.get(i + 1).copied().unwrap_or(0));
            if i + 1 + ch_len < n && s[i + 1 + ch_len] == b'\'' {
                i += 2 + ch_len;
                tok!(start, i, 1u8);
                continue;
            }
            i += 1;
            while i < n && is_ident_cont(s[i]) {
                i += 1;
            }
            tok!(start, i, 4u8);
            continue;
        }
        // 記号 (1 byte ずつ。直前が空白かどうかもハッシュに入れて `&&` と `& &` を区別)
        i += 1;
        // impl ブロックの閉じ括弧はどのアイテムにも属さない
        if c == b'}' && impl_header.is_some() && depth == 1 {
            if st.cur.is_some() {
                return Err(format!(
                    "{}:1: unterminated item in impl block",
                    st.line_at(start)
                ));
            }
            depth = 0;
            base = 0;
            impl_header = None;
            continue;
        }
        if c == b'#' && depth == 0 && st.cur.is_none() {
            let mut j = i;
            while j < n && s[j].is_ascii_whitespace() {
                j += 1;
            }
            inner_attr = j < n && s[j] == b'!';
        }
        tok!(start, i, 5u8);
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth < 0 {
                    return Err(format!(
                        "{}:1: unbalanced `{}`",
                        st.line_at(start),
                        c as char
                    ));
                }
            }
            _ => {}
        }
        let header_end = c == b'{' && depth == 1 && impl_pending;
        let attr_end = inner_attr && c == b']' && depth == 0;
        let item_end = depth == base && (c == b';' || (c == b'}' && !semi_only));
        if header_end || attr_end || item_end {
            let (sp, ln, idents) = st.cur.take().unwrap();
            let mut h = KeyHasher::new();
            let kind = if attr_end {
                inner_attr = false;
                ItemKind::InnerAttr
            } else if header_end {
                if is_trait {
                    ItemKind::TraitHeader
                } else {
                    ItemKind::ImplHeader
                }
            } else if let Some(hi) = impl_header {
                // メンバのハッシュにはヘッダ (= self 型 / trait 名) を含める
                h.add(&items[hi as usize].hash);
                if is_trait {
                    ItemKind::TraitMember(hi)
                } else {
                    ItemKind::ImplMember(hi)
                }
            } else {
                ItemKind::Normal
            };
            h.add(&st.buf[..]);
            st.buf.clear();
            st.seen_ident = false;
            st.n_head = 0;
            semi_only = false;
            if header_end {
                impl_pending = false;
                impl_header = Some(items.len() as u32);
                base = 1;
            }
            items.push(SrcItem {
                start: sp,
                end: i,
                line: ln,
                hash: h.finish(),
                idents,
                kind,
            });
        }
    }
    if impl_header.is_some() {
        return Err("unterminated impl block".into());
    }
    if let Some((sp, ln, ..)) = st.cur {
        return Err(format!(
            "{ln}:1: unterminated item starting here (byte {sp})"
        ));
    }
    Ok(items)
}

fn utf8_len(b: u8) -> usize {
    match b {
        0xf0.. => 4,
        0xe0.. => 3,
        0xc0.. => 2,
        _ => 1,
    }
}

/// 位置 i から文字列リテラルが始まっていればその終端を返す。
fn string_lit(s: &[u8], mut i: usize) -> Option<usize> {
    let n = s.len();
    // 接頭辞
    if i < n && (s[i] == b'b' || s[i] == b'c') {
        if i + 1 < n && (s[i + 1] == b'"' || s[i + 1] == b'r') {
            i += 1;
        } else if s[i] == b'b' && i + 1 < n && s[i + 1] == b'\'' {
            // b'x'
            let mut j = i + 2;
            if j < n && s[j] == b'\\' {
                j += 1;
            }
            j += 1;
            while j < n && s[j] != b'\'' {
                j += 1;
            }
            return Some((j + 1).min(n));
        } else {
            return None;
        }
    }
    if i < n && s[i] == b'r' {
        let mut j = i + 1;
        let mut hashes = 0;
        while j < n && s[j] == b'#' {
            hashes += 1;
            j += 1;
        }
        if j >= n || s[j] != b'"' {
            return None;
        }
        j += 1;
        loop {
            if j >= n {
                return Some(n);
            }
            if s[j] == b'"'
                && s.len() >= j + 1 + hashes
                && s[j + 1..j + 1 + hashes].iter().all(|&c| c == b'#')
            {
                return Some(j + 1 + hashes);
            }
            j += 1;
        }
    }
    if i < n && s[i] == b'"' {
        let mut j = i + 1;
        while j < n && s[j] != b'"' {
            if s[j] == b'\\' {
                j += 1;
            }
            j += 1;
        }
        return Some((j + 1).min(n));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hashes(src: &str) -> Vec<Key> {
        split(src).unwrap().iter().map(|i| i.hash).collect()
    }

    #[test]
    fn formatting_and_comments_do_not_change_hash() {
        let a = hashes("fn f(x: u32) -> u32 { x + 1 }\nfn g() {}");
        let b = hashes("// hi\nfn f(x: u32)->u32{\n  /* c */ x+1\n}\n\nfn g() { }");
        assert_eq!(a, b);
    }

    #[test]
    fn edits_change_only_that_item() {
        let a = hashes("fn f() { 1; }\nfn g() { 2; }");
        let b = hashes("fn f() { 1; }\nfn g() { 3; }");
        assert_eq!(a[0], b[0]);
        assert_ne!(a[1], b[1]);
    }

    #[test]
    fn impl_members_are_separate_items() {
        let src = "struct S { x: i64 }\nimpl S {\n    fn a(&self) -> i64 { self.x }\n    fn b() {}\n}\nfn main() {}";
        let it = split(src).unwrap();
        let kinds: Vec<_> = it.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ItemKind::Normal,
                ItemKind::ImplHeader,
                ItemKind::ImplMember(1),
                ItemKind::ImplMember(1),
                ItemKind::Normal
            ]
        );
        assert_eq!(
            &src[it[2].start..it[2].end],
            "fn a(&self) -> i64 { self.x }"
        );
        assert_eq!(it[2].line, 3);
        // メソッド b だけ編集すると a のハッシュは変わらない
        let it2 = split(&src.replace("fn b() {}", "fn b() { 1; }")).unwrap();
        assert_eq!(it[2].hash, it2[2].hash);
        assert_ne!(it[3].hash, it2[3].hash);
        // self 型が変わるとメンバ全部が変わる
        let it3 = split(&src.replace("impl S", "impl T")).unwrap();
        assert_ne!(it[2].hash, it3[2].hash);
    }

    #[test]
    fn use_and_const_items_end_at_semicolon() {
        let src = "use core::cmp::{max, min};\nconst P: Pt = Pt { x: 1 };\nconst fn f() -> u8 { 1 }\nfn main() {}";
        let it = split(src).unwrap();
        let texts: Vec<&str> = it.iter().map(|i| &src[i.start..i.end]).collect();
        assert_eq!(
            texts,
            vec![
                "use core::cmp::{max, min};",
                "const P: Pt = Pt { x: 1 };",
                "const fn f() -> u8 { 1 }",
                "fn main() {}"
            ]
        );
    }

    #[test]
    fn jointness_matters() {
        assert_ne!(hashes("fn f() { a && b; }"), hashes("fn f() { a & &b; }"));
    }

    #[test]
    fn strings_chars_lifetimes() {
        let it = split(
            "fn f() { let s = \"}\"; let c = '}'; let b = b'}'; let r = r#\"}\"#; }\nfn g<'a>() {}",
        )
        .unwrap();
        assert_eq!(it.len(), 2);
    }
}

/// 1 ファイルの中のモジュール `mod name { .. }` を平らにする: `mod name {` (と前の `pub` / 属性) と
/// 対応する `}` を空白で塗りつぶし、中のアイテムをトップレベルのアイテムにする (バイト位置と行は変えない)。
/// `#[cfg(test)] mod tests { .. }` はまるごと消す。モジュールのパス (`geo::f`) は名前解決が先頭を読み飛ばす。
/// モジュールがなければ None
/// 簡単な字句解析: (種類, 開始, 終了)。0 = 識別子, 1 = 記号 1 文字, 2 = リテラル (コメントは飛ばす)
fn tokenize(s: &[u8]) -> Vec<(u8, usize, usize)> {
    let n = s.len();
    // (種類, 開始, 終了): 0 = 識別子, 1 = 記号 1 文字, 2 = リテラル
    let mut toks: Vec<(u8, usize, usize)> = vec![];
    let mut i = 0;
    while i < n {
        let c = s[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < n && s[i + 1] == b'/' {
            while i < n && s[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < n && s[i + 1] == b'*' {
            let mut d = 0;
            while i < n {
                if s[i] == b'/' && i + 1 < n && s[i + 1] == b'*' {
                    d += 1;
                    i += 2;
                } else if s[i] == b'*' && i + 1 < n && s[i + 1] == b'/' {
                    d -= 1;
                    i += 2;
                    if d == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if let Some(end) = string_lit(s, i) {
            toks.push((2, i, end));
            i = end;
            continue;
        }
        if is_ident_start(c) {
            let st = i;
            while i < n && is_ident_cont(s[i]) {
                i += 1;
            }
            toks.push((0, st, i));
            continue;
        }
        if c == b'\'' {
            // 文字リテラル (ライフタイムは識別子の続きとして読む)
            let st = i;
            if i + 1 < n && s[i + 1] == b'\\' {
                i += 2;
                while i < n && s[i] != b'\'' {
                    i += 1;
                }
                i += 1;
                toks.push((2, st, i.min(n)));
                continue;
            }
            let l = utf8_len(s.get(i + 1).copied().unwrap_or(0));
            if i + 1 + l < n && s[i + 1 + l] == b'\'' {
                i += 2 + l;
                toks.push((2, st, i));
                continue;
            }
            i += 1;
            continue;
        }
        toks.push((1, i, i + 1));
        i += 1;
    }
    toks
}

/// 関数の本体の中に書いたアイテム (`fn helper() {..}` / `struct` / `enum` / `const` / `use`) をファイルの末尾へ移す。
/// 元の場所は空白にして (改行は残す)、後ろの行番号を変えない
pub fn hoist_nested_items(src: &str) -> Option<String> {
    // (速く飛ばす: 字下げした行頭の `fn ` などがなければ入れ子のアイテムはない)
    let indented = src.split('\n').any(|l| {
        let t = l.trim_start();
        t.len() < l.len() && ["fn ", "struct ", "enum ", "use ", "const ", "static "].iter().any(|k| t.starts_with(k))
    });
    if !indented {
        return None;
    }
    let s = src.as_bytes();
    let toks = tokenize(s);
    let tok = |k: usize| -> &[u8] { &s[toks[k].1..toks[k].2] };
    let is_sym = |k: usize, c: &[u8]| toks[k].0 == 1 && tok(k) == c;
    // 各位置の波括弧の深さと、関数の本体の深さのスタック
    let mut depth = 0i32;
    let mut fn_bodies: Vec<i32> = vec![];
    let mut pending_fn = false;
    let mut moves: Vec<(usize, usize)> = vec![];
    let mut k = 0;
    while k < toks.len() {
        let t = tok(k);
        if toks[k].0 == 0 {
            let in_body = fn_bodies.last().is_some_and(|&d| depth > d);
            let stmt_start = k == 0 || is_sym(k - 1, b"{") || is_sym(k - 1, b"}") || is_sym(k - 1, b";") || is_sym(k - 1, b"]");
            let item_kw = matches!(t, b"fn" | b"struct" | b"enum" | b"const" | b"use" | b"static");
            // (`const` の後ろに `fn` や `|` (const クロージャ) が来るものは別扱い: `const fn` は fn で見る)
            if in_body && stmt_start && item_kw && k + 1 < toks.len() && toks[k + 1].0 == 0 {
                // アイテムの終わり: `{ .. }` (fn / struct / enum) か `;`
                let mut j = k + 1;
                let mut d = 0i32;
                let mut end = None;
                while j < toks.len() {
                    if toks[j].0 == 1 {
                        match tok(j) {
                            b"(" | b"[" => d += 1,
                            b")" | b"]" => d -= 1,
                            b";" if d == 0 => {
                                end = Some(j);
                                break;
                            }
                            b"{" if d == 0 => {
                                // 対応する }
                                let mut e = 0i32;
                                let mut m = j;
                                while m < toks.len() {
                                    if is_sym(m, b"{") {
                                        e += 1;
                                    } else if is_sym(m, b"}") {
                                        e -= 1;
                                        if e == 0 {
                                            break;
                                        }
                                    }
                                    m += 1;
                                }
                                end = Some(m);
                                break;
                            }
                            _ => {}
                        }
                    }
                    j += 1;
                }
                let Some(e) = end else { return None };
                if e >= toks.len() {
                    return None;
                }
                // 前の `#[..]` も一緒に
                let mut st = k;
                while st >= 1 && is_sym(st - 1, b"]") {
                    let mut m = st - 1;
                    let mut d = 0;
                    loop {
                        if is_sym(m, b"]") {
                            d += 1;
                        } else if is_sym(m, b"[") {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        if m == 0 {
                            break;
                        }
                        m -= 1;
                    }
                    if m >= 1 && is_sym(m - 1, b"#") {
                        st = m - 1;
                    } else {
                        break;
                    }
                }
                moves.push((toks[st].1, toks[e].2));
                k = e + 1;
                continue;
            }
            if t == b"fn" {
                pending_fn = true;
            }
        } else if toks[k].0 == 1 {
            match t {
                b"{" => {
                    if pending_fn {
                        fn_bodies.push(depth);
                        pending_fn = false;
                    }
                    depth += 1;
                }
                b"}" => {
                    depth -= 1;
                    if fn_bodies.last() == Some(&depth) {
                        fn_bodies.pop();
                    }
                }
                // `fn f();` (宣言だけ) は本体なし
                b";" => pending_fn = false,
                _ => {}
            }
        }
        k += 1;
    }
    if moves.is_empty() {
        return None;
    }
    let mut out = s.to_vec();
    let mut tail = Vec::new();
    for &(a, b) in &moves {
        tail.push(b'\n');
        tail.extend_from_slice(&s[a..b]);
        for x in &mut out[a..b] {
            if *x != b'\n' {
                *x = b' ';
            }
        }
    }
    out.extend_from_slice(&tail);
    out.push(b'\n');
    String::from_utf8(out).ok()
}

pub fn flatten_modules(src: &str) -> Option<String> {
    if !src.contains("mod") {
        return None;
    }
    let s = src.as_bytes();
    let toks = tokenize(s);
    let tok = |k: usize| -> &[u8] { &s[toks[k].1..toks[k].2] };
    // 対応する閉じ括弧のトークン位置
    let matching = |k: usize| -> Option<usize> {
        let (open, close) = match tok(k) {
            b"{" => (b"{", b"}"),
            b"[" => (b"[", b"]"),
            b"(" => (b"(", b")"),
            _ => return None,
        };
        let mut d = 0;
        for j in k..toks.len() {
            if toks[j].0 == 1 {
                if tok(j) == open {
                    d += 1;
                } else if tok(j) == close {
                    d -= 1;
                    if d == 0 {
                        return Some(j);
                    }
                }
            }
        }
        None
    };
    let mut out = s.to_vec();
    let blank = |out: &mut Vec<u8>, a: usize, b: usize| {
        for x in &mut out[a..b] {
            if *x != b'\n' {
                *x = b' ';
            }
        }
    };
    let mut changed = false;
    let mut k = 0;
    while k + 2 < toks.len() {
        if !(toks[k].0 == 0 && tok(k) == b"mod" && toks[k + 1].0 == 0 && tok(k + 2) == b"{") {
            k += 1;
            continue;
        }
        let Some(close) = matching(k + 2) else { return None };
        // 前の `pub` / `pub(crate)` と属性 `#[..]` も含める
        let mut st = k;
        let mut is_test = false;
        loop {
            if st >= 1 && toks[st - 1].0 == 0 && tok(st - 1) == b"pub" {
                st -= 1;
                continue;
            }
            if st >= 1 && tok(st - 1) == b")" {
                // `pub(crate)`
                let mut j = st - 1;
                while j > 0 && tok(j) != b"(" {
                    j -= 1;
                }
                if j >= 1 && tok(j - 1) == b"pub" {
                    st = j - 1;
                    continue;
                }
            }
            if st >= 1 && tok(st - 1) == b"]" {
                let mut j = st - 1;
                let mut d = 0;
                loop {
                    if tok(j) == b"]" {
                        d += 1;
                    } else if tok(j) == b"[" {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    if j == 0 {
                        break;
                    }
                    j -= 1;
                }
                if j >= 1 && tok(j - 1) == b"#" {
                    let attr = &s[toks[j].1..toks[st - 1].2];
                    let a = String::from_utf8_lossy(attr);
                    if a.contains("cfg") && a.contains("test") {
                        is_test = true;
                    }
                    st = j - 1;
                    continue;
                }
            }
            break;
        }
        changed = true;
        if is_test {
            blank(&mut out, toks[st].1, toks[close].2);
            k = close + 1;
        } else {
            blank(&mut out, toks[st].1, toks[k + 2].2);
            blank(&mut out, toks[close].1, toks[close].2);
            k += 3;
        }
    }
    changed.then(|| String::from_utf8(out).unwrap_or_default())
}
