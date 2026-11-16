//! 依存するクレートのソースを、使う側のクレートに渡す形 (インターフェース) にする。
//!
//! ワークスペースでは、使う側のクレートは依存するクレートのソースも一緒に解析する (型・シグネチャ・総称関数の本体が要る)。
//! 総称でない関数の本体は依存するクレート自身がコンパイルして .o に入れるので、使う側では要らない: `{ loop {} }` にする
//! (改行は残して行の対応を保つ)。マクロの展開・モジュールを平らにする・字句解析・要約・Program が、依存の本体の分だけ軽くなる。
//!
//! 本体を残すもの: 総称関数 (`fn f<T>` / 引数の `impl Trait` / `where`)、総称な impl (`impl<T> ..`) と trait の中
//! (既定のメソッドは使う側の型で単相化する)、`const fn`、戻り値が `impl Trait` のもの (本体から型を決める)、`async fn`

type Tok = (u8, usize, usize);

/// 括弧の対応 (開き → 閉じ)。対応しないものは usize::MAX
fn matching(s: &[u8], toks: &[Tok]) -> Vec<usize> {
    let mut close = vec![usize::MAX; toks.len()];
    let mut st: Vec<usize> = vec![];
    for (k, t) in toks.iter().enumerate() {
        if t.0 != 1 {
            continue;
        }
        match s[t.1] {
            b'(' | b'[' | b'{' => st.push(k),
            b')' | b']' | b'}' => {
                if let Some(o) = st.pop() {
                    close[o] = k;
                }
            }
            _ => {}
        }
    }
    close
}

/// 本体を `{ loop {} }` にしたソース。変えるものがなければ元のまま
pub fn interface_text(src: &str) -> String {
    let s = src.as_bytes();
    let toks = crate::lexer::tokenize(s);
    let close = matching(s, &toks);
    let n = toks.len();
    let t = |k: usize| -> &[u8] {
        if k < n {
            &s[toks[k].1..toks[k].2]
        } else {
            b""
        }
    };
    let id = |k: usize, w: &[u8]| k < n && toks[k].0 == 0 && t(k) == w;
    let sym = |k: usize, c: u8| k < n && toks[k].0 == 1 && s[toks[k].1] == c;
    // (本体を消す範囲: `{` の後ろから `}` の前まで)
    let mut cut: Vec<(usize, usize)> = vec![];
    // 開いている中括弧ごとの文脈: 0 = モジュール (トップレベル)、1 = 総称でない impl、2 = そのほか (中は見ない)
    let mut ctx: Vec<(usize, u8)> = vec![];
    let mut pending: u8 = 2;
    let mut k = 0;
    while k < n {
        // (閉じた文脈を外す)
        while let Some(&(e, _)) = ctx.last() {
            if k > e {
                ctx.pop();
            } else {
                break;
            }
        }
        let cur = ctx.last().map(|c| c.1).unwrap_or(0);
        if sym(k, b'{') {
            let e = close[k];
            if e == usize::MAX {
                break;
            }
            let c = pending;
            pending = 2;
            if c == 2 {
                // (中は見ない: struct の本体・総称な impl・trait・関数の本体など)
                k = e + 1;
                continue;
            }
            ctx.push((e, c));
            k += 1;
            continue;
        }
        if cur == 2 {
            k += 1;
            continue;
        }
        if id(k, b"mod") && k + 2 < n && toks[k + 1].0 == 0 && sym(k + 2, b'{') {
            pending = 0;
            k += 2;
            continue;
        }
        if id(k, b"impl") && cur == 0 {
            // (`impl<..>` は総称: 中は見ない)
            pending = if sym(k + 1, b'<') { 2 } else { 1 };
            k += 1;
            continue;
        }
        if id(k, b"fn") && toks.get(k + 1).is_some_and(|x| x.0 == 0) {
            // (前の修飾子: const fn / async fn は残す)
            let mut j = k;
            let mut keep = false;
            while j > 0 {
                j -= 1;
                let w = t(j);
                if toks[j].0 == 0 && (w == b"const" || w == b"async") {
                    keep = true;
                    break;
                }
                if !(toks[j].0 == 0 && (w == b"pub" || w == b"unsafe" || w == b"extern" || w == b"crate" || w == b"default")) && toks[j].0 != 2 && !sym(j, b'(') && !sym(j, b')') {
                    break;
                }
            }
            let mut i = k + 2;
            if sym(i, b'<') {
                keep = true;
            }
            // (引数の並び)
            while i < n && !sym(i, b'(') && !sym(i, b'{') && !sym(i, b';') {
                i += 1;
            }
            if !sym(i, b'(') || close[i] == usize::MAX {
                k = i;
                continue;
            }
            let pe = close[i];
            if (i..pe).any(|q| id(q, b"impl")) {
                keep = true;
            }
            // (戻り値と where: 本体の `{` か宣言の `;` まで)
            let mut q = pe + 1;
            while q < n && !sym(q, b'{') && !sym(q, b';') {
                if id(q, b"impl") || id(q, b"where") {
                    keep = true;
                }
                if sym(q, b'(') || sym(q, b'[') {
                    if close[q] == usize::MAX {
                        break;
                    }
                    q = close[q];
                }
                q += 1;
            }
            if !sym(q, b'{') || close[q] == usize::MAX {
                k = q + 1;
                continue;
            }
            let be = close[q];
            if !keep && be > q + 1 {
                cut.push((toks[q].2, toks[be].1));
            }
            k = be + 1;
            continue;
        }
        k += 1;
    }
    if cut.is_empty() {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len());
    let mut last = 0;
    for (a, z) in cut {
        out.push_str(&src[last..a]);
        out.push_str(" loop {} ");
        for &c in &s[a..z] {
            if c == b'\n' {
                out.push('\n');
            }
        }
        last = z;
    }
    out.push_str(&src[last..]);
    out
}

/// モジュールの中のアイテム (依存するクレートのインターフェースを、使う側が名前でたどるため)
pub struct StubItem {
    /// 定義する名前 (impl はヘッダの名前すべて: 型か trait が届けば impl も届く)
    pub names: Vec<String>,
    /// アイテムのテキストのハッシュ
    pub hash: crate::key::Key,
    /// 中の識別子すべて
    pub idents: Vec<String>,
}

/// src (クレートのテキスト) のアイテム。`mod m { .. }` の中も (mod 自体はアイテムにしない)
pub fn items(src: &str) -> Vec<StubItem> {
    let s = src.as_bytes();
    let toks = crate::lexer::tokenize(s);
    let close = matching(s, &toks);
    let n = toks.len();
    let t = |k: usize| -> &[u8] {
        if k < n {
            &s[toks[k].1..toks[k].2]
        } else {
            b""
        }
    };
    let id = |k: usize, w: &[u8]| k < n && toks[k].0 == 0 && t(k) == w;
    let sym = |k: usize, c: u8| k < n && toks[k].0 == 1 && s[toks[k].1] == c;
    let mut out: Vec<StubItem> = vec![];
    let mut k = 0;
    while k < n {
        // (モジュールの閉じ括弧)
        if sym(k, b'}') || sym(k, b';') {
            k += 1;
            continue;
        }
        // `[pub] mod m {`: 中を見る
        let mut j = k;
        while sym(j, b'#') && sym(j + 1, b'[') && close[j + 1] != usize::MAX {
            j = close[j + 1] + 1;
        }
        if id(j, b"pub") {
            j += 1;
            if sym(j, b'(') && close[j] != usize::MAX {
                j = close[j] + 1;
            }
        }
        if id(j, b"mod") && toks.get(j + 1).is_some_and(|x| x.0 == 0) && sym(j + 2, b'{') {
            k = j + 3;
            continue;
        }
        // アイテムの終わり: `;` か、本体の `}` (const / static / use / type は `;` まで)
        let semi_kind = (j..n.min(j + 4)).any(|q| id(q, b"const") && !id(q + 1, b"fn") || id(q, b"static") || id(q, b"use") || id(q, b"type"));
        let mut e = j;
        while e < n {
            if sym(e, b';') {
                break;
            }
            if (sym(e, b'(') || sym(e, b'[') || sym(e, b'{')) && close[e] != usize::MAX {
                let c = close[e];
                if sym(e, b'{') && !semi_kind {
                    e = c;
                    if sym(e + 1, b';') {
                        e += 1;
                    }
                    break;
                }
                e = c + 1;
                continue;
            }
            e += 1;
        }
        let e = e.min(n - 1);
        let mut idents: Vec<String> = vec![];
        let mut q = k;
        while q <= e {
            if toks[q].0 == 0 {
                let w = String::from_utf8_lossy(t(q)).into_owned();
                if !idents.contains(&w) {
                    idents.push(w);
                }
            }
            q += 1;
        }
        let mut names: Vec<String> = vec![];
        let mut q = j;
        while q < e {
            let w = t(q);
            if toks[q].0 == 0 && matches!(w, b"fn" | b"struct" | b"enum" | b"union" | b"trait" | b"type" | b"const" | b"static") && toks.get(q + 1).is_some_and(|x| x.0 == 0) {
                let nm = if w == b"static" && id(q + 1, b"mut") { q + 2 } else { q + 1 };
                if !(w == b"const" && id(nm, b"fn")) {
                    names.push(String::from_utf8_lossy(t(nm)).into_owned());
                    break;
                }
            }
            if toks[q].0 == 0 && (w == b"impl" || w == b"use") {
                // (impl / use はヘッダ (use は全体) の名前すべて)
                let mut r = q + 1;
                while r <= e && !sym(r, b'{') || (w == b"use" && r <= e) {
                    if toks[r].0 == 0 {
                        names.push(String::from_utf8_lossy(t(r)).into_owned());
                    }
                    r += 1;
                }
                break;
            }
            q += 1;
        }
        if names.is_empty() {
            names = idents.clone();
        }
        let mut h = crate::key::KeyHasher::new();
        h.add(&src[toks[k].1..toks[e].2]);
        out.push(StubItem { names, hash: h.finish(), idents });
        k = e + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn stubs_concrete_bodies() {
        let src = "pub mod m {\n pub fn a(x: u32) -> u32 {\n x + 1\n }\n pub fn g<T: Copy>(x: T) -> T { x }\n pub fn h(x: impl Fn()) { x() }\n pub const fn c() -> u8 { 3 }\n pub fn o() -> impl Iterator<Item = u8> { 0..3 }\n struct S;\n impl S { fn m(&self) -> u8 { 1 } }\n impl<T> W<T> { fn n(&self) { body() } }\n trait Tr { fn d(&self) -> u8 { 2 } }\n fn w<T>(t: T) where T: Copy { t; }\n mod inner { fn z() { q() } }\n}\n";
        let o = interface_text(src);
        assert_eq!(o.lines().count(), src.lines().count());
        let its = items(&o);
        let names: Vec<String> = its.iter().map(|i| i.names.join("+")).collect();
        assert_eq!(names, ["a", "g", "h", "c", "o", "S", "S", "T+W+T", "Tr", "w", "z"]);
        assert_eq!(
            norm(&o),
            norm("pub mod m { pub fn a(x: u32) -> u32 { loop {} } pub fn g<T: Copy>(x: T) -> T { x } pub fn h(x: impl Fn()) { x() } pub const fn c() -> u8 { 3 } pub fn o() -> impl Iterator<Item = u8> { 0..3 } struct S; impl S { fn m(&self) -> u8 { loop {} } } impl<T> W<T> { fn n(&self) { body() } } trait Tr { fn d(&self) -> u8 { 2 } } fn w<T>(t: T) where T: Copy { t; } mod inner { fn z() { loop {} } } }")
        );
    }
}
