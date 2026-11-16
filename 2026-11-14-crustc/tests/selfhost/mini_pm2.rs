// proc_macro2 の代わり (src/macros.rs が使う分だけ): 文字列を字句解析してトークン木にする。
// 位置 (byte_range / 行・列) は与えた文字列の中の位置。コメントは飛ばす (doc コメントも)。
// rustc 版と crustc 版の両方がこれを使うので、本物の proc_macro2 と細部が違っても出力は比べられる。
pub mod proc_macro2 {
    use std::fmt;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Delimiter {
        Parenthesis,
        Brace,
        Bracket,
        None,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Spacing {
        Alone,
        Joint,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct LineColumn {
        pub line: usize,
        pub column: usize,
    }

    #[derive(Clone, Copy, Debug)]
    pub struct Span {
        lo: usize,
        hi: usize,
        line: usize,
        column: usize,
    }

    impl Span {
        pub fn call_site() -> Span {
            Span { lo: 0, hi: 0, line: 1, column: 0 }
        }
        pub fn byte_range(&self) -> std::ops::Range<usize> {
            self.lo..self.hi
        }
        pub fn start(&self) -> LineColumn {
            LineColumn { line: self.line, column: self.column }
        }
    }

    #[derive(Clone, Debug)]
    pub struct Ident {
        name: String,
        span: Span,
    }

    impl Ident {
        pub fn span(&self) -> Span {
            self.span
        }
    }

    impl PartialEq for Ident {
        fn eq(&self, o: &Ident) -> bool {
            self.name == o.name
        }
    }

    impl PartialEq<str> for Ident {
        fn eq(&self, o: &str) -> bool {
            self.name == o
        }
    }

    impl fmt::Display for Ident {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "{}", self.name)
        }
    }

    #[derive(Clone, Debug)]
    pub struct Punct {
        ch: char,
        spacing: Spacing,
        span: Span,
    }

    impl Punct {
        pub fn as_char(&self) -> char {
            self.ch
        }
        pub fn spacing(&self) -> Spacing {
            self.spacing
        }
        pub fn span(&self) -> Span {
            self.span
        }
    }

    #[derive(Clone, Debug)]
    pub struct Literal {
        text: String,
        span: Span,
    }

    impl Literal {
        pub fn span(&self) -> Span {
            self.span
        }
    }

    impl fmt::Display for Literal {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "{}", self.text)
        }
    }

    #[derive(Clone, Debug)]
    pub struct Group {
        delim: Delimiter,
        stream: TokenStream,
        span: Span,
    }

    impl Group {
        pub fn new(delim: Delimiter, stream: TokenStream) -> Group {
            Group { delim: delim, stream: stream, span: Span::call_site() }
        }
        pub fn delimiter(&self) -> Delimiter {
            self.delim
        }
        pub fn stream(&self) -> TokenStream {
            self.stream.clone()
        }
        pub fn span(&self) -> Span {
            self.span
        }
    }

    #[derive(Clone, Debug)]
    pub enum TokenTree {
        Group(Group),
        Ident(Ident),
        Punct(Punct),
        Literal(Literal),
    }

    impl TokenTree {
        pub fn span(&self) -> Span {
            match self {
                TokenTree::Group(g) => g.span,
                TokenTree::Ident(i) => i.span,
                TokenTree::Punct(p) => p.span,
                TokenTree::Literal(l) => l.span,
            }
        }
    }

    #[derive(Clone, Debug, Default)]
    pub struct TokenStream {
        toks: Vec<TokenTree>,
    }

    #[derive(Debug)]
    pub struct LexError {
        msg: String,
    }

    impl fmt::Display for LexError {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "{}", self.msg)
        }
    }

    impl IntoIterator for TokenStream {
        type Item = TokenTree;
        type IntoIter = std::vec::IntoIter<TokenTree>;
        fn into_iter(self) -> std::vec::IntoIter<TokenTree> {
            self.toks.into_iter()
        }
    }

    const PUNCT: &str = "+-*/%^!&|=<>@.,;:#$?~'\\";

    struct Lexer<'a> {
        b: &'a [u8],
        s: &'a str,
        i: usize,
        line: usize,
        line_start: usize,
    }

    impl<'a> Lexer<'a> {
        fn span(&self, lo: usize, line: usize, col: usize) -> Span {
            Span { lo: lo, hi: self.i, line: line, column: col }
        }
        fn col(&self, at: usize) -> usize {
            // (UTF-8 の続きのバイトを数えない: chars().count() と同じ)
            self.b[self.line_start..at].iter().filter(|&&x| (x & 0xC0) != 0x80).count()
        }
        fn bump(&mut self) {
            if self.b[self.i] == b'\n' {
                self.line += 1;
                self.line_start = self.i + 1;
            }
            self.i += 1;
        }
        fn skip_trivia(&mut self) -> Result<(), LexError> {
            while self.i < self.b.len() {
                let c = self.b[self.i];
                if c.is_ascii_whitespace() {
                    self.bump();
                } else if self.s[self.i..].starts_with("//") {
                    while self.i < self.b.len() && self.b[self.i] != b'\n' {
                        self.bump();
                    }
                } else if self.s[self.i..].starts_with("/*") {
                    let mut depth = 0;
                    loop {
                        if self.i >= self.b.len() {
                            return Err(LexError { msg: "unterminated block comment".to_string() });
                        }
                        if self.s[self.i..].starts_with("/*") {
                            depth += 1;
                            self.bump();
                            self.bump();
                        } else if self.s[self.i..].starts_with("*/") {
                            depth -= 1;
                            self.bump();
                            self.bump();
                            if depth == 0 {
                                break;
                            }
                        } else {
                            self.bump();
                        }
                    }
                } else {
                    break;
                }
            }
            Ok(())
        }
        fn is_ident_start(c: char) -> bool {
            c == '_' || c.is_alphabetic()
        }
        fn is_ident_cont(c: char) -> bool {
            c == '_' || c.is_alphanumeric()
        }
        fn cur(&self) -> char {
            self.s[self.i..].chars().next().unwrap()
        }
        fn ident_end(&self, from: usize) -> usize {
            let mut j = from;
            for c in self.s[from..].chars() {
                if !Lexer::is_ident_cont(c) {
                    break;
                }
                j += c.len_utf8();
            }
            j
        }
        fn advance_to(&mut self, j: usize) {
            while self.i < j {
                self.bump();
            }
        }
        // 文字列 ("..")。i は開きの `"`
        fn quoted(&mut self, q: u8) -> Result<(), LexError> {
            self.bump();
            loop {
                if self.i >= self.b.len() {
                    return Err(LexError { msg: "unterminated literal".to_string() });
                }
                let c = self.b[self.i];
                if c == b'\\' {
                    self.bump();
                    if self.i < self.b.len() {
                        self.bump();
                    }
                } else if c == q {
                    self.bump();
                    return Ok(());
                } else {
                    self.bump();
                }
            }
        }
        // 生文字列 r#".."#。i は `r` の後ろ
        fn raw(&mut self) -> Result<(), LexError> {
            let mut h = 0;
            while self.i < self.b.len() && self.b[self.i] == b'#' {
                h += 1;
                self.bump();
            }
            if self.i >= self.b.len() || self.b[self.i] != b'"' {
                return Err(LexError { msg: "bad raw string".to_string() });
            }
            self.bump();
            let close = format!("\"{}", "#".repeat(h));
            loop {
                if self.i >= self.b.len() {
                    return Err(LexError { msg: "unterminated raw string".to_string() });
                }
                if self.s[self.i..].starts_with(&close) {
                    let j = self.i + close.len();
                    self.advance_to(j);
                    return Ok(());
                }
                self.bump();
            }
        }
        fn suffix(&mut self) {
            if self.i < self.b.len() && Lexer::is_ident_start(self.cur()) {
                let j = self.ident_end(self.i);
                self.advance_to(j);
            }
        }
        fn stream(&mut self, close: Option<u8>) -> Result<TokenStream, LexError> {
            let mut toks = Vec::new();
            loop {
                self.skip_trivia()?;
                if self.i >= self.b.len() {
                    if close.is_some() {
                        return Err(LexError { msg: "unclosed delimiter".to_string() });
                    }
                    return Ok(TokenStream { toks: toks });
                }
                let lo = self.i;
                let (line, col) = (self.line, self.col(lo));
                let c = self.b[self.i];
                if Some(c) == close {
                    return Ok(TokenStream { toks: toks });
                }
                match c {
                    b'(' | b'[' | b'{' => {
                        let (d, cl) = match c {
                            b'(' => (Delimiter::Parenthesis, b')'),
                            b'[' => (Delimiter::Bracket, b']'),
                            _ => (Delimiter::Brace, b'}'),
                        };
                        self.bump();
                        let inner = self.stream(Some(cl))?;
                        self.bump();
                        toks.push(TokenTree::Group(Group { delim: d, stream: inner, span: self.span(lo, line, col) }));
                    }
                    b')' | b']' | b'}' => {
                        return Err(LexError { msg: "unexpected closing delimiter".to_string() });
                    }
                    b'"' => {
                        self.quoted(b'"')?;
                        self.suffix();
                        toks.push(self.lit(lo, line, col));
                    }
                    b'\'' => {
                        // 文字 'x' / '\n' か、ライフタイム 'a
                        let rest = &self.s[self.i + 1..];
                        let mut it = rest.chars();
                        let c1 = it.next();
                        let c2 = it.next();
                        if c1 == Some('\\') || (c1.is_some() && c2 == Some('\'')) {
                            self.quoted(b'\'')?;
                            self.suffix();
                            toks.push(self.lit(lo, line, col));
                        } else {
                            self.bump();
                            toks.push(TokenTree::Punct(Punct { ch: '\'', spacing: Spacing::Joint, span: self.span(lo, line, col) }));
                        }
                    }
                    b'0'..=b'9' => {
                        while self.i < self.b.len() {
                            let d = self.b[self.i];
                            if d.is_ascii_alphanumeric() || d == b'_' {
                                // (指数 `1e-5`)
                                let e = (d == b'e' || d == b'E') && !self.s[lo..self.i].starts_with("0x");
                                self.bump();
                                if e && self.i < self.b.len() && (self.b[self.i] == b'-' || self.b[self.i] == b'+') {
                                    self.bump();
                                }
                            } else if d == b'.'
                                && self.b.get(self.i + 1).is_some_and(|n| n.is_ascii_digit())
                                && !self.s[lo..self.i].contains('.')
                            {
                                self.bump();
                            } else if d == b'.'
                                && !self.s[lo..self.i].contains('.')
                                && !self.b.get(self.i + 1).is_some_and(|n| *n == b'.' || n.is_ascii_alphabetic() || *n == b'_')
                            {
                                // (`1.` も浮動小数点)
                                self.bump();
                                break;
                            } else {
                                break;
                            }
                        }
                        toks.push(self.lit(lo, line, col));
                    }
                    _ if (c == b'r' || c == b'b')
                        && (self.s[self.i..].starts_with("r\"")
                            || self.s[self.i..].starts_with("r#\"")
                            || self.s[self.i..].starts_with("r##")
                            || self.s[self.i..].starts_with("br\"")
                            || self.s[self.i..].starts_with("br#")
                            || self.s[self.i..].starts_with("b\"")
                            || self.s[self.i..].starts_with("b'")) =>
                    {
                        if c == b'b' {
                            self.bump();
                        }
                        if self.b[self.i] == b'r' {
                            self.bump();
                            self.raw()?;
                        } else {
                            let q = self.b[self.i];
                            self.quoted(q)?;
                        }
                        self.suffix();
                        toks.push(self.lit(lo, line, col));
                    }
                    _ if Lexer::is_ident_start(self.cur()) => {
                        // (生識別子 r#match)
                        let start = if self.s[self.i..].starts_with("r#") { self.i + 2 } else { self.i };
                        let j = self.ident_end(start);
                        self.advance_to(j);
                        let name = self.s[lo..j].to_string();
                        toks.push(TokenTree::Ident(Ident { name: name, span: self.span(lo, line, col) }));
                    }
                    _ => {
                        let ch = self.cur();
                        if !PUNCT.contains(ch) {
                            return Err(LexError { msg: format!("unexpected character {ch:?}") });
                        }
                        self.bump();
                        let joint = self.i < self.b.len() && PUNCT.contains(self.cur()) && self.b[self.i] != b'\'';
                        let sp = if joint { Spacing::Joint } else { Spacing::Alone };
                        toks.push(TokenTree::Punct(Punct { ch: ch, spacing: sp, span: self.span(lo, line, col) }));
                    }
                }
            }
        }
        fn lit(&self, lo: usize, line: usize, col: usize) -> TokenTree {
            TokenTree::Literal(Literal { text: self.s[lo..self.i].to_string(), span: self.span(lo, line, col) })
        }
    }

    impl TokenStream {
        pub fn new() -> TokenStream {
            TokenStream { toks: Vec::new() }
        }
        pub fn from_str(s: &str) -> Result<TokenStream, LexError> {
            let mut lx = Lexer { b: s.as_bytes(), s: s, i: 0, line: 1, line_start: 0 };
            lx.stream(None)
        }
        pub fn is_empty(&self) -> bool {
            self.toks.is_empty()
        }
    }
}
