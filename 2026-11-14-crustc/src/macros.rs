//! `macro_rules!` の展開 (ソースのテキストの段階で行う)。
//!
//! 1 ファイルの中の `macro_rules! name { (pattern) => { body }; .. }` を集め、`name!(..)` の呼び出しを
//! 展開したテキストで置き換えてから字句解析に渡す。定義は空白で消す。
//! - 展開は 1 行に収める (後ろの行番号を変えないため。エラーの列は展開の中ではずれる)
//! - 式の位置では `({ .. })` で包み、トップレベル (アイテムの位置) ではそのまま
//! - `$x:expr` などの断片は `( .. )` で包んで埋め込む (Rust の断片は不透明なので `$x * $x` の優先順位が変わらない)
//! - 断片の種類: expr / ident / ty / pat / path / literal / tt / block / stmt / lifetime / item。
//!   expr などはパターンの次の区切り (`,` / `;` / `=>` など) が出るまでのトークン列を取る
//! - 繰り返し `$( .. ) sep op` (op は `*` / `+` / `?`)。展開の中の呼び出しも (深さ 64 まで) 展開する

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use std::collections::HashMap;
use std::str::FromStr;

/// パターン / 本体のトークン (区切りの付いた繰り返しは Rep)
#[derive(Clone, Debug)]
enum Pt {
    Tok(TokenTree),
    Group(Delimiter, Vec<Pt>),
    Var(String, String),
    Rep(Vec<Pt>, Option<TokenTree>, char),
}

#[derive(Clone, Debug)]
enum Bind {
    One(Vec<TokenTree>, bool),
    Many(Vec<Bind>),
}

struct Rule {
    pat: Vec<Pt>,
    body: Vec<Pt>,
}

fn parse_pt(ts: TokenStream, is_pat: bool) -> Vec<Pt> {
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            TokenTree::Punct(p) if p.as_char() == '$' && i + 1 < toks.len() => match &toks[i + 1] {
                TokenTree::Ident(id) => {
                    // `$x:expr` (パターン) / `$x` (本体)
                    if is_pat
                        && i + 3 < toks.len()
                        && matches!(&toks[i + 2], TokenTree::Punct(c) if c.as_char() == ':')
                    {
                        if let TokenTree::Ident(k) = &toks[i + 3] {
                            out.push(Pt::Var(id.to_string(), k.to_string()));
                            i += 4;
                            continue;
                        }
                    }
                    out.push(Pt::Var(id.to_string(), String::new()));
                    i += 2;
                }
                TokenTree::Group(g) if g.delimiter() == Delimiter::Parenthesis => {
                    let inner = parse_pt(g.stream(), is_pat);
                    // `$( .. ) sep op` / `$( .. ) op`
                    let mut j = i + 2;
                    let mut sep = None;
                    let mut op = '*';
                    if let Some(TokenTree::Punct(p)) = toks.get(j) {
                        if matches!(p.as_char(), '*' | '+' | '?') {
                            op = p.as_char();
                            j += 1;
                        } else {
                            sep = Some(toks[j].clone());
                            j += 1;
                            if let Some(TokenTree::Punct(q)) = toks.get(j) {
                                if matches!(q.as_char(), '*' | '+' | '?') {
                                    op = q.as_char();
                                    j += 1;
                                }
                            }
                        }
                    } else if let Some(t) = toks.get(j) {
                        sep = Some(t.clone());
                        j += 1;
                        if let Some(TokenTree::Punct(q)) = toks.get(j) {
                            op = q.as_char();
                            j += 1;
                        }
                    }
                    out.push(Pt::Rep(inner, sep, op));
                    i = j;
                }
                _ => {
                    out.push(Pt::Tok(toks[i].clone()));
                    i += 1;
                }
            },
            TokenTree::Group(g) => {
                out.push(Pt::Group(g.delimiter(), parse_pt(g.stream(), is_pat)));
                i += 1;
            }
            t => {
                out.push(Pt::Tok(t.clone()));
                i += 1;
            }
        }
    }
    out
}

fn same_tok(a: &TokenTree, b: &TokenTree) -> bool {
    match (a, b) {
        (TokenTree::Ident(x), TokenTree::Ident(y)) => x == y,
        (TokenTree::Punct(x), TokenTree::Punct(y)) => x.as_char() == y.as_char(),
        (TokenTree::Literal(x), TokenTree::Literal(y)) => x.to_string() == y.to_string(),
        // (目印としてのグループ: 括弧の種類が同じなら)
        (TokenTree::Group(x), TokenTree::Group(y)) => x.delimiter() == y.delimiter(),
        _ => false,
    }
}

/// パターンの位置 k から後ろで最初に現れる具体的なトークン (断片の終わりの目印)
fn next_lit(pat: &[Pt], k: usize, follow: Option<&TokenTree>) -> Option<TokenTree> {
    for p in &pat[k..] {
        match p {
            Pt::Tok(t) => return Some(t.clone()),
            // (`$t:ty { .. }`: 次が括弧のグループなら、同じ種類の括弧で断片が終わる)
            Pt::Group(d, _) if *d != Delimiter::None => {
                return Some(TokenTree::Group(proc_macro2::Group::new(*d, TokenStream::new())));
            }
            Pt::Group(..) => return None,
            Pt::Rep(_, Some(s), _) => return Some(s.clone()),
            _ => return None,
        }
    }
    follow.cloned()
}

/// 1 つの断片を取る。取った数 (と、式として包むか)
fn take_frag(toks: &[TokenTree], kind: &str, stop: Option<&TokenTree>) -> Option<(usize, bool)> {
    match kind {
        "ident" | "lifetime" | "literal" | "tt" => {
            if toks.is_empty() {
                return None;
            }
            // (負のリテラル `-1` は 2 トークン)
            if kind == "literal" {
                if let (TokenTree::Punct(p), Some(TokenTree::Literal(_))) = (&toks[0], toks.get(1)) {
                    if p.as_char() == '-' {
                        return Some((2, false));
                    }
                }
            }
            if kind == "lifetime" {
                return Some((2.min(toks.len()), false));
            }
            Some((1, false))
        }
        "block" => match toks.first() {
            Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => Some((1, false)),
            _ => None,
        },
        _ => {
            // expr / ty / pat / path / stmt / item: 次の区切りまで (グループの中は見ない)
            let mut n = 0;
            while n < toks.len() {
                if let Some(s) = stop {
                    if same_tok(&toks[n], s) {
                        break;
                    }
                }
                // 式は `;` で、型は `,` で必ず終わる
                if let TokenTree::Punct(p) = &toks[n] {
                    if p.as_char() == ';' && kind != "item" {
                        break;
                    }
                    if p.as_char() == ',' && stop.is_none() && matches!(kind, "expr" | "ty" | "pat" | "path") {
                        break;
                    }
                    // `=>` は式の中に現れない
                    if p.as_char() == '=' && p.spacing() == Spacing::Joint {
                        if let Some(TokenTree::Punct(q)) = toks.get(n + 1) {
                            if q.as_char() == '>' {
                                break;
                            }
                        }
                    }
                }
                n += 1;
            }
            if n == 0 {
                return None;
            }
            Some((n, kind == "expr"))
        }
    }
}

/// パターン pat を toks 全体に合わせる
fn match_pat(pat: &[Pt], toks: &[TokenTree], binds: &mut HashMap<String, Bind>) -> bool {
    match match_prefix(pat, toks, binds, None) {
        Some(n) => n == toks.len(),
        None => false,
    }
}

/// pat を toks の前から合わせ、使ったトークン数を返す
fn match_prefix(pat: &[Pt], toks: &[TokenTree], binds: &mut HashMap<String, Bind>, follow: Option<&TokenTree>) -> Option<usize> {
    let mut i = 0;
    for (k, p) in pat.iter().enumerate() {
        match p {
            Pt::Tok(t) => {
                if i < toks.len() && same_tok(t, &toks[i]) {
                    i += 1;
                } else {
                    return None;
                }
            }
            Pt::Group(d, inner) => match toks.get(i) {
                Some(TokenTree::Group(g)) if g.delimiter() == *d => {
                    let sub: Vec<TokenTree> = g.stream().into_iter().collect();
                    if !match_pat(inner, &sub, binds) {
                        return None;
                    }
                    i += 1;
                }
                _ => return None,
            },
            Pt::Var(name, kind) => {
                let stop = next_lit(pat, k + 1, follow);
                let (n, wrap) = take_frag(&toks[i..], kind, stop.as_ref())?;
                binds.insert(name.clone(), Bind::One(toks[i..i + n].to_vec(), wrap));
                i += n;
            }
            Pt::Rep(inner, sep, op) => {
                let stop = next_lit(pat, k + 1, follow);
                let mut items: Vec<HashMap<String, Bind>> = vec![];
                loop {
                    if i >= toks.len() {
                        break;
                    }
                    if let Some(s) = &stop {
                        if sep.as_ref().is_none_or(|x| !same_tok(x, s)) && same_tok(&toks[i], s) {
                            break;
                        }
                    }
                    let mut b = HashMap::new();
                    let Some(n) = match_prefix(inner, &toks[i..], &mut b, sep.as_ref().or(stop.as_ref())) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    i += n;
                    items.push(b);
                    if *op == '?' {
                        break;
                    }
                    match sep {
                        Some(s) => {
                            if i < toks.len() && same_tok(s, &toks[i]) {
                                i += 1;
                            } else {
                                break;
                            }
                        }
                        None => {}
                    }
                }
                if *op == '+' && items.is_empty() {
                    return None;
                }
                // 変数ごとに Many にまとめる
                let mut names = vec![];
                collect_vars(inner, &mut names);
                for n in names {
                    let v = items.iter().map(|b| b.get(&n).cloned().unwrap_or(Bind::Many(vec![]))).collect();
                    binds.insert(n, Bind::Many(v));
                }
            }
        }
    }
    Some(i)
}

fn collect_vars(p: &[Pt], out: &mut Vec<String>) {
    for x in p {
        match x {
            Pt::Var(n, _) => out.push(n.clone()),
            Pt::Group(_, i) | Pt::Rep(i, _, _) => collect_vars(i, out),
            Pt::Tok(_) => {}
        }
    }
}

fn tt_text(t: &TokenTree, out: &mut String) {
    match t {
        TokenTree::Group(g) => {
            let (o, c) = match g.delimiter() {
                Delimiter::Parenthesis => ("(", ")"),
                Delimiter::Brace => ("{", "}"),
                Delimiter::Bracket => ("[", "]"),
                Delimiter::None => ("", ""),
            };
            out.push_str(o);
            for x in g.stream() {
                tt_text(&x, out);
            }
            out.push_str(c);
            out.push(' ');
        }
        TokenTree::Punct(p) => {
            out.push(p.as_char());
            if p.spacing() == Spacing::Alone {
                out.push(' ');
            }
        }
        TokenTree::Ident(i) => {
            out.push_str(&i.to_string());
            out.push(' ');
        }
        TokenTree::Literal(l) => {
            // (1 行にするので、文字列の中の改行はエスケープに直す)
            let s = l.to_string();
            if s.starts_with('r') && s.contains('\n') {
                out.push_str(&format!("{:?}", s.trim_start_matches('r').trim_matches('#').trim_matches('"')));
            } else {
                out.push_str(&s.replace('\n', "\\n"));
            }
            out.push(' ');
        }
    }
}

fn transcribe(body: &[Pt], binds: &HashMap<String, Bind>, out: &mut String) -> Result<(), String> {
    for p in body {
        match p {
            Pt::Tok(t) => tt_text(t, out),
            Pt::Group(d, inner) => {
                let (o, c) = match d {
                    Delimiter::Parenthesis => ("(", ")"),
                    Delimiter::Brace => ("{", "}"),
                    Delimiter::Bracket => ("[", "]"),
                    Delimiter::None => ("", ""),
                };
                out.push_str(o);
                transcribe(inner, binds, out)?;
                out.push_str(c);
                out.push(' ');
            }
            Pt::Var(name, _) => match binds.get(name) {
                Some(Bind::One(ts, wrap)) => {
                    if *wrap {
                        out.push('(');
                    }
                    for t in ts {
                        tt_text(t, out);
                    }
                    if *wrap {
                        out.push(')');
                    }
                    out.push(' ');
                }
                Some(Bind::Many(_)) => return Err(format!("macro variable `${name}` is still repeating here")),
                // (`$crate` など、束縛されていない `$x` はそのまま)
                // (`$crate::m::f` は `crate::m::f`: モジュールは平らにして最後のセグメントで引く)
                None => {
                    if name == "crate" {
                        out.push_str("crate");
                    } else {
                        out.push('$');
                        out.push_str(name);
                    }
                    out.push(' ');
                }
            },
            Pt::Rep(inner, sep, _) => {
                let mut names = vec![];
                collect_vars(inner, &mut names);
                let n = names
                    .iter()
                    .filter_map(|v| match binds.get(v) {
                        Some(Bind::Many(xs)) => Some(xs.len()),
                        _ => None,
                    })
                    .max()
                    .unwrap_or(0);
                for k in 0..n {
                    if k > 0 {
                        if let Some(s) = sep {
                            tt_text(s, out);
                        }
                    }
                    let mut b = binds.clone();
                    for v in &names {
                        if let Some(Bind::Many(xs)) = binds.get(v) {
                            if let Some(x) = xs.get(k) {
                                b.insert(v.clone(), x.clone());
                            }
                        }
                    }
                    transcribe(inner, &b, out)?;
                }
            }
        }
    }
    Ok(())
}

/// ソースの中の macro_rules! を展開する。なければ None
pub fn expand(src: &str) -> Result<Option<String>, String> {
    if !src.contains("macro_rules!") {
        return Ok(None);
    }
    let mut text = src.to_string();
    let mut macros: HashMap<String, Vec<Rule>> = HashMap::new();
    // 1. 定義を集めて消す (関数の本体や `mod m { .. }` の中の定義も)
    {
        let ts = TokenStream::from_str(&text).map_err(|e| format!("1:1: {e}"))?;
        let mut cut: Vec<(usize, usize)> = vec![];
        fn collect(ts: TokenStream, macros: &mut HashMap<String, Vec<Rule>>, cut: &mut Vec<(usize, usize)>) {
            let toks: Vec<TokenTree> = ts.into_iter().collect();
            let mut i = 0;
            while i < toks.len() {
                let is_def = i + 3 < toks.len()
                    && matches!(&toks[i], TokenTree::Ident(x) if x == "macro_rules")
                    && matches!(&toks[i + 1], TokenTree::Punct(p) if p.as_char() == '!');
                if !is_def {
                    if let TokenTree::Group(g) = &toks[i] {
                        collect(g.stream(), macros, cut);
                    }
                    i += 1;
                    continue;
                }
                let (TokenTree::Ident(name), TokenTree::Group(g)) = (&toks[i + 2], &toks[i + 3]) else {
                    i += 1;
                    continue;
                };
                let mut rules = vec![];
                let body: Vec<TokenTree> = g.stream().into_iter().collect();
                let mut j = 0;
                while j < body.len() {
                    // (matcher) => {transcriber} ;
                    let TokenTree::Group(m) = &body[j] else { j += 1; continue };
                    let Some(TokenTree::Group(t)) = body.get(j + 3) else { break };
                    rules.push(Rule { pat: parse_pt(m.stream(), true), body: parse_pt(t.stream(), false) });
                    j += 4;
                    if let Some(TokenTree::Punct(p)) = body.get(j) {
                        if p.as_char() == ';' {
                            j += 1;
                        }
                    }
                }
                macros.insert(name.to_string(), rules);
                // `#[macro_export]` などの属性も消す
                let mut st = toks[i].span().byte_range().start;
                if i >= 2 {
                    if let (TokenTree::Punct(h), TokenTree::Group(a)) = (&toks[i - 2], &toks[i - 1]) {
                        if h.as_char() == '#' && a.delimiter() == Delimiter::Bracket {
                            st = h.span().byte_range().start;
                        }
                    }
                }
                let mut en = g.span().byte_range().end;
                if let Some(TokenTree::Punct(p)) = toks.get(i + 4) {
                    if p.as_char() == ';' {
                        en = p.span().byte_range().end;
                    }
                }
                cut.push((st, en));
                i += 4;
            }
        }
        collect(ts, &mut macros, &mut cut);
        let mut b = text.into_bytes();
        for (a, e) in cut {
            for x in &mut b[a..e] {
                if *x != b'\n' {
                    *x = b' ';
                }
            }
        }
        text = String::from_utf8(b).map_err(|e| e.to_string())?;
    }
    if macros.is_empty() {
        return Ok(Some(text));
    }
    // 2. 呼び出しを展開する (展開したものの中の呼び出しも)
    for _round in 0..64 {
        let ts = TokenStream::from_str(&text).map_err(|e| format!("1:1: {e}"))?;
        let mut reps: Vec<(usize, usize, String)> = vec![];
        find_calls(ts, &macros, 0, &mut reps)?;
        if reps.is_empty() {
            return Ok(Some(text));
        }
        // 後ろから置き換える (1 行に収めるので、置き換えた範囲の改行は残す)
        reps.sort_by_key(|r| std::cmp::Reverse(r.0));
        for (a, e, rep) in reps {
            let nl = text[a..e].bytes().filter(|&c| c == b'\n').count();
            let mut r = rep;
            for _ in 0..nl {
                r.push('\n');
            }
            text.replace_range(a..e, &r);
        }
    }
    Err("1:1: macro expansion is too deep (recursion limit 64)".into())
}

/// `name!(..)` / `name![..]` / `name!{..}` の呼び出し (いちばん外側だけ。中は次の周で展開する)
fn find_calls(
    ts: TokenStream,
    macros: &HashMap<String, Vec<Rule>>,
    depth: usize,
    out: &mut Vec<(usize, usize, String)>,
) -> Result<(), String> {
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    let mut i = 0;
    while i < toks.len() {
        if i + 2 < toks.len() {
            if let (TokenTree::Ident(name), TokenTree::Punct(p), TokenTree::Group(g)) = (&toks[i], &toks[i + 1], &toks[i + 2]) {
                if p.as_char() == '!' {
                    if let Some(rules) = macros.get(&name.to_string()) {
                        let args: Vec<TokenTree> = g.stream().into_iter().collect();
                        let mut done = false;
                        for r in rules {
                            let mut binds = HashMap::new();
                            if match_pat(&r.pat, &args, &mut binds) {
                                let mut body = String::new();
                                transcribe(&r.body, &binds, &mut body)?;
                                // 式の位置は `({ .. })`、トップレベルはアイテムとしてそのまま
                                let rep = if depth == 0 { body } else { format!("({{ {body} }})") };
                                let mut a = name.span().byte_range().start;
                                // (`crate::m!(..)` / `self::m!(..)`: 前のパスも消す)
                                let mut k = i;
                                while k >= 3 {
                                    match (&toks[k - 3], &toks[k - 2], &toks[k - 1]) {
                                        (TokenTree::Ident(seg), TokenTree::Punct(c1), TokenTree::Punct(c2))
                                            if c1.as_char() == ':' && c2.as_char() == ':' =>
                                        {
                                            a = seg.span().byte_range().start;
                                            k -= 3;
                                        }
                                        _ => break,
                                    }
                                }
                                let mut e = g.span().byte_range().end;
                                // (トップレベルの `name!(..);` の `;` も取る)
                                if depth == 0 {
                                    if let Some(TokenTree::Punct(s)) = toks.get(i + 3) {
                                        if s.as_char() == ';' {
                                            e = s.span().byte_range().end;
                                        }
                                    }
                                }
                                out.push((a, e, rep));
                                done = true;
                                break;
                            }
                        }
                        if !done {
                            let lc = name.span().start();
                            return Err(format!("{}:{}: no rules of `{name}!` matched this call", lc.line, lc.column + 1));
                        }
                        i += 3;
                        continue;
                    }
                }
            }
        }
        if let TokenTree::Group(g) = &toks[i] {
            find_calls(g.stream(), macros, depth + 1, out)?;
        }
        i += 1;
    }
    Ok(())
}
