//! クロージャの解析 (src/closure.rs と同じこと) を、自前の構文木 (src/parse.rs) の上で行う。
//! syn から離れるための移植の 1 つ目: 本体の中のクロージャ (前順・マクロの引数の中も)、自由変数、
//! キャプチャを値として使うか。tests/ast_closure.rs が syn 版と同じ結果になることを確かめる。
//! 外部クレートに依存しないので crustc 自身でもコンパイルできる。

use crate::parse::{Block, Expr, Pat, Stmt, TokKind, parse_expr, tokenize};

/// マクロの引数 (`( a , b )` のテキスト) を `,` 区切りの式の列として読む。読めなければ None
pub fn macro_args(g: &str) -> Option<Vec<Expr>> {
    let inner = strip_delims(g)?;
    let mut out = Vec::new();
    if inner.trim().is_empty() {
        return Some(out);
    }
    // (`,` で分けてから式として読む: 括弧の中の `,` は分けない)
    for part in split_top_commas(inner)? {
        if part.trim().is_empty() {
            continue;
        }
        out.push(parse_expr(&part).ok()?);
    }
    Some(out)
}

fn strip_delims(g: &str) -> Option<&str> {
    let t = g.trim();
    let first = t.chars().next()?;
    let last = t.chars().last()?;
    if !matches!(first, '(' | '[' | '{') || !matches!(last, ')' | ']' | '}') || t.len() < 2 {
        return None;
    }
    Some(&t[1..t.len() - 1])
}

/// トップレベルの `,` で分ける (トークン単位: 文字列・文字の中の `,` は数えない)
fn split_top_commas(s: &str) -> Option<Vec<String>> {
    let toks = tokenize(s).ok()?;
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0usize;
    for t in &toks {
        match t.kind {
            TokKind::Open => depth += 1,
            TokKind::Close => depth -= 1,
            TokKind::Punct if depth == 0 && t.text == "," => {
                parts.push(s[start..t.lo as usize].to_string());
                start = t.lo as usize + 1;
            }
            _ => {}
        }
    }
    parts.push(s[start..].to_string());
    Some(parts)
}

/// マクロの引数のうちクロージャを含むもの (`|` の記号があるときだけ)。`vec![x; n]` も
pub fn macro_exprs(g: &str) -> Vec<Expr> {
    let Ok(toks) = tokenize(g) else { return Vec::new() };
    if !toks.iter().any(|t| t.kind == TokKind::Punct && t.text == "|") {
        return Vec::new();
    }
    if let Some(es) = macro_args(g) {
        return es;
    }
    let Some(inner) = strip_delims(g) else { return Vec::new() };
    match parse_expr(&format!("[{inner}]")) {
        Ok(e) => vec![e],
        Err(_) => Vec::new(),
    }
}

/// 子の式を順に f に渡す (ブロック・文の中も。アイテムには入らない)
fn each_child(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    match e {
        Expr::Lit(_) | Expr::Path(..) | Expr::Continue(_) => {}
        Expr::Unary(_, x) | Expr::Try(x) | Expr::Paren(x) | Expr::Ref(_, x) | Expr::RawRef(_, x) | Expr::Field(x, _) | Expr::Cast(x, _) => f(x),
        Expr::Binary(_, l, r) | Expr::Assign(l, r) | Expr::AssignOp(_, l, r) | Expr::Index(l, r) | Expr::Repeat(l, r) => {
            f(l);
            f(r);
        }
        Expr::Call(x, a) => {
            f(x);
            for y in a {
                f(y);
            }
        }
        Expr::MethodCall(x, _, _, a) => {
            f(x);
            for y in a {
                f(y);
            }
        }
        Expr::Range(a, b, _) => {
            if let Some(a) = a {
                f(a);
            }
            if let Some(b) = b {
                f(b);
            }
        }
        Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => block_exprs(b, f),
        Expr::If(c, t, e) => {
            f(c);
            block_exprs(t, f);
            if let Some(e) = e {
                f(e);
            }
        }
        Expr::Let(_, x) => f(x),
        Expr::Match(x, arms) => {
            f(x);
            for a in arms {
                if let Some(g) = &a.guard {
                    f(g);
                }
                f(&a.body);
            }
        }
        Expr::While(_, c, b) => {
            f(c);
            block_exprs(b, f);
        }
        Expr::For(_, _, x, b) => {
            f(x);
            block_exprs(b, f);
        }
        Expr::Break(_, v) | Expr::Return(v) => {
            if let Some(v) = v {
                f(v);
            }
        }
        Expr::Closure(_, _, _, b) => f(b),
        Expr::Tuple(v) | Expr::Array(v) => {
            for y in v {
                f(y);
            }
        }
        Expr::Struct(_, fs, base) => {
            for x in fs {
                f(&x.1);
            }
            if let Some(b) = base {
                f(b);
            }
        }
        Expr::Macro(..) => {}
    }
}

fn block_exprs(b: &Block, f: &mut dyn FnMut(&Expr)) {
    for s in &b.stmts {
        match s {
            Stmt::Let(_, _, init, els) => {
                if let Some(e) = init {
                    f(e);
                }
                if let Some(b) = els {
                    block_exprs(b, f);
                }
            }
            Stmt::Expr(e, _) => f(e),
            Stmt::Item(_) => {}
        }
    }
}

/// 本体の中のクロージャ式 (前順。マクロの引数の中も)
pub fn closures_of(b: &Block) -> Vec<Expr> {
    let mut out = Vec::new();
    block_exprs(b, &mut |e| collect_closures(e, &mut out));
    out
}

fn collect_closures(e: &Expr, out: &mut Vec<Expr>) {
    if let Expr::Closure(..) = e {
        out.push(e.clone());
    }
    if let Expr::Macro(_, g) = e {
        for x in macro_exprs(g) {
            collect_closures(&x, out);
        }
        return;
    }
    each_child(e, &mut |c| collect_closures(c, out));
}

/// closures_of と同じ順の、クロージャの場所: 本体に書かれたものはアドレス (Some)、マクロの引数の中のもの
/// (読み直すのでアドレスが決まらない) は None と式の Debug の文字列
pub fn closure_slots(b: &Block) -> Vec<(Option<usize>, String)> {
    let mut out = Vec::new();
    block_exprs(b, &mut |e| collect_slots(e, &mut out));
    out
}

fn collect_slots(e: &Expr, out: &mut Vec<(Option<usize>, String)>) {
    if let Expr::Closure(..) = e {
        out.push((Some(e as *const Expr as usize), format!("{e:?}")));
    }
    if let Expr::Macro(_, g) = e {
        let mut v = Vec::new();
        for x in macro_exprs(g) {
            collect_closures(&x, &mut v);
        }
        for c in v {
            out.push((None, format!("{c:?}")));
        }
        return;
    }
    each_child(e, &mut |c| collect_slots(c, out));
}

/// パターンが束縛する名前 (大文字で始まる裸の名前はパスのパターン)
pub fn pat_idents(p: &Pat, out: &mut Vec<String>) {
    match p {
        Pat::Ident(r, m, n, sub) => {
            let path_like = n.starts_with(|c: char| c.is_ascii_uppercase()) && sub.is_none() && !*r && !*m;
            if !path_like {
                out.push(n.clone());
            }
            if let Some(s) = sub {
                pat_idents(s, out);
            }
        }
        Pat::Tuple(v) | Pat::TupleStruct(_, v) | Pat::Or(v) | Pat::Slice(v) => {
            for x in v {
                pat_idents(x, out);
            }
        }
        Pat::Struct(_, fs, _) => {
            for f in fs {
                pat_idents(&f.1, out);
            }
        }
        Pat::Ref(_, x) => pat_idents(x, out),
        _ => {}
    }
}

/// 書式文字列の `{name}` / `{name:?}` に直接書いた名前
pub fn fmt_inline_names(f: &str) -> Vec<String> {
    let b = f.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'{' {
            if b.get(i + 1) == Some(&b'{') {
                i += 2;
                continue;
            }
            let st = i + 1;
            let mut j = st;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j > st && !b[st].is_ascii_digit() && matches!(b.get(j), Some(b'}') | Some(b':')) {
                out.push(f[st..j].to_string());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// 文字列リテラルの中身 (`"..."` / `r#"..."#`。エスケープはそのまま)
fn str_lit_body(t: &str) -> Option<&str> {
    let t = t.strip_prefix('r').unwrap_or(t);
    let t = t.trim_matches('#');
    t.strip_prefix('"')?.strip_suffix('"')
}

struct Free {
    scopes: Vec<Vec<String>>,
    out: Vec<String>,
}

impl Free {
    fn bound(&self, n: &str) -> bool {
        self.scopes.iter().any(|s| s.iter().any(|x| x == n))
    }
    fn bind(&mut self, p: &Pat) {
        let mut v = vec![];
        pat_idents(p, &mut v);
        self.scopes.last_mut().unwrap().extend(v);
    }
    fn name(&mut self, n: &str) {
        // (`_` は式としては推論の穴 (syn の Expr::Infer) で、変数ではない)
        if n != "_" && !self.bound(n) && !self.out.iter().any(|x| x == n) {
            self.out.push(n.to_string());
        }
    }
    fn block(&mut self, b: &Block) {
        self.scopes.push(vec![]);
        for s in &b.stmts {
            match s {
                Stmt::Let(p, _, init, els) => {
                    if let Some(e) = init {
                        self.expr(e);
                    }
                    if let Some(b) = els {
                        self.block(b);
                    }
                    self.bind(p);
                }
                Stmt::Expr(e, _) => self.expr(e),
                Stmt::Item(_) => {}
            }
        }
        self.scopes.pop();
    }
    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Path(None, p) => {
                if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() {
                    let n = p.segs[0].name.clone();
                    self.name(&n);
                }
            }
            Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => self.block(b),
            Expr::Closure(_, ps, _, body) => {
                self.scopes.push(vec![]);
                for (p, _) in ps {
                    self.bind(p);
                }
                self.expr(body);
                self.scopes.pop();
            }
            Expr::For(_, p, x, b) => {
                self.expr(x);
                self.scopes.push(vec![]);
                self.bind(p);
                self.block(b);
                self.scopes.pop();
            }
            Expr::Match(x, arms) => {
                self.expr(x);
                for a in arms {
                    self.scopes.push(vec![]);
                    self.bind(&a.pat);
                    if let Some(g) = &a.guard {
                        self.expr(g);
                    }
                    self.expr(&a.body);
                    self.scopes.pop();
                }
            }
            Expr::Let(p, x) => {
                self.expr(x);
                self.bind(p);
            }
            Expr::If(c, t, els) => {
                self.scopes.push(vec![]);
                self.expr(c);
                self.block(t);
                self.scopes.pop();
                if let Some(e) = els {
                    self.expr(e);
                }
            }
            Expr::While(_, c, b) => {
                self.scopes.push(vec![]);
                self.expr(c);
                self.block(b);
                self.scopes.pop();
            }
            Expr::Macro(mp, g) if mp.segs.len() == 1 && mp.segs[0].name == "matches" => {
                // `matches!(e, pat if guard)`: e と guard の中を見る (パターンの束縛は guard の中だけ)
                let Some(inner) = strip_delims(g) else { return };
                let Some(parts) = split_top_commas(inner) else { return };
                if parts.len() < 2 {
                    return;
                }
                if let Ok(e) = parse_expr(&parts[0]) {
                    self.expr(&e);
                }
                let rest = parts[1..].join(",");
                if let Some((pat, guard)) = crate::parse::parse_pat_guard(&rest) {
                    if let Some(gd) = guard {
                        self.scopes.push(vec![]);
                        self.bind(&pat);
                        self.expr(&gd);
                        self.scopes.pop();
                    }
                }
            }
            Expr::Macro(_, g) => {
                if let Some(es) = macro_args(g) {
                    for x in &es {
                        self.expr(x);
                        if let Expr::Lit(t) = x {
                            if let Some(body) = str_lit_body(t) {
                                for n in fmt_inline_names(body) {
                                    self.name(&n);
                                }
                            }
                        }
                    }
                }
            }
            _ => {
                let mut kids: Vec<Expr> = Vec::new();
                each_child(e, &mut |c| kids.push(c.clone()));
                for k in &kids {
                    self.expr(k);
                }
            }
        }
    }
}

/// クロージャの自由変数 (使われた順)
pub fn free_vars(c: &Expr) -> Vec<String> {
    let mut v = Free { scopes: vec![vec![]], out: Vec::new() };
    v.expr(c);
    v.out
}

fn is_cap(e: &Expr, caps: &[String]) -> bool {
    match e {
        Expr::Path(None, p) => !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() && caps.iter().any(|c| *c == p.segs[0].name),
        _ => false,
    }
}

fn moves_in(e: &Expr, caps: &[String]) -> bool {
    match e {
        Expr::Closure(..) => return false,
        Expr::Call(_, a) if a.iter().any(|x| is_cap(x, caps)) => return true,
        Expr::Return(Some(v)) if is_cap(v, caps) => return true,
        Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => {
            if b.stmts.iter().any(|s| matches!(s, Stmt::Let(_, _, Some(x), _) if is_cap(x, caps))) {
                return true;
            }
        }
        _ => {}
    }
    let mut found = false;
    each_child_stmts(e, &mut |c| {
        if !found && moves_in(c, caps) {
            found = true;
        }
    });
    found
}

/// each_child と同じだが、ブロックの let の右辺の判定 (moves_in の中) のために子のブロックも辿る
fn each_child_stmts(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    match e {
        Expr::If(c, t, els) => {
            f(c);
            f(&Expr::Block(None, false, t.clone()));
            if let Some(x) = els {
                f(x);
            }
        }
        Expr::While(_, c, b) => {
            f(c);
            f(&Expr::Block(None, false, b.clone()));
        }
        Expr::For(_, _, x, b) => {
            f(x);
            f(&Expr::Block(None, false, b.clone()));
        }
        _ => each_child(e, f),
    }
}

/// クロージャの本体がキャプチャした変数を値として使うか (関数の引数・本体の値・`let y = x;` の右辺・`return x`)
pub fn moves_capture(c: &Expr, caps: &[String]) -> bool {
    let Expr::Closure(_, _, _, body) = c else { return false };
    let tail = match &**body {
        Expr::Block(_, _, b) => b.stmts.last().and_then(|s| match s {
            Stmt::Expr(e, false) => Some(e),
            _ => None,
        }),
        e => Some(e),
    };
    if tail.is_some_and(|e| is_cap(e, caps)) {
        return true;
    }
    moves_in(body, caps)
}

/// 子の式を順に f に渡す (外から使う版)
pub fn each_child_pub(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    each_child(e, f)
}

fn assign_base(e: &Expr) -> Option<String> {
    match e {
        Expr::Path(None, p) if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() => Some(p.segs[0].name.clone()),
        Expr::Field(x, _) | Expr::Index(x, _) | Expr::Paren(x) | Expr::Unary(_, x) => assign_base(x),
        _ => None,
    }
}

fn assigns_in(e: &Expr, caps: &[String]) -> bool {
    match e {
        Expr::Closure(..) => return false,
        Expr::Assign(l, _) | Expr::AssignOp(_, l, _) if assign_base(l).is_some_and(|n| caps.iter().any(|c| *c == n)) => return true,
        _ => {}
    }
    let mut found = false;
    each_child(e, &mut |c| {
        if !found && assigns_in(c, caps) {
            found = true;
        }
    });
    found
}

/// クロージャの本体がキャプチャした変数に代入するか (`r = ..` / `n += 1` / `v.x = ..`)
pub fn assigns_capture(c: &Expr, caps: &[String]) -> bool {
    let Expr::Closure(_, _, _, body) = c else { return false };
    assigns_in(body, caps)
}
