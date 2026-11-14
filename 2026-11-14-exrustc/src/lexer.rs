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
        let op = kind == 5 && b"&|=<>!-+*/%^.:#@~?$".contains(&self.s[st]);
        let joint = op && self.prev_punct && !self.prev_ws;
        // (種類, 長さ, 本体) の列にする。長さを入れるので曖昧さがない
        self.buf.push(kind << 1 | joint as u8);
        self.buf.extend_from_slice(&((en - st) as u32).to_le_bytes());
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
    let mut st = St { s, cur: None, prev_ws: true, prev_punct: false, buf: Vec::with_capacity(4096), line_cache: (0, 1), seen_ident: false };
    // impl ブロックの中にいるときは、メンバの区切りを depth 1 で判定する
    let mut base: i32 = 0;
    let mut impl_header: Option<u32> = None;
    let mut impl_pending = false;
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
            if depth == 0 && !st.seen_ident {
                let w = &s[start..i];
                // `unsafe impl` に備えて unsafe は読み飛ばす
                if w != b"unsafe" {
                    st.seen_ident = true;
                    impl_pending = w == b"impl";
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
                return Err(format!("{}:1: unterminated item in impl block", st.line_at(start)));
            }
            depth = 0;
            base = 0;
            impl_header = None;
            continue;
        }
        tok!(start, i, 5u8);
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth < 0 {
                    return Err(format!("{}:1: unbalanced `{}`", st.line_at(start), c as char));
                }
            }
            _ => {}
        }
        let header_end = c == b'{' && depth == 1 && impl_pending;
        if header_end || (depth == base && (c == b';' || c == b'}')) {
            let (sp, ln, idents) = st.cur.take().unwrap();
            let mut h = KeyHasher::new();
            let kind = if header_end {
                ItemKind::ImplHeader
            } else if let Some(hi) = impl_header {
                // メンバのハッシュにはヘッダ (= self 型) を含める
                h.add(&items[hi as usize].hash);
                ItemKind::ImplMember(hi)
            } else {
                ItemKind::Normal
            };
            h.add(&st.buf[..]);
            st.buf.clear();
            st.seen_ident = false;
            if header_end {
                impl_pending = false;
                impl_header = Some(items.len() as u32);
                base = 1;
            }
            items.push(SrcItem { start: sp, end: i, line: ln, hash: h.finish(), idents, kind });
        }
    }
    if impl_header.is_some() {
        return Err("unterminated impl block".into());
    }
    if let Some((sp, ln, ..)) = st.cur {
        return Err(format!("{ln}:1: unterminated item starting here (byte {sp})"));
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
            if s[j] == b'"' && s.len() >= j + 1 + hashes && s[j + 1..j + 1 + hashes].iter().all(|&c| c == b'#') {
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
            vec![ItemKind::Normal, ItemKind::ImplHeader, ItemKind::ImplMember(1), ItemKind::ImplMember(1), ItemKind::Normal]
        );
        assert_eq!(&src[it[2].start..it[2].end], "fn a(&self) -> i64 { self.x }");
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
    fn jointness_matters() {
        assert_ne!(hashes("fn f() { a && b; }"), hashes("fn f() { a & &b; }"));
    }

    #[test]
    fn strings_chars_lifetimes() {
        let it = split("fn f() { let s = \"}\"; let c = '}'; let b = b'}'; let r = r#\"}\"#; }\nfn g<'a>() {}").unwrap();
        assert_eq!(it.len(), 2);
    }
}
