//! 前処理 (thread_local! の展開・マクロの展開・`mod` を平らにする・入れ子のアイテムの移動・コンストラクタの書き換え) を、
//! 編集をまたいでチャンクごとに覚えて差分で作る (LSP の didChange で、変わっていない所を作り直さない)。
//!
//! - チャンク: アイテムの境界 (`mod` / `impl` / `trait` の中も) で切った行の範囲。境界は、`}` / `;` / 開き `{` の後で、
//!   空白だけの行頭から始まるトークンの前。
//! - 前処理はどれも、ファイル全体から集めたもの (マクロの定義・タプル struct とバリアントの要素数・モジュールの木と `use`・
//!   入れ子のアイテムの名前) を使ってトークンごとに書き換え、行の数を変えない (移したアイテムは末尾に足す)。
//!   集めるものはアイテムの宣言にしか依らないので、関数の本体の中の式を空白にした「骨組み」(入れ子のアイテム・`use`・
//!   `macro_rules!`・ユーザのマクロの呼び出しと、括弧と `;` は残す。行の数は同じ) から同じものが集まる。
//! - 骨組みの全体が前回と同じなら、変わっていないチャンクの結果は前回のものを使い、変わったチャンクだけを、ほかを骨組みに
//!   したソースで前処理して行で切り出す。骨組みが変わったら (宣言を変えた) 全体を前処理し直す。
//! - `CRUSTC_PREP_VERIFY=1` で毎回全体の前処理と比べる。`CRUSTC_NO_PREP_INC=1` で差分を使わない。

use crate::fxhash::FxMap;
use crate::lexer;

pub type R<T> = Result<T, String>;

/// 前処理の結果
pub struct Prep {
    pub text: String,
    /// 入れ子のアイテムを末尾に移したとき: 移したアイテムの (入力での開始行 (0 始まり), 改行の数)
    pub moved: Option<Vec<(usize, usize)>>,
}

fn count_nl(b: &[u8]) -> usize {
    let mut n = 0;
    for &c in b {
        if c == b'\n' {
            n += 1;
        }
    }
    n
}

fn fnv(h: u64, b: &[u8]) -> u64 {
    let mut h = h;
    for &c in b {
        h = (h ^ c as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

const FNV0: u64 = 0xcbf2_9ce4_8422_2325;

fn mix(h: u64, x: u64) -> u64 {
    let h = (h ^ x).wrapping_mul(0x0100_0000_01b3);
    h ^ (h >> 29)
}

/// 全体を前処理する (with_prelude_in のもともとの順)
pub fn prep_full(src: &str, crates: &[String], fix: &mut Vec<(u32, u32, i32)>, cfg: Option<&crate::cfg::CfgSet>) -> R<Prep> {
    let tl = lexer::expand_thread_local(src);
    let s1: &str = match &tl {
        Some(t) => t.as_str(),
        None => src,
    };
    let expanded = crate::macros::expand_cfg(s1, cfg)?;
    let s2: &str = match &expanded {
        Some(t) => t.as_str(),
        None => s1,
    };
    if let Some(p) = std::env::var_os("CRUSTC_DUMP_PRE") {
        let _ = std::fs::write(p, s2);
    }
    let flat = lexer::flatten_modules_fix(s2, crates, fix);
    let s3: &str = match &flat {
        Some(t) => t.as_str(),
        None => s2,
    };
    let mut moved: Option<Vec<(usize, usize)>> = None;
    let hoisted = match lexer::hoist_nested_items_parts(s3) {
        Some((body, parts)) => {
            if parts.is_empty() {
                Some(body)
            } else {
                // (移したものの開始行: 本文は入力と同じ行の並び)
                let bb = body.as_bytes();
                let mut info = vec![];
                let mut out = body.clone().into_bytes();
                let (mut at, mut line) = (0usize, 0usize);
                for (pos, t) in &parts {
                    line += count_nl(&bb[at..*pos]);
                    at = *pos;
                    info.push((line, count_nl(t.as_bytes())));
                    out.push(b'\n');
                    out.extend_from_slice(t.as_bytes());
                }
                out.push(b'\n');
                moved = Some(info);
                Some(String::from_utf8(out).map_err(|e| e.to_string())?)
            }
        }
        None => None,
    };
    let s4: &str = match &hoisted {
        Some(t) => t.as_str(),
        None => s3,
    };
    let cv = crate::ctors::rewrite_ctor_values(s4);
    let text = match cv {
        Some(t) => t,
        None => s4.to_string(),
    };
    Ok(Prep { text, moved })
}

/// チャンクの結果: 本文の行 (改行込み)、末尾に移したアイテム、桁のずれ (行はチャンクの先頭からの 0 始まり)
struct ChunkOut {
    text: String,
    pieces: Vec<String>,
    fix: Vec<(u32, u32, i32)>,
}

#[derive(Default)]
pub struct PrepMemo {
    /// 呼ばれた回数 (1 回目は全体を前処理するだけ: 1 度しか前処理しないビルドでチャンクの記録を作らない)
    calls: u32,
    /// crates と cfg
    key: u64,
    /// 骨組みの全体のハッシュ
    fp: u64,
    /// チャンクのハッシュ → その中で定義したマクロの名前
    names: FxMap<u64, Vec<String>>,
    /// (チャンクのハッシュ, マクロの名前の集合のハッシュ) → (骨組み, そのハッシュ)
    skel: FxMap<(u64, u64), (String, u64)>,
    /// 前回の入力とチャンクの開始位置 (位置, 囲むもののハッシュ, 階層)
    chunks: Option<(String, Vec<(usize, u64, Levels)>, bool)>,
    /// (チャンクのハッシュ, 囲むもの (mod / impl / trait) のハッシュ, 同じものの何番目か) → 結果
    out: FxMap<(u64, u64, u32), ChunkOut>,
}

/// チャンクの開始位置 (byte) と、囲むもの (mod の名前の列・impl / trait) のハッシュ
type Levels = Vec<(i32, u64)>;

/// src[from..to] (from はチャンクの境界、levels はそこでの階層) のチャンクの開始位置 (from を含む) と、終わりの階層。
/// 終わりがアイテムの間でなければ (と、コメント・文字列が開いたままかもしれなければ) 終わりの階層は None
fn chunk_range(src: &str, from: usize, to: usize, init: Levels) -> (Vec<(usize, u64, Levels)>, Option<Levels>) {
    let s = &src.as_bytes()[..to];
    let toks0 = lexer::tokenize(&s[from..]);
    let toks: Vec<(u8, usize, usize)> = toks0.iter().map(|t| (t.0, t.1 + from, t.2 + from)).collect();
    // (階層: (中の波括弧の深さ, 囲むもののハッシュ))
    let mut levels: Levels = init;
    let first_ctx = levels[levels.len() - 1].1;
    let mut out: Vec<(usize, u64, Levels)> = vec![(from, first_ctx, levels.clone())];
    let mut depth = levels[levels.len() - 1].0;
    let mut pd = 0i32;
    // アイテムの間にいるか、直前のトークンが `}` / `;` / 階層を開く `{` か
    let mut at_b = true;
    let mut prev_ok = true;
    // アイテムの先頭の語 (pub / unsafe / async を除く)、mod の名前、fn を見たか
    let mut head: Vec<u8> = vec![];
    let mut mod_name: Vec<u8> = vec![];
    let mut want_name = false;
    let mut saw_fn = false;
    let mut k = 0;
    while k < toks.len() {
        let (kind, a, b) = toks[k];
        let lvl = levels[levels.len() - 1];
        if at_b && prev_ok && pd == 0 && depth == lvl.0 && k > 0 {
            // (行頭からこのトークンまで空白だけなら、行頭が境界)
            let mut ls = a;
            while ls > 0 && s[ls - 1] != b'\n' {
                ls -= 1;
            }
            let mut blank = true;
            for &c in &s[ls..a] {
                if c != b' ' && c != b'\t' && c != b'\r' {
                    blank = false;
                }
            }
            if blank && ls > out[out.len() - 1].0 {
                out.push((ls, lvl.1, levels.clone()));
            }
        }
        if kind == 1 {
            let c = s[a];
            if c == b'{' {
                let hd: &[u8] = &head;
                let opens = pd == 0 && depth == lvl.0 && (hd == b"mod" || hd == b"impl" || hd == b"trait" || (hd == b"extern" && !saw_fn));
                depth += 1;
                if opens {
                    let mut h = mix(lvl.1, head.len() as u64);
                    h = fnv(h, &head);
                    if hd == b"mod" {
                        h = fnv(mix(h, 7), &mod_name);
                    }
                    levels.push((depth, h));
                    at_b = true;
                    prev_ok = true;
                    head.clear();
                    saw_fn = false;
                } else {
                    at_b = false;
                    prev_ok = false;
                }
            } else if c == b'}' {
                depth -= 1;
                if depth < lvl.0 && levels.len() > 1 {
                    levels.pop();
                    at_b = true;
                    prev_ok = true;
                    head.clear();
                    saw_fn = false;
                } else if depth == lvl.0 && pd == 0 {
                    at_b = true;
                    prev_ok = true;
                    head.clear();
                    saw_fn = false;
                } else {
                    prev_ok = false;
                }
            } else if c == b'(' || c == b'[' {
                pd += 1;
                at_b = false;
                prev_ok = false;
            } else if c == b')' || c == b']' {
                pd -= 1;
                prev_ok = false;
            } else if c == b';' {
                if depth == lvl.0 && pd == 0 {
                    at_b = true;
                    prev_ok = true;
                    head.clear();
                    saw_fn = false;
                } else {
                    prev_ok = false;
                }
            } else {
                at_b = false;
                prev_ok = false;
            }
        } else {
            if kind == 0 {
                let w = &s[a..b];
                if want_name {
                    mod_name = w.to_vec();
                    want_name = false;
                }
                if pd == 0 && depth == lvl.0 && head.is_empty() && w != b"pub" && w != b"unsafe" && w != b"async" {
                    head = w.to_vec();
                    if w == b"mod" {
                        want_name = true;
                    }
                }
                if w == b"fn" {
                    saw_fn = true;
                }
            }
            at_b = false;
            prev_ok = false;
        }
        k += 1;
    }
    let lvl = levels[levels.len() - 1];
    let last_end = match toks.last() {
        Some(t) => t.2,
        None => from,
    };
    let mut clean = at_b && prev_ok && pd == 0 && depth == lvl.0;
    // (最後のトークンの後ろは空白と `//` のコメントだけ)
    let g = &s[last_end..];
    let mut i = 0;
    while i < g.len() && clean {
        if g[i].is_ascii_whitespace() {
            i += 1;
        } else if g[i] == b'/' && i + 1 < g.len() && g[i + 1] == b'/' {
            while i < g.len() && g[i] != b'\n' {
                i += 1;
            }
        } else {
            clean = false;
        }
    }
    (out, if clean { Some(levels) } else { None })
}

/// チャンクの開始位置と、囲むもののハッシュ (前回の結果があれば、変わった所の周りだけ切り直す)
/// 2 つ目は、全体の括弧が釣り合っているか (釣り合っていないソースでは差分を使わない)
fn chunk_starts(src: &str, memo: &mut Option<(String, Vec<(usize, u64, Levels)>, bool)>) -> (Vec<(usize, u64)>, bool) {
    let r = match memo.as_ref() {
        Some((old, st, bal)) => chunk_diff(old, st, src).map(|v| (v, *bal)),
        None => None,
    };
    let (st, bal) = match r {
        Some(v) => v,
        None => {
            let (v, end) = chunk_range(src, 0, src.len(), vec![(0, FNV0)]);
            let bal = end == Some(vec![(0, FNV0)]);
            (v, bal)
        }
    };
    let out = st.iter().map(|x| (x.0, x.1)).collect();
    *memo = Some((src.to_string(), st, bal));
    (out, bal)
}

fn chunk_diff(old: &str, st: &[(usize, u64, Levels)], new: &str) -> Option<Vec<(usize, u64, Levels)>> {
    let (p, n) = (old.as_bytes(), new.as_bytes());
    if p == n {
        return Some(st.to_vec());
    }
    let lim = p.len().min(n.len());
    let mut pre = 0;
    while pre < lim && p[pre] == n[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < lim - pre && p[p.len() - 1 - suf] == n[n.len() - 1 - suf] {
        suf += 1;
    }
    // 前: 次の境界が変わった所より前のチャンク
    let mut j = 0;
    while j + 1 < st.len() && st[j + 1].0 <= pre {
        j += 1;
    }
    let a = st[j].0;
    // 後ろ: 変わった所より後から始まる最初の境界
    let chg_end = p.len() - suf;
    let m = st.iter().position(|x| x.0 > chg_end && x.0 > a);
    let (b_old, tail) = match m {
        Some(m) => (st[m].0, m),
        None => (p.len(), st.len()),
    };
    let b_new = (b_old + n.len()).checked_sub(p.len())?;
    if b_new < a {
        return None;
    }
    let (mid, end) = chunk_range(new, a, b_new, st[j].2.clone());
    if tail < st.len() {
        // (切り直した所の終わりが、後ろの最初のチャンクの始まりと同じ階層でなければ使わない)
        if end.as_ref() != Some(&st[tail].2) {
            return None;
        }
    }
    let mut out: Vec<(usize, u64, Levels)> = st[..j].to_vec();
    out.extend(mid);
    let d = n.len() as i64 - p.len() as i64;
    for x in &st[tail..] {
        out.push(((x.0 as i64 + d) as usize, x.1, x.2.clone()));
    }
    Some(out)
}

/// チャンクの中で定義したマクロの名前
fn macro_names(t: &str) -> Vec<String> {
    let mut v = vec![];
    if !t.contains("macro_rules") {
        return v;
    }
    let s = t.as_bytes();
    let toks = lexer::tokenize(s);
    let mut k = 0;
    while k + 2 < toks.len() {
        if toks[k].0 == 0 && &s[toks[k].1..toks[k].2] == b"macro_rules" && toks[k + 1].0 == 1 && s[toks[k + 1].1] == b'!' && toks[k + 2].0 == 0 {
            v.push(String::from_utf8_lossy(&s[toks[k + 2].1..toks[k + 2].2]).into_owned());
        }
        k += 1;
    }
    v
}

/// 骨組み: 関数の本体の中の、アイテム・ユーザのマクロ (と thread_local!) の呼び出し・括弧・`;` でないトークンを消す。
/// 改行の数は同じ (消したトークンや空白の中の改行は残す)
fn skeleton(t: &str, names: &[String]) -> String {
    let s = t.as_bytes();
    let toks = lexer::tokenize(s);
    let n = toks.len();
    let is_sym = |k: usize, c: u8| toks[k].0 == 1 && s[toks[k].1] == c;
    let mut blank = vec![false; n];
    let mut depth = 0i32;
    let mut pending_fn = false;
    let mut bodies: Vec<i32> = vec![];
    // (アイテムやマクロの呼び出しの終わりのトークン)
    let end_of = |k: usize, mac: bool| -> usize {
        if mac {
            // (k + 2 が開き括弧)
            let mut d = 0i32;
            let mut j = k + 2;
            while j < n {
                if toks[j].0 == 1 {
                    let c = s[toks[j].1];
                    if c == b'(' || c == b'[' || c == b'{' {
                        d += 1;
                    } else if c == b')' || c == b']' || c == b'}' {
                        d -= 1;
                        if d == 0 {
                            return j;
                        }
                    }
                }
                j += 1;
            }
            return n - 1;
        }
        let mut d = 0i32;
        let mut j = k + 1;
        while j < n {
            if toks[j].0 == 1 {
                let c = s[toks[j].1];
                if c == b'(' || c == b'[' {
                    d += 1;
                } else if c == b')' || c == b']' {
                    d -= 1;
                } else if c == b';' && d == 0 {
                    return j;
                } else if c == b'{' && d == 0 {
                    let mut e = 0i32;
                    let mut m = j;
                    while m < n {
                        if is_sym(m, b'{') {
                            e += 1;
                        } else if is_sym(m, b'}') {
                            e -= 1;
                            if e == 0 {
                                return m;
                            }
                        }
                        m += 1;
                    }
                    return n - 1;
                }
            }
            j += 1;
        }
        n - 1
    };
    // (本体ごとの (開きのトークン, 閉じのトークン, 残すものがあるか))
    let mut spans: Vec<(usize, usize, bool)> = vec![];
    let mut open_at: Vec<usize> = vec![];
    // const / static の初期化式 (`=` から `;` まで): ユーザのマクロの呼び出しのほかは消す
    let mut init_start: usize = 0;
    let mut init_end: usize = 0;
    let mut k = 0;
    while k < n {
        let in_body = match bodies.last() {
            Some(&d) => depth > d,
            None => false,
        };
        let in_init = k >= init_start && k < init_end;
        if (in_body || in_init) && toks[k].0 == 0 {
            let w = &s[toks[k].1..toks[k].2];
            let stmt_start = k == 0 || is_sym(k - 1, b'{') || is_sym(k - 1, b'}') || is_sym(k - 1, b';') || is_sym(k - 1, b']');
            let item_kw = in_body
                && matches!(
                    w,
                    b"fn" | b"struct" | b"enum" | b"const" | b"use" | b"static" | b"impl" | b"trait" | b"type" | b"mod" | b"union" | b"macro_rules"
                );
            let mac = k + 2 < n
                && is_sym(k + 1, b'!')
                && (is_sym(k + 2, b'(') || is_sym(k + 2, b'[') || is_sym(k + 2, b'{'))
                && (w == b"thread_local" || names.iter().any(|x| x.as_bytes() == w));
            if (stmt_start && item_kw) || mac {
                // (残す。中の波括弧は釣り合っているので深さは変わらない)
                if let Some(sp) = spans.last_mut() {
                    if in_body {
                        sp.2 = true;
                    }
                }
                k = end_of(k, mac && !(stmt_start && item_kw)) + 1;
                continue;
            }
        }
        if in_init {
            blank[k] = true;
            k += 1;
            continue;
        }
        if in_body {
            let keep = toks[k].0 == 1 && matches!(s[toks[k].1], b'{' | b'}' | b'(' | b')' | b'[' | b']' | b';');
            if !keep {
                blank[k] = true;
            }
        } else if toks[k].0 == 0 && depth == 0 && k >= init_end {
            // (チャンクのいちばん外の const / static の初期化式: `=` の次から `;` の前まで)
            let w = &s[toks[k].1..toks[k].2];
            let is_fn = k + 1 < n && toks[k + 1].0 == 0 && &s[toks[k + 1].1..toks[k + 1].2] == b"fn";
            if (w == b"const" || w == b"static") && !is_fn {
                let mut j = k + 1;
                let mut d = 0i32;
                let mut eq = n;
                while j < n {
                    if toks[j].0 == 1 {
                        let c = s[toks[j].1];
                        if c == b'(' || c == b'[' || c == b'{' {
                            d += 1;
                        } else if c == b')' || c == b']' || c == b'}' {
                            d -= 1;
                        } else if c == b';' && d <= 0 {
                            break;
                        } else if c == b'=' && d <= 0 {
                            eq = j;
                            break;
                        }
                    }
                    j += 1;
                }
                if eq < n {
                    let mut e = eq + 1;
                    let mut dd = 0i32;
                    while e < n {
                        if toks[e].0 == 1 {
                            let c = s[toks[e].1];
                            if c == b'(' || c == b'[' || c == b'{' {
                                dd += 1;
                            } else if c == b')' || c == b']' || c == b'}' {
                                dd -= 1;
                            } else if c == b';' && dd <= 0 {
                                break;
                            }
                        }
                        e += 1;
                    }
                    init_start = eq + 1;
                    init_end = e;
                }
            }
        }
        if toks[k].0 == 0 && &s[toks[k].1..toks[k].2] == b"fn" {
            pending_fn = true;
        }
        if toks[k].0 == 1 {
            match s[toks[k].1] {
                b'{' => {
                    if pending_fn {
                        bodies.push(depth);
                        open_at.push(k);
                        spans.push((k, n, false));
                        pending_fn = false;
                    }
                    depth += 1;
                }
                b'}' => {
                    depth -= 1;
                    if bodies.last() == Some(&depth) {
                        bodies.pop();
                        if let Some(o) = open_at.pop() {
                            if let Some(sp) = spans.iter_mut().rev().find(|sp| sp.0 == o) {
                                sp.1 = k;
                            }
                        }
                    }
                }
                b';' => pending_fn = false,
                _ => {}
            }
        }
        k += 1;
    }
    // (残すもののない本体は、括弧も含めて中を全部消す)
    for &(o, c, has) in &spans {
        if !has && c < n {
            let mut m = o + 1;
            while m < c {
                blank[m] = true;
                m += 1;
            }
        }
    }
    let mut out: Vec<u8> = Vec::with_capacity(t.len() / 2 + 16);
    let mut last = 0usize;
    let squeeze = |out: &mut Vec<u8>, b: &[u8]| {
        let nl = count_nl(b);
        if nl == 0 {
            if !b.is_empty() && !matches!(out.last(), Some(b' ') | Some(b'\n')) {
                out.push(b' ');
            }
        } else {
            for _ in 0..nl {
                out.push(b'\n');
            }
        }
    };
    for k in 0..n {
        let (a, b) = (toks[k].1, toks[k].2);
        if a < last {
            continue;
        }
        if blank[k] || (k > 0 && blank[k - 1]) {
            squeeze(&mut out, &s[last..a]);
        } else {
            out.extend_from_slice(&s[last..a]);
        }
        if blank[k] {
            squeeze(&mut out, &s[a..b]);
            if !matches!(out.last(), Some(b' ') | Some(b'\n')) {
                out.push(b' ');
            }
        } else {
            out.extend_from_slice(&s[a..b]);
        }
        last = b;
    }
    if n > 0 && blank[n - 1] {
        squeeze(&mut out, &s[last..]);
    } else {
        out.extend_from_slice(&s[last..]);
    }
    String::from_utf8(out).unwrap_or_else(|_| t.to_string())
}

/// 前処理の結果をチャンク (lines: 各チャンクの開始行 (0 始まり)、最後は全体の行数) ごとに分ける。
/// 行の数が入力と合わなければ None
fn split_out(p: &Prep, lines: &[usize], fix: &[(u32, u32, i32)], want: &[bool]) -> Option<Vec<Option<ChunkOut>>> {
    let total = lines[lines.len() - 1];
    let b = p.text.as_bytes();
    let mut nls: Vec<usize> = vec![];
    for (i, &c) in b.iter().enumerate() {
        if c == b'\n' {
            nls.push(i);
        }
    }
    // (本文: 入力の行の数だけ。移したものがあれば、その次の改行から後ろ)
    let (body_end, tail) = match &p.moved {
        Some(info) => {
            let sep = *nls.get(total - 1)?;
            let t = &p.text[sep + 1..p.text.len().checked_sub(1)?];
            let mut pieces: Vec<(usize, String)> = vec![];
            let mut at = 0usize;
            let tb = t.as_bytes();
            for &(line, nl) in info {
                // (nl 個の改行を含む部分と、続く区切りの改行)
                let mut e = at;
                let mut seen = 0;
                while e < tb.len() && !(tb[e] == b'\n' && seen == nl) {
                    if tb[e] == b'\n' {
                        seen += 1;
                    }
                    e += 1;
                }
                pieces.push((line, t[at..e].to_string()));
                at = e + 1;
            }
            (sep, pieces)
        }
        None => {
            if nls.len() != total - 1 {
                return None;
            }
            (b.len(), vec![])
        }
    };
    if nls.iter().filter(|&&x| x < body_end).count() != total - 1 {
        return None;
    }
    let line_start = |l: usize| -> usize { if l == 0 { 0 } else { nls[l - 1] + 1 } };
    let mut outs: Vec<Option<ChunkOut>> = vec![];
    for j in 0..lines.len() - 1 {
        if !want[j] {
            outs.push(None);
            continue;
        }
        let (l0, l1) = (lines[j], lines[j + 1]);
        let a = line_start(l0);
        let e = if l1 >= total { body_end } else { line_start(l1) };
        let mut c = ChunkOut { text: p.text[a..e].to_string(), pieces: vec![], fix: vec![] };
        for (line, t) in &tail {
            if *line >= l0 && *line < l1 {
                // (移したものがチャンクの外まで続くなら使わない)
                if *line + count_nl(t.as_bytes()) >= l1 {
                    return None;
                }
                c.pieces.push(t.clone());
            }
        }
        for &(fl, col, cum) in fix {
            let l = fl as usize - 1;
            if l >= l0 && l < l1 {
                c.fix.push(((l - l0) as u32, col, cum));
            }
        }
        outs.push(Some(c));
    }
    Some(outs)
}

/// 前処理 (差分)。fix には桁のずれを足す
pub fn prep(src: &str, crates: &[String], fix: &mut Vec<(u32, u32, i32)>, cfg: Option<&crate::cfg::CfgSet>, memo: &mut PrepMemo) -> R<String> {
    memo.calls += 1;
    if std::env::var_os("CRUSTC_NO_PREP_INC").is_some() || (memo.calls == 1 && std::env::var_os("CRUSTC_PREP_VERIFY").is_none()) {
        return prep_full(src, crates, fix, cfg).map(|p| p.text);
    }
    let r = prep_inc(src, crates, cfg, memo);
    let (text, fx) = match r {
        Ok(Some(x)) => x,
        Ok(None) => {
            let c = memo.calls;
            *memo = PrepMemo::default();
            memo.calls = c;
            return prep_full(src, crates, fix, cfg).map(|p| p.text);
        }
        Err(e) => {
            let c = memo.calls;
            *memo = PrepMemo::default();
            memo.calls = c;
            return Err(e);
        }
    };
    if std::env::var_os("CRUSTC_PREP_VERIFY").is_some() {
        let mut f2 = vec![];
        let full = prep_full(src, crates, &mut f2, cfg)?;
        if full.text != text || f2 != fx {
            if let Ok(p) = std::env::var("CRUSTC_PREP_VERIFY_DUMP") {
                let _ = std::fs::write(format!("{p}.inc"), &text);
                let _ = std::fs::write(format!("{p}.full"), &full.text);
            }
            return Err("internal: incremental preprocessing differs from the full preprocessing".into());
        }
    }
    fix.extend(fx);
    Ok(text)
}

/// 差分の前処理: (本文, 桁のずれ)。前処理が失敗したら Err、使えなければ Ok(None) (呼ぶ側が全体を前処理する)
fn prep_inc(src: &str, crates: &[String], cfg: Option<&crate::cfg::CfgSet>, memo: &mut PrepMemo) -> R<Option<(String, Vec<(u32, u32, i32)>)>> {
    let mut key = FNV0;
    for c in crates {
        key = fnv(mix(key, 1), c.as_bytes());
    }
    if let Some(c) = cfg {
        key = fnv(mix(key, 2), format!("{c:?}").as_bytes());
    }
    if memo.key != key {
        let c = memo.calls;
        *memo = PrepMemo::default();
        memo.calls = c;
        memo.key = key;
    }
    let log = std::env::var_os("CRUSTC_PREP_LOG").is_some();
    let (starts, balanced) = chunk_starts(src, &mut memo.chunks);
    if !balanced {
        return Ok(None);
    }
    if log {
        eprintln!("prep: {} chunks", starts.len());
    }
    let sb = src.as_bytes();
    // (チャンクの開始行と、全体の行数)
    let mut lines: Vec<usize> = Vec::with_capacity(starts.len() + 1);
    let mut line = 0usize;
    let mut at = 0usize;
    for &(st, _) in &starts {
        line += count_nl(&sb[at..st]);
        at = st;
        lines.push(line);
    }
    lines.push(line + count_nl(&sb[at..]) + 1);
    let nchunk = starts.len();
    let text_of = |j: usize| -> &str {
        let e = if j + 1 < nchunk { starts[j + 1].0 } else { src.len() };
        &src[starts[j].0..e]
    };
    let mut hs: Vec<u64> = Vec::with_capacity(nchunk);
    for j in 0..nchunk {
        hs.push(fnv(FNV0, text_of(j).as_bytes()));
    }
    // マクロの名前
    let mut names: Vec<String> = vec![];
    let mut new_names: FxMap<u64, Vec<String>> = FxMap::default();
    for j in 0..nchunk {
        let v = match memo.names.get(&hs[j]) {
            Some(v) => v.clone(),
            None => macro_names(text_of(j)),
        };
        for x in &v {
            if !names.contains(x) {
                names.push(x.clone());
            }
        }
        new_names.insert(hs[j], v);
    }
    memo.names = new_names;
    names.sort();
    let mut nh = FNV0;
    for x in &names {
        nh = fnv(mix(nh, 3), x.as_bytes());
    }
    // 骨組みと、その全体のハッシュ
    let mut fp = mix(key, nh);
    let mut new_skel: FxMap<(u64, u64), (String, u64)> = FxMap::default();
    for j in 0..nchunk {
        let k = (hs[j], nh);
        let e = match memo.skel.remove(&k) {
            Some(e) => e,
            None => match new_skel.get(&k) {
                Some(e) => (e.0.clone(), e.1),
                None => {
                    let sk = skeleton(text_of(j), &names);
                    let h = fnv(FNV0, sk.as_bytes());
                    (sk, h)
                }
            },
        };
        fp = mix(mix(fp, starts[j].1), e.1);
        new_skel.insert(k, e);
    }
    memo.skel = new_skel;
    if log {
        eprintln!("prep: same fp: {}", memo.fp == fp);
    }
    // (チャンクの結果のキー)
    let mut occ: FxMap<(u64, u64), u32> = FxMap::default();
    let mut keys: Vec<(u64, u64, u32)> = Vec::with_capacity(nchunk);
    for j in 0..nchunk {
        let c = occ.entry((hs[j], starts[j].1)).or_insert(0);
        keys.push((hs[j], starts[j].1, *c));
        *c += 1;
    }
    let mut outs: Vec<Option<ChunkOut>> = (0..nchunk).map(|_| None).collect();
    if memo.fp == fp && !memo.out.is_empty() {
        let mut missing = vec![false; nchunk];
        let mut any = false;
        for j in 0..nchunk {
            match memo.out.remove(&keys[j]) {
                Some(o) => outs[j] = Some(o),
                None => {
                    missing[j] = true;
                    any = true;
                }
            }
        }
        if any {
            // 変わったチャンクだけそのまま、ほかは骨組みにしたソースを前処理して、変わったチャンクの行を切り出す
            let mut red = String::with_capacity(src.len() / 2);
            for j in 0..nchunk {
                if missing[j] {
                    red.push_str(text_of(j));
                } else {
                    match memo.skel.get(&(hs[j], nh)) {
                        Some(e) => red.push_str(&e.0),
                        None => return Ok(None),
                    }
                }
            }
            if log {
                let nm = missing.iter().filter(|&&x| x).count();
                eprintln!("prep: {nm} changed, reduced {} of {} bytes", red.len(), src.len());
            }
            let mut f = vec![];
            let p = match prep_full(&red, crates, &mut f, cfg) {
                Ok(p) => p,
                Err(_) => return Ok(None),
            };
            let Some(got) = split_out(&p, &lines, &f, &missing) else { return Ok(None) };
            for (j, g) in got.into_iter().enumerate() {
                if let Some(g) = g {
                    outs[j] = Some(g);
                }
            }
        }
    } else {
        let mut f = vec![];
        let p = prep_full(src, crates, &mut f, cfg)?;
        let all = vec![true; nchunk];
        let Some(got) = split_out(&p, &lines, &f, &all) else { return Ok(None) };
        outs = got;
        memo.fp = fp;
    }
    // 組み立てる
    let mut text = String::with_capacity(src.len() + 64);
    let mut pieces: Vec<&str> = vec![];
    let mut fx: Vec<(u32, u32, i32)> = vec![];
    for j in 0..nchunk {
        let Some(o) = &outs[j] else { return Ok(None) };
        text.push_str(&o.text);
        for p in &o.pieces {
            pieces.push(p);
        }
        for &(l, c, d) in &o.fix {
            fx.push((l + lines[j] as u32 + 1, c, d));
        }
    }
    if !pieces.is_empty() {
        for p in &pieces {
            text.push('\n');
            text.push_str(p);
        }
        text.push('\n');
    }
    memo.out = FxMap::default();
    for (j, o) in outs.into_iter().enumerate() {
        if let Some(o) = o {
            memo.out.insert(keys[j], o);
        }
    }
    Ok(Some((text, fx)))
}
