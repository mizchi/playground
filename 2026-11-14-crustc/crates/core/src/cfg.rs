//! 条件付きコンパイル: `#[cfg(..)]` の付いたもの (アイテム・フィールド・バリアント・match の腕・文・式) を
//! 述語が偽なら空白にする。`#[cfg_attr(p, a)]` は p が真なら `#[a]`、偽なら消す。`cfg!(p)` は `true` / `false` にする。
//! ついでに `extern crate x;` (std / core / alloc と、ワークスペースの依存) も消す。
//! バイト位置と行は変えない (空白で塗る) ので、エラーと panic の場所はそのまま。
//!
//! 述語: `feature = ".."` (クレートごとの集合)・`debug_assertions` (-O でなければ真)・`unix` / `target_os = "linux"` など
//! x86-64 Linux の値・`test` / `doc` / `miri` は偽・`not` / `all` / `any`。知らない名前は偽

/// cfg の述語を評価するための設定
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CfgSet {
    pub features: Vec<String>,
    pub debug_assertions: bool,
}

impl CfgSet {
    fn flag(&self, name: &str) -> bool {
        match name {
            "unix" => true,
            "debug_assertions" => self.debug_assertions,
            "overflow_checks" => self.debug_assertions,
            _ => false,
        }
    }

    fn kv(&self, key: &str, val: &str) -> bool {
        match key {
            "feature" => self.features.iter().any(|f| f == val),
            "target_os" => val == "linux",
            "target_family" => val == "unix",
            "target_arch" => val == "x86_64",
            "target_pointer_width" => val == "64",
            "target_endian" => val == "little",
            "target_env" => val == "gnu",
            "target_vendor" => val == "unknown",
            "panic" => val == "unwind",
            "target_has_atomic" => val == "8" || val == "16" || val == "32" || val == "64" || val == "ptr",
            "target_feature" => val == "sse" || val == "sse2" || val == "fxsr",
            _ => false,
        }
    }
}

/// 字句: (種類, 開始, 終了)。種類 0 = 識別子, 1 = 記号 1 文字, 2 = リテラル
type Tok = (u8, usize, usize);

/// 述語 toks[i..] を評価し、(値, 次の位置)
fn eval(s: &[u8], toks: &[Tok], mut i: usize, c: &CfgSet) -> (bool, usize) {
    let t = |k: usize| -> &[u8] {
        match toks.get(k) {
            Some(&(_, a, b)) => &s[a..b],
            None => b"",
        }
    };
    let name: &[u8] = t(i);
    i += 1;
    if t(i) == b"(" && (name == b"not" || name == b"all" || name == b"any") {
        i += 1;
        let mut vals: Vec<bool> = vec![];
        while i < toks.len() && t(i) != b")" {
            let (v, j) = eval(s, toks, i, c);
            vals.push(v);
            i = j;
            if t(i) == b"," {
                i += 1;
            }
        }
        i += 1;
        let v = match name {
            b"not" => !vals.first().copied().unwrap_or(false),
            b"all" => vals.iter().all(|&v| v),
            _ => vals.iter().any(|&v| v),
        };
        return (v, i);
    }
    let name = String::from_utf8_lossy(name).into_owned();
    if t(i) == b"=" && toks.get(i + 1).is_some_and(|x| x.0 == 2) {
        let lit = t(i + 1);
        let val = String::from_utf8_lossy(&lit[1..lit.len() - 1]).into_owned();
        return (c.kv(&name, &val), i + 2);
    }
    (c.flag(&name), i)
}

fn is_open(b: &[u8]) -> bool {
    b == b"(" || b == b"[" || b == b"{"
}

fn is_close(b: &[u8]) -> bool {
    b == b")" || b == b"]" || b == b"}"
}

/// 対応する閉じ括弧の位置
fn matching(s: &[u8], toks: &[Tok], i: usize) -> usize {
    let mut d = 0i32;
    let mut k = i;
    while k < toks.len() {
        let b = &s[toks[k].1..toks[k].2];
        if toks[k].0 == 1 && is_open(b) {
            d += 1;
        } else if toks[k].0 == 1 && is_close(b) {
            d -= 1;
            if d == 0 {
                return k;
            }
        }
        k += 1;
    }
    toks.len() - 1
}

fn blank(out: &mut [u8], a: usize, b: usize) {
    for k in a..b {
        if out[k] != b'\n' {
            out[k] = b' ';
        }
    }
}

/// 属性の後ろの「もの」の終わり (その最後の字句の位置)。i は対象の最初の字句
fn thing_end(s: &[u8], toks: &[Tok], mut i: usize) -> usize {
    let t = |k: usize| -> &[u8] {
        match toks.get(k) {
            Some(&(_, a, b)) => &s[a..b],
            None => b"",
        }
    };
    // (ほかの属性を飛ばす)
    while t(i) == b"#" && t(i + 1) == b"[" {
        i = matching(s, toks, i + 1) + 1;
    }
    // (`;` まで続くもの: `const X: T = S { .. };`・`use a::{b, c};`・`let`・`type`・`static`)
    let mut j = i;
    while matches!(t(j), b"pub" | b"unsafe" | b"async" | b"extern") || t(j).first() == Some(&b'"') {
        if t(j) == b"pub" && t(j + 1) == b"(" {
            j = matching(s, toks, j + 1);
        }
        j += 1;
    }
    let semi_item = matches!(t(j), b"const" | b"static" | b"use" | b"let" | b"type") && t(j + 1) != b"fn";
    let mut k = i;
    while k < toks.len() {
        let b = t(k);
        if toks[k].0 == 1 && is_close(b) {
            // (包んでいるものの終わり: その手前まで)
            return k - 1;
        }
        if toks[k].0 == 1 && (b == b"," || b == b";") {
            return k;
        }
        if toks[k].0 == 1 && is_open(b) {
            let m = matching(s, toks, k);
            if b == b"{" && !semi_item {
                // (ブロックで終わるもの。後ろの `,` も消す: バリアント・match の腕)
                return if t(m + 1) == b"," { m + 1 } else { m };
            }
            k = m + 1;
            continue;
        }
        k += 1;
    }
    toks.len() - 1
}

/// src の cfg を評価する。変わらなければ None
pub fn strip(src: &str, c: &CfgSet) -> Option<String> {
    if !src.contains("cfg") && !src.contains("extern crate") {
        return None;
    }
    let s = src.as_bytes();
    let toks = crate::lexer::tokenize(s);
    let mut out = s.to_vec();
    let mut changed = false;
    let t = |k: usize| -> &[u8] {
        match toks.get(k) {
            Some(&(_, a, b)) => &s[a..b],
            None => b"",
        }
    };
    let mut i = 0;
    while i < toks.len() {
        // `cfg!(p)` → `true` / `false`
        if t(i) == b"cfg" && t(i + 1) == b"!" && t(i + 2) == b"(" {
            let close = matching(s, &toks, i + 2);
            let (v, _) = eval(s, &toks, i + 3, c);
            let (a, b) = (toks[i].1, toks[close].2);
            blank(&mut out, a, b);
            let w: &[u8] = if v { b"true" } else { b"false" };
            if b - a >= w.len() {
                out[a..a + w.len()].copy_from_slice(w);
            }
            changed = true;
            i = close + 1;
            continue;
        }
        // `extern crate x [as y];`
        if t(i) == b"extern" && t(i + 1) == b"crate" && toks.get(i + 2).is_some_and(|x| x.0 == 0) {
            let mut e = i + 3;
            while e < toks.len() && t(e) != b";" && e < i + 6 {
                e += 1;
            }
            if t(e) == b";" {
                blank(&mut out, toks[i].1, toks[e].2);
                changed = true;
                i = e + 1;
                continue;
            }
        }
        if t(i) != b"#" {
            i += 1;
            continue;
        }
        let inner = t(i + 1) == b"!";
        let lb = if inner { i + 2 } else { i + 1 };
        if t(lb) != b"[" {
            i += 1;
            continue;
        }
        let rb = matching(s, &toks, lb);
        let name = t(lb + 1);
        if (name == b"cfg" || name == b"cfg_attr") && t(lb + 2) == b"(" {
            let pclose = matching(s, &toks, lb + 2);
            let (v, after) = eval(s, &toks, lb + 3, c);
            changed = true;
            if name == b"cfg_attr" {
                if v && t(after) == b"," {
                    // `#[cfg_attr(p, a, b)]` → `#[         a, b ]`
                    blank(&mut out, toks[lb + 1].1, toks[after].2);
                    blank(&mut out, toks[pclose].1, toks[pclose].2);
                    i = after + 1;
                    continue;
                }
                blank(&mut out, toks[i].1, toks[rb].2);
                i = rb + 1;
                continue;
            }
            if inner {
                // (`#![cfg(..)]` が偽ならファイル全体。今は属性だけ消す)
                blank(&mut out, toks[i].1, toks[rb].2);
                if !v {
                    blank(&mut out, 0, s.len());
                    return String::from_utf8(out).ok();
                }
                i = rb + 1;
                continue;
            }
            if v {
                blank(&mut out, toks[i].1, toks[rb].2);
                i = rb + 1;
            } else {
                let e = thing_end(s, &toks, rb + 1);
                blank(&mut out, toks[i].1, toks[e.max(rb)].2);
                i = e.max(rb) + 1;
            }
            continue;
        }
        i += 1;
    }
    if changed { String::from_utf8(out).ok() } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str, feats: &[&str]) -> String {
        let c = CfgSet { features: feats.iter().map(|s| s.to_string()).collect(), debug_assertions: true };
        let o = strip(src, &c).unwrap_or_else(|| src.to_string());
        assert_eq!(o.len(), src.len());
        o.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn items_fields_arms() {
        assert_eq!(run("#[cfg(feature = \"std\")] fn a() {} fn b() {}", &[]), "fn b() {}");
        assert_eq!(run("#[cfg(feature = \"std\")] fn a() {} fn b() {}", &["std"]), "fn a() {} fn b() {}");
        assert_eq!(run("struct S { #[cfg(test)] a: u8, b: u8 }", &[]), "struct S { b: u8 }");
        assert_eq!(run("enum E { #[cfg(not(unix))] A { x: u8 }, B }", &[]), "enum E { B }");
        assert_eq!(run("match x { #[cfg(any(test, doc))] 1 => {} _ => 2 }", &[]), "match x { _ => 2 }");
        assert_eq!(run("#[cfg(test)] const X: S = S { a: 1 }; fn f() {}", &[]), "fn f() {}");
        assert_eq!(run("#[cfg(test)] use a::{b, c}; fn f() {}", &[]), "fn f() {}");
        assert_eq!(run("#[cfg_attr(feature = \"std\", derive(Debug))] struct S;", &["std"]), "#[ derive(Debug) ] struct S;");
        assert_eq!(run("#![cfg_attr(not(feature = \"std\"), no_std)] fn f() {}", &["std"]), "fn f() {}");
        assert_eq!(run("let a = cfg!(all(unix, target_pointer_width = \"64\"));", &[]), "let a = true ;");
        assert_eq!(run("fn f() { #[cfg(test)] g(); h(); }", &[]), "fn f() { h(); }");
        assert_eq!(run("[1, #[cfg(test)] 2, 3]", &[]), "[1, 3]");
        assert_eq!(run("#[cfg(feature = \"alloc\")] extern crate alloc; extern crate std as s; fn f() {}", &["alloc"]), "fn f() {}");
    }
}
