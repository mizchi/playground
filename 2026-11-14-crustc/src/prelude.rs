//! 組み込みの prelude。core の代わりに、よく使う型をこのコンパイラが扱えるサブセットで書いたもの。
//! ユーザのソースの後ろに付けて一緒にコンパイルする (使われたものだけがコード生成される)。

/// `Box::new` が呼ぶアロケータ (libc の malloc)
pub const BOX_ALLOC: &str = "__exr_malloc";
/// Box の drop が呼ぶ解放関数 (libc の free)
pub const BOX_FREE: &str = "__exr_free";
/// drop glue: `__exr_drop_in_place::<T>` の本体は codegen が型ごとに作る
pub const DROP_GLUE: &str = "__exr_drop_in_place";

/// prelude の本体 (Rust で書いた core / alloc の代わり)。build.rs がアイテムの分割を前もって済ませる
pub const PRELUDE: &str = include_str!("prelude_lib.rs.in");

include!(concat!(env!("OUT_DIR"), "/prelude_items.rs"));

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::lexer::{ItemKind, SrcItem};

/// prelude のアイテム (一度だけ字句解析する)。(アイテム, 定義する名前, core:: 経由でしか使えないか)
fn items() -> &'static [(SrcItem, Option<String>, bool)] {
    static ITEMS: OnceLock<Vec<(SrcItem, Option<String>, bool)>> = OnceLock::new();
    ITEMS.get_or_init(|| {
        // (分割は build.rs が済ませてある)
        PRELUDE_ITEMS
            .iter()
            .map(|&(start, end, line, k, a, idents)| SrcItem {
                start: start as usize,
                end: end as usize,
                line: line as usize,
                hash: 0,
                idents: idents.to_vec(),
                kind: match k {
                    0 => ItemKind::Normal,
                    1 => ItemKind::ImplHeader,
                    2 => ItemKind::ImplMember(a),
                    3 => ItemKind::TraitHeader,
                    4 => ItemKind::TraitMember(a),
                    _ => ItemKind::InnerAttr,
                },
            })
            .map(|it| {
                let name = defined_name(PRELUDE, &it);
                let core_only = it
                    .idents
                    .iter()
                    .any(|&(a, b)| PRELUDE[a as usize..b as usize].starts_with("__core_"));
                (it, name, core_only)
            })
            .collect()
    })
}

/// イテレータを作る呼び出し (`.iter()` / `.into_iter()` / `(a..b)`) の後ろに、for が直接扱える
/// `rev` / `enumerate` / `copied` / `cloned` 以外のメソッドが続くか (`v.iter().map(..)` など)
fn iter_chain(src: &str) -> bool {
    let b = src.as_bytes();
    let starts = ["iter()", "iter_mut()", "into_iter()"];
    let mut found = vec![];
    for s in starts {
        let mut i = 0;
        while let Some(k) = src[i..].find(s) {
            found.push(i + k + s.len());
            i += k + s.len();
        }
    }
    // `(a..b).` の `)` の後ろ
    let mut i = 0;
    while let Some(k) = src[i..].find("..") {
        let mut j = i + k + 2;
        while j < b.len() && !matches!(b[j], b'(' | b')' | b';' | b'{' | b'}' | b'\n') {
            j += 1;
        }
        if j < b.len() && b[j] == b')' {
            found.push(j + 1);
        }
        i += k + 2;
    }
    for mut j in found {
        // `.name(..)` の列を辿る
        loop {
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= b.len() || b[j] != b'.' {
                break;
            }
            let st = j + 1;
            let mut e = st;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                e += 1;
            }
            let name = &src[st..e];
            if !matches!(name, "rev" | "enumerate" | "copied" | "cloned") {
                return !name.is_empty();
            }
            // 引数の括弧を飛ばす
            j = e;
            if j < b.len() && b[j] == b'(' {
                let mut d = 0;
                while j < b.len() {
                    match b[j] {
                        b'(' => d += 1,
                        b')' => {
                            d -= 1;
                            if d == 0 {
                                j += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
            }
        }
    }
    false
}

/// src[a..b] の識別子がメソッド呼び出し (`.name(` / `.name::<`) の位置にあるか (フィールド `.next` は除く)
fn method_call_at(src: &[u8], a: usize, b: usize) -> bool {
    a > 0 && src[a - 1] == b'.' && b < src.len() && (src[b] == b'(' || src[b] == b':')
}

/// Iterator の trait / impl (と IntoIter の Drop) のヘッダか
fn is_iterator_trait(h: usize) -> bool {
    let it = &items()[h].0;
    it.idents
        .iter()
        .any(|&(a, b)| matches!(&PRELUDE[a as usize..b as usize], "Iterator" | "IntoIter"))
}

/// `__exr_str_<m>` / `__exr_char_<m>` の helper: メソッド名 m → helper の名前
fn helper_suffixes() -> &'static HashMap<&'static str, Vec<&'static str>> {
    static H: OnceLock<HashMap<&'static str, Vec<&'static str>>> = OnceLock::new();
    H.get_or_init(|| {
        let mut m: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        for (_, name, _) in items() {
            let Some(n) = name.as_deref() else { continue };
            for pre in ["__exr_str_", "__exr_char_"] {
                if let Some(suf) = n.strip_prefix(pre) {
                    if !suf.is_empty() {
                        // (`trim_matches_fn` はクロージャのパターン版: メソッド名 `trim_matches` で引く)
                        let suf = suf.strip_suffix("_fn").unwrap_or(suf);
                        m.entry(suf).or_default().push(n);
                    }
                }
            }
        }
        m
    })
}

/// 比較の trait (PartialEq / Eq / PartialOrd / Ord) の宣言か
fn is_cmp_trait(h: usize) -> bool {
    let it = &items()[h].0;
    matches!(it.kind, ItemKind::TraitHeader)
        && it
            .idents
            .iter()
            .any(|&(a, b)| matches!(&PRELUDE[a as usize..b as usize], "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Clone" | "Copy" | "Default"))
}

/// 演算子の trait (宣言はユーザが手で impl するときだけ付ける)
const OP_TRAITS: &[&str] = &[
    "Add", "Sub", "Mul", "Div", "Rem", "Neg", "Not", "BitAnd", "BitOr", "BitXor", "AddAssign", "SubAssign",
    "MulAssign", "DivAssign", "RemAssign", "Index", "IndexMut", "From", "Deref", "IntoIterator", "Error",
];

/// 演算子の trait の宣言か
fn is_op_trait(h: usize) -> bool {
    let it = &items()[h].0;
    matches!(it.kind, ItemKind::TraitHeader)
        && it.idents.iter().any(|&(a, b)| OP_TRAITS.contains(&&PRELUDE[a as usize..b as usize]))
}

/// ユーザが手で impl している (`impl Ord for S` / `impl<T> Add for W<T>`) trait の名前 (最後のセグメント)
fn user_impl_traits(src: &str) -> Vec<&str> {
    let mut out = vec![];
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(k) = src[i..].find("impl") {
        let mut j = i + k + 4;
        i = j;
        let skip_ws = |j: &mut usize| {
            while *j < b.len() && b[*j].is_ascii_whitespace() {
                *j += 1;
            }
        };
        skip_ws(&mut j);
        if j < b.len() && b[j] == b'<' {
            let mut depth = 0;
            while j < b.len() {
                match b[j] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            skip_ws(&mut j);
        }
        let start = j;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b':') {
            j += 1;
        }
        let name = src[start..j].rsplit("::").next().unwrap_or("");
        // (`impl Mul<i32> for V` の型引数は飛ばす)
        if j < b.len() && b[j] == b'<' {
            let mut depth = 0;
            while j < b.len() {
                match b[j] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
        }
        skip_ws(&mut j);
        if !name.is_empty() && src[j..].starts_with("for") {
            out.push(name);
        }
    }
    out
}

/// ユーザが比較の trait を手で実装しているか (`impl Ord for S` / `impl<T> PartialEq for W<T>`)
#[allow(dead_code)]
fn impls_cmp_trait(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(k) = src[i..].find("impl") {
        let mut j = i + k + 4;
        i = j;
        let skip_ws = |j: &mut usize| {
            while *j < b.len() && b[*j].is_ascii_whitespace() {
                *j += 1;
            }
        };
        skip_ws(&mut j);
        if j < b.len() && b[j] == b'<' {
            let mut depth = 0;
            while j < b.len() {
                match b[j] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            skip_ws(&mut j);
        }
        let start = j;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b':') {
            j += 1;
        }
        let name = src[start..j].rsplit("::").next().unwrap_or("");
        if matches!(name, "PartialEq" | "Eq" | "PartialOrd" | "Ord") {
            skip_ws(&mut j);
            if src[j..].starts_with("for") {
                return true;
            }
        }
    }
    false
}

/// `(1..)` のような終わりのない範囲を値として使っていそうか (`..)` があるか)
fn open_range(src: &str) -> bool {
    // `(1..)` / `(start..)`: `(` と `..)` の間が識別子か数字だけ
    let b = src.as_bytes();
    src.match_indices("..)").any(|(i, _)| {
        let mut k = i;
        while k > 0 && (b[k - 1].is_ascii_alphanumeric() || b[k - 1] == b'_') {
            k -= 1;
        }
        k < i && k > 0 && b[k - 1] == b'('
    })
}

/// 書式に `{:e}` / `{:.2E}` のような指数表記の指定があるか
fn fmt_uses_exp(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(k) = src[i..].find('{') {
        let st = i + k + 1;
        let mut j = st;
        while j < b.len() && b[j] != b'}' && b[j] != b'{' && b[j] != b'\n' && j - st < 24 {
            j += 1;
        }
        if j < b.len() && b[j] == b'}' && j > st && matches!(b[j - 1], b'e' | b'E') {
            if let Some(c) = src[st..j].find(':') {
                let sp = &src[st + c + 1..j - 1];
                if sp.bytes().all(|x| x.is_ascii_digit() || matches!(x, b'.' | b'+' | b'#' | b'<' | b'>' | b'^' | b'$')) {
                    return true;
                }
            }
        }
        i = st;
    }
    false
}

/// `vec![x; n]` の x がリテラルでないものがあるか
fn vec_repeat_nonliteral(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(k) = src[i..].find("vec![") {
        let start = i + k + 5;
        let mut depth = 0i32;
        let mut j = start;
        while j < b.len() {
            match b[j] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' if depth == 0 => break,
                b')' | b']' | b'}' => depth -= 1,
                b';' if depth == 0 => {
                    let x = src[start..j].trim_start();
                    let lit = x.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '\'')
                        || x.starts_with("true")
                        || x.starts_with("false")
                        || x.starts_with("b'");
                    if !lit {
                        return true;
                    }
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        i = start;
    }
    false
}

/// 書式化で使う prelude の名前
const FMT_NAMES: &[&str] = &[
    "String", "new", "Formatter", "Display", "Debug", "__exr_formatter", "__exr_fmt_spec",
    "__exr_fmt_lit", "__exr_fmt_int", "__exr_fmt_str", "__exr_fmt_bool", "__exr_fmt_char",
    "__exr_fmt_str_debug", "__exr_fmt_char_debug", "__exr_dbg_begin", "__exr_dbg_item",
    "__exr_dbg_item_end", "__exr_dbg_end", "__exr_print", "__exr_eprint", "__exr_display",
    "__exr_debug", "__exr_fmt_ok", "fmt", "__FmtError",
];

/// 書式の引数つきの panic 系マクロ (`panic!("bad {}", x)` など) があるか
fn panic_with_args(src: &str) -> bool {
    ["panic!(", "unreachable!(", "todo!(", "unimplemented!(", "assert!(", "assert_eq!(", "assert_ne!("].iter().any(|m| {
        src.match_indices(m).any(|(i, _)| {
            let rest = &src[i + m.len()..];
            let line = rest.split('\n').next().unwrap_or("");
            line.contains('{') && line.contains("\",")
        })
    })
}

/// 文字列リテラル以外への `.to_string()` があるか (`5.to_string()` は書式化を使う)
fn nonliteral_to_string(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(k) = src[i..].find(".to_string()") {
        let at = i + k;
        if at == 0 || b[at - 1] != b'"' {
            return true;
        }
        i = at + 1;
    }
    false
}

/// これらの名前が現れたら Iterator を使っているとみなす
const ITER_NAMES: &[&str] = &[
    "Iterator", "next", "map", "filter", "filter_map", "collect", "sum", "product", "count",
    "fold", "for_each", "any", "all", "find", "position", "rposition", "partition", "cycle", "nth", "zip", "chain", "take", "skip",
    "take_while", "copied", "cloned", "into_iter", "flat_map", "flatten", "step_by", "peekable", "skip_while", "scan",
    "inspect",
];

/// prelude のアイテムの識別子を need に足す (Iterator のメソッド名はメソッド呼び出しの位置だけ)
fn add_idents(need: &mut HashSet<&'static str>, it: &SrcItem) {
    let bytes = PRELUDE.as_bytes();
    for &(a, b) in &it.idents {
        let w = &PRELUDE[a as usize..b as usize];
        if w != "Iterator" && ITER_NAMES.contains(&w) && !method_call_at(bytes, a as usize, b as usize) {
            continue;
        }
        if matches!(w, "width" | "precision" | "fill" | "sign_plus") && !method_call_at(bytes, a as usize, b as usize) {
            continue;
        }
        need.insert(w);
    }
}

fn bucket(first: u8, len: usize) -> usize {
    first as usize * 32 + len.min(31)
}

/// Vec / String の impl ヘッダなら、その型名
fn owner(h: usize) -> Option<&'static str> {
    static OWNERS: OnceLock<Vec<Option<&'static str>>> = OnceLock::new();
    OWNERS.get_or_init(|| {
        items()
            .iter()
            .map(|(it, _, _)| {
                if !matches!(it.kind, ItemKind::ImplHeader) {
                    return None;
                }
                it.idents
                    .iter()
                    .map(|&(a, b)| &PRELUDE[a as usize..b as usize])
                    .find_map(|w| match w {
                        "Vec" | "String" | "HashMap" | "Reverse" | "BinaryHeap" | "SplitStr" | "CharIndices"
                        | "Bytes" | "ToCase" | "ParseIntError" | "IntErrorKind" | "Rev" | "StepBy" | "Peekable"
                        | "SkipWhile" | "Scan" | "Inspect" | "FlatMap" | "Flatten" | "Cloned" | "Windows" | "Chunks"
                        | "HashSet" | "SetIntoIter" | "BTreeMap" | "BTreeSet" | "BtEntry" | "BtIter" | "BtIterMut"
                        | "BtIntoIter" | "VecDeque" | "DqIter" | "DqIntoIter" | "f64" | "f32" | "ParseFloatError" | "i128" | "u128" | "FromUtf8Error" | "Utf8Error" | "Rc" | "RefCell" | "Cell" | "RangeFrom" | "TryFromIntError" | "Stdin" | "StdinLock" | "StdinLines" | "Stdout" | "StdoutLock"
                        | "BufWriter" | "IoError" | "__DynError" => Some(w),
                        // HashMap のイテレータ (HmIter など) は HashMap を使うときだけ
                        w if w.starts_with("Hm") => Some("HashMap"),
                        _ => None,
                    })
            })
            .collect()
    })[h]
}

/// `fn x` / `struct X` / `enum X` の名前 (`__core_x` は x として引かれる)
fn defined_name(src: &str, it: &SrcItem) -> Option<String> {
    // (起動時に prelude の全アイテムで呼ぶので、Vec を作らずに辿る)
    let mut ids = it.idents.iter().map(|&(a, b)| &src[a as usize..b as usize]);
    ids.find(|w| matches!(*w, "fn" | "struct" | "enum" | "static"))?;
    ids.next().map(|s| s.strip_prefix("__core_").unwrap_or(s).to_string())
}

/// ユーザのコードから (識別子を辿って) 使われうる prelude のアイテムだけを、ソースとして返す。
/// Option を使わないプログラムには何も足さないので、キャッシュも大きくならない
pub fn select(user_src: &str, user_items: &[SrcItem]) -> String {
    select_with(user_src, user_items, &mut SelectCache::default())
}

/// アイテムごとの「当たった名前」と「イテレータの列があるか」の記憶 (アイテムのハッシュ → 結果)。
/// LSP / インクリメンタルビルドでは変わっていないアイテムを走査し直さない
#[derive(Default)]
pub struct SelectCache {
    map: crate::fxhash::FxMap<crate::cache::Key, (Vec<&'static str>, bool)>,
    /// 直前の選択でのユーザのアイテムの数 (これより後ろのアイテムは prelude)
    pub user_items: usize,
}

pub fn select_with(user_src: &str, user_items: &[SrcItem], cache: &mut SelectCache) -> String {
    let all = items();
    // ユーザの識別子のうち、prelude が定義する名前 (と関係する名前) に当たるものだけを集める。
    // 全識別子の集合を作ると大きなプログラムで毎回 ms 単位かかるので、小さい集合に対して引く
    // 先頭の 1 文字ごとの候補 (数個) と直接比べる。ハッシュしないので大きなプログラムでも速い
    static NAMES: OnceLock<Vec<Vec<&'static str>>> = OnceLock::new();
    let by_first = NAMES.get_or_init(|| {
        let mut h: Vec<&'static str> = all.iter().filter_map(|x| x.1.as_deref()).collect();
        // `s.to_uppercase()` / `c.is_alphabetic()` は `__exr_str_*` / `__exr_char_*` の helper の名前で引く
        h.extend(helper_suffixes().keys().copied());
        h.extend([
            "Some", "None", "Ok", "Err", "core", "std", "Box", "dyn", "Drop", "vec", "to_string",
            "to_owned", "iter", "iter_mut", "chars", "into_iter", "next", "map", "filter",
            "filter_map", "collect", "sum", "product", "count", "fold", "for_each", "any", "all",
            "find", "position", "nth", "zip", "chain", "take", "skip", "take_while", "copied",
            "cloned", "format", "print", "println", "eprint", "eprintln", "write", "writeln",
            "split", "split_whitespace", "lines", "trim", "starts_with", "ends_with", "contains",
            "clamp", "from_u32", "char", "join", "concat", "drain", "default", "f64", "f32", "i128", "u128", "Iterator", "into", "split_at_mut",
        ]);
        // (先頭の 1 byte, 長さ) ごとの候補。prelude の名前が増えても 1 識別子あたりの比較は 0〜1 回
        let mut t: Vec<Vec<&'static str>> = vec![vec![]; 256 * 32];
        for n in h {
            let k = bucket(n.as_bytes()[0], n.len());
            if !t[k].contains(&n) {
                t[k].push(n);
            }
        }
        t
    });
    let mut need: HashSet<&str> = HashSet::new();
    let bytes = user_src.as_bytes();
    let mut chain = false;
    let old = std::mem::take(&mut cache.map);
    for it in user_items {
        if let Some(r) = old.get(&it.hash) {
            need.extend(r.0.iter().copied());
            chain |= r.1;
            cache.map.insert(it.hash, r.clone());
            continue;
        }
        let mut names: Vec<&'static str> = vec![];
        for &(a, b) in &it.idents {
            let cands = &by_first[bucket(bytes[a as usize], (b - a) as usize)];
            if cands.is_empty() {
                continue;
            }
            let w = &user_src[a as usize..b as usize];
            if let Some(n) = cands.iter().find(|n| **n == w) {
                // Iterator のメソッド名は、メソッド呼び出しの位置 (`.count()`) でだけ数える
                // (ローカル変数の `count` や `sum` で Iterator を付けない)
                // (`mem::take(..)` のようなパスの最後は数える)
                let after_path = a >= 2 && &bytes[a as usize - 2..a as usize] == b"::";
                // (Formatter の `width()` などもメソッド呼び出しの位置だけ: フィールドや変数の `width` で付けない)
                if matches!(*n, "width" | "precision" | "fill" | "sign_plus") && !method_call_at(bytes, a as usize, b as usize) {
                    continue;
                }
                if *n != "Iterator" && ITER_NAMES.contains(n) && !method_call_at(bytes, a as usize, b as usize) && !after_path {
                    continue;
                }
                // 書式化のマクロは `name!(` のときだけ (`p.write(..)` のメソッドは別物)
                if matches!(*n, "format" | "print" | "println" | "eprint" | "eprintln" | "write" | "writeln")
                    && bytes.get(b as usize) != Some(&b'!')
                {
                    continue;
                }
                if *n == "cmp" && bytes.get(b as usize) == Some(&b':') {
                    continue;
                }
                // (自分の関数 `clamp(v, lo, hi)` では付けない)
                if *n == "clamp" && !method_call_at(bytes, a as usize, b as usize) {
                    continue;
                }
                if !names.contains(n) {
                    names.push(n);
                }
            }
        }
        // イテレータの列はアイテムの中にしか現れない
        let c = !names.is_empty() && iter_chain(&user_src[it.start..it.end]);
        need.extend(names.iter().copied());
        chain |= c;
        cache.map.insert(it.hash, (names, c));
    }
    // (std のプログラムは panic のハンドラを必ず使うので、何も使わなくても prelude を付ける)
    if need.is_empty() && user_src.contains("no_std") {
        return String::new();
    }
    // 書式化のマクロ: Formatter と書き出しの関数一式 (codegen が名前を書かずに呼ぶ)
    if ["format", "print", "println", "eprint", "eprintln", "write", "writeln", "fmt", "Formatter"]
        .iter()
        .any(|n| need.contains(n))
        || nonliteral_to_string(user_src)
        || panic_with_args(user_src)
        // (assert_eq! / assert_ne! は失敗したとき left / right を {:?} で出す。no_std は panic_handler を呼ぶだけ)
        || ((user_src.contains("assert_eq!(") || user_src.contains("assert_ne!(")) && !user_src.contains("no_std"))
    {
        need.extend(FMT_NAMES.iter().copied());
    }
    // `vec![..]` は Vec::with_capacity + push に、`"..".to_string()` は String::from になる
    if need.contains("vec") {
        need.extend(["Vec", "with_capacity", "push"]);
    }
    if need.contains("to_string") || need.contains("to_owned") {
        need.extend(["String", "from"]);
    }
    // Iterator (アダプタ・消費するメソッド) を使うときだけ、イテレータの部品を付ける。
    // `for x in v.iter()` だけなら codegen が直接回すので要らない
    // (イテレータの列は必ず作るもの (iter / into_iter / chars / 範囲 / 自前の impl Iterator) から始まる。
    //  Option::map や自分の型の fold だけでは付けない)
    let producer = ["iter", "iter_mut", "into_iter"].iter().any(|n| need.contains(n));
    // &str の split / lines などは prelude の関数 (イテレータを返すものはイテレータの部品も要る)
    for (m, f) in [
        ("split", "__exr_str_split"),
        ("split_whitespace", "__exr_str_split_ws"),
        ("lines", "__exr_str_lines"),
        ("trim", "__exr_str_trim"),
        ("starts_with", "__exr_str_starts_with"),
        ("ends_with", "__exr_str_ends_with"),
        ("contains", "__exr_str_find"),
        ("find", "__exr_str_find"),
    ] {
        if need.contains(m) {
            need.insert(f);
            if !matches!(m, "split" | "split_whitespace" | "lines" | "trim") {
                need.insert("__exr_char_utf8");
            }
        }
    }
    if need.contains("HashSet") {
        need.extend(["HashMap", "keys", "into_iter", "contains_key", "next"]);
    }
    if need.contains("HashMap") {
        need.extend(["__exr_hash", "__exr_hash_bytes"]);
    }
    if need.contains("BTreeMap") || need.contains("BTreeSet") {
        need.extend(["__exr_cmp", "Vec", "insert", "remove", "next"]);
    }
    if need.contains("VecDeque") {
        need.extend(["next", "Drop", "index", "index_mut"]);
    }
    if need.contains("BTreeMap") {
        need.insert("index");
    }
    // `s + "x"` / `s += "x"`: String の add / add_assign (演算子は識別子として現れない)
    if need.contains("String") || need.contains("to_string") || need.contains("format") {
        need.extend(["add", "add_assign"]);
    }
    // &str / String / char の helper (`s.replace(..)` → __exr_str_replace)
    let hs = helper_suffixes();
    // (整数・Option・イテレータにも同じ名前のメソッドがあるものは、文字列を使っていそうなときだけ:
    //  String が現れるか、文字列リテラルが受け手 (`"ab".repeat(3)`))
    let str_evidence = |n: &str| -> bool {
        need.contains("String") || user_src.contains(&format!("\".{n}("))
    };
    let found: Vec<&'static str> = need
        .iter()
        .filter(|n| {
            !matches!(**n, "to_ascii_uppercase" | "to_ascii_lowercase" | "eq_ignore_ascii_case" | "replace" | "repeat")
                || str_evidence(n)
        })
        .filter_map(|n| hs.get(n))
        .flatten()
        .copied()
        .collect();
    // (chars で回す helper とイテレータを返す helper はイテレータの部品も要る)
    let iter_helper = found.iter().any(|f| {
        matches!(
            *f,
            "__exr_str_char_indices" | "__exr_str_bytes" | "__exr_char_to_uppercase" | "__exr_char_to_lowercase"
        )
    });
    // (helper の &str の引数に char を渡すと codegen が UTF-8 にする)
    if found.iter().any(|f| f.starts_with("__exr_str_")) {
        need.insert("__exr_char_utf8");
    }
    need.extend(found);
    if iter_helper {
        need.extend(["chars", "Iterator", "next"]);
    }
    // `s.split("ab")` (char 以外の区切り)
    if need.contains("split")
        && user_src.match_indices(".split(").any(|(i, m)| user_src.as_bytes().get(i + m.len()) != Some(&b'\''))
    {
        need.extend(["__exr_str_split_str", "Iterator"]);
    }
    if need.contains("from") && need.contains("char") {
        need.insert("__exr_char_from_u8");
    }
    if need.contains("from_u32") {
        need.insert("__exr_char_from_u32_checked");
    }
    if need.contains("parse") {
        need.extend(["__from_str", "__FromStr"]);
    }
    if need.contains("flat_map") || need.contains("flatten") {
        need.extend(["__flat_prep", "__flat_next", "__Flat", "__flat_mark", "reverse", "pop", "take"]);
    }
    // `T::default()` / `Default::default()` / `..Default::default()` / unwrap_or_default / mem::take
    if ["default", "Default", "unwrap_or_default", "take"].iter().any(|n| need.contains(n)) {
        need.insert("__exr_default");
    }
    // `x.clone()` を codegen が __exr_clone::<T> にする (prelude の Vec::clone / cloned / to_vec の中も)
    // (drop の要らない型と、自分の clone を持つ型 (String / Vec) には要らない)
    if ["clone", "cloned", "to_vec", "resize"].iter().any(|n| need.contains(n)) {
        need.insert("__exr_clone");
    }
    if need.contains("join") || need.contains("concat") {
        need.extend(["__exr_join_str", "__exr_join_string"]);
        if need.contains("concat") {
            need.insert("__exr_concat_vec");
        }
    }
    if need.contains("drain") {
        need.insert("__drain");
    }
    for (m, f) in [
        ("windows", "__exr_slice_windows"),
        ("chunks", "__exr_slice_chunks"),
        ("to_vec", "__exr_slice_to_vec"),
        ("split_at", "__exr_slice_split_at"),
        ("split_at_mut", "__exr_slice_split_at_mut"),
    ] {
        if need.contains(m) {
            need.extend([f, "Iterator", "next"]);
        }
    }
    // checked_* は Option を返す
    if user_src.contains("checked_") || user_src.contains("overflowing_") {
        need.insert("Option");
    }
    // `vec![x; n]` で x が Copy でなさそうなら (リテラルでない) resize で並べる
    if need.contains("vec") && vec_repeat_nonliteral(user_src) {
        need.extend(["resize", "new", "__exr_clone"]);
    }
    // String への collect は要素が &str / String なら push_str を使う
    if need.contains("collect") && need.contains("String") {
        need.extend(["push_str", "as_str"]);
    }
    // Rc は deref の先のメソッドを呼ぶ
    if need.contains("Rc") {
        need.insert("deref");
    }
    // `char::is_ascii_digit` を関数の値として
    if user_src.contains("char::is_ascii") || user_src.contains("char::to_ascii") {
        for (_, name, _) in items() {
            if let Some(n) = name.as_deref() {
                if let Some(m) = n.strip_prefix("__exr_charfn_") {
                    if user_src.contains(&format!("char::{m}")) {
                        need.insert(n);
                    }
                }
            }
        }
    }
    // Box を通した再帰型の `==` は glue 関数で (その場で展開すると止まらない)
    if need.contains("Box") && user_src.contains("PartialEq") {
        need.insert("__exr_eq");
    }
    if need.contains("BTreeMap") && user_src.contains(".range(") {
        need.extend(["range", "__bt_span", "Range", "RangeInclusive"]);
        if open_range(user_src) || user_src.contains("..))") {
            need.insert("RangeFrom");
        }
    }
    if need.contains("rotate_left") || need.contains("rotate_right") {
        need.extend(["rotate_left", "__rev_range", "swap"]);
    }
    if user_src.contains(".or_default()") {
        need.extend(["or_default", "__exr_default"]);
    }
    // (Formatter のビルダーの entries はイテレータを受け取る)
    if user_src.contains(".entries(") {
        need.extend(["Iterator", "iter", "next", "into_iter"]);
    }
    // `HashMap::from([..])` は vec![..] にする
    if user_src.contains("::from([") {
        need.extend(["from", "vec", "reverse", "pop", "insert"]);
    }
    if need.contains("extend") && need.contains("String") {
        need.extend(["push_str", "as_str", "push"]);
    }
    if user_src.contains("].into_iter()") {
        need.extend(["vec", "into_iter", "IntoIter", "Iterator", "next"]);
    }
    if need.contains("cycle") {
        need.extend(["Cycle", "__exr_clone"]);
    }
    // 算術の検査の panic (no_std で panic_handler を持つプログラムは使わない)
    if !user_src.contains("no_std") {
        need.extend(["__exr_err_str", "__exr_fflush", "__exr_write", "__exr_exit"]);
        need.extend(["__exr_rt_init", "__exr_on_trap", "__exr_sigaction", "__EXR_TRAP_TABLE", "__EXR_KIND_MSG", "__EXR_KIND_OFF", "__EXR_SA", "__exr_err_num", "__exr_panic_index", "__EXR_CALLER"]);

    }
    if user_src.contains("process::exit") {
        need.insert("process_exit");
    }
    if user_src.contains("dbg!") {
        need.extend(FMT_NAMES.iter().copied());
        need.extend(["eprintln", "Debug", "__exr_eprint"]);
    }
    // 標準入出力
    if user_src.contains("stdin") {
        need.extend(["Stdin", "StdinLock", "StdinLines", "__core_stdin", "IoError", "Result", "Option", "next"]);
    }
    if user_src.contains("stdout") || need.contains("BufWriter") {
        need.extend(["Stdout", "StdoutLock", "__core_stdout", "IoError", "Result", "write_str", "flush", "as_str"]);
    }
    if user_src.contains("try_into") {
        need.extend(["TryFromIntError", "Result"]);
    }
    if open_range(user_src) {
        need.insert("RangeFrom");
    }
    // `{:e}` / `{:.3E}`: 指数表記
    if fmt_uses_exp(user_src) {
        need.extend(["__exr_fmt_exp", "__exr_fmt_exp_int", "__exr_udiv128", "__exr_fmt_float"]);
    }
    // `x.into()` は期待型の from を呼ぶ
    if user_src.contains(".into()") {
        need.insert("from");
    }
    if user_src.contains("isqrt") {
        need.insert("__exr_isqrt");
    }
    // 128 bit 整数: 除算と書式化
    if need.contains("i128") || need.contains("u128") {
        need.extend(["__exr_udiv128", "__exr_sdiv128", "__exr_fmt_int128", "__exr_u128_to_f", "__exr_f_to_u128"]);
    }
    // 浮動小数点: 書式化・powi・libm の宣言 (f64 / f32 の名前か、`1.5` のようなリテラルがあるとき)
    // (`fn f64(..)` のような関数の名前は数えない: 型の位置 (`: f64` / `as f64` / `f64::` など) だけ)
    let float_named = (need.contains("f64") || need.contains("f32")) && {
        let b = user_src.as_bytes();
        ["f64", "f32"].iter().any(|w| {
            user_src.match_indices(w).any(|(i, _)| {
                let after = b.get(i + 3).copied().unwrap_or(b' ');
                let before_ident = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
                let after_ident = after.is_ascii_alphanumeric() || after == b'_';
                let mut j = i;
                while j > 0 && b[j - 1] == b' ' {
                    j -= 1;
                }
                let after_fn = j >= 2 && &b[j - 2..j] == b"fn";
                !before_ident && !after_ident && after != b'(' && !after_fn
            })
        })
    };
    let float_lit = !float_named && {
        let b = user_src.as_bytes();
        // (文字列リテラルの中 (`"{:8.3}"` など) は数えない)
        let mut in_str = false;
        let mut found = false;
        for i in 1..b.len().saturating_sub(1) {
            match b[i] {
                b'"' if b[i - 1] != b'\\' && b[i - 1] != b'\'' => in_str = !in_str,
                b'.' if !in_str && b[i - 1].is_ascii_digit() && b[i + 1].is_ascii_digit() => {
                    found = true;
                    break;
                }
                _ => {}
            }
        }
        found
    };
    if float_named || float_lit {
        need.extend(["__exr_fmt_float", "__exr_powi", "__exr_powif", "__exr_ordering"]);
        // (`f64::max` などのパス: 名前の後ろが一致する wrapper)
        for (a, bb) in [("f64", "__exr_f64_"), ("f32", "__exr_f32_")] {
            if user_src.contains(&format!("{a}::")) {
                for (_, name, _) in items() {
                    if let Some(n) = name.as_deref() {
                        if let Some(m) = n.strip_prefix(bb) {
                            if user_src.contains(&format!("{a}::{m}")) {
                                need.insert(n);
                            }
                        }
                    }
                }
            }
        }
        for (_, name, _) in items() {
            if let Some(n) = name.as_deref() {
                if n.starts_with("__exr_") && matches!(n.trim_start_matches("__exr_").trim_end_matches('f'),
                    "fmod" | "pow" | "exp" | "exp2" | "log" | "log10" | "log2" | "sin" | "cos" | "tan" | "asin" | "acos"
                    | "atan" | "atan2" | "sinh" | "cosh" | "tanh" | "asinh" | "acosh" | "atanh" | "cbrt" | "hypot"
                    | "round" | "expm1" | "log1p")
                {
                    need.insert(n);
                }
            }
        }
    }
    // `x.extend(arg)`: codegen が arg を into_iter / iter にし、要素が参照なら __extend_ref を呼ぶ
    if need.contains("extend") {
        need.extend(["__extend_ref", "into_iter", "iter", "Iterator"]);
    }
    // (アダプタの列があれば、その元 (Range など) の next も要る)
    if chain {
        need.insert("next");
    }
    // `Box<dyn Error>` と `impl Error for E {}`、`fn main() -> Result<..>`
    if user_src.contains("dyn Error") || user_src.contains("error::Error") || user_src.contains("Error for") {
        need.extend(["__DynError", "__exr_dyn_error", "__err_mark", "String", "format", "Display", "Debug"]);
        need.extend(FMT_NAMES.iter().copied());
    }
    if user_src.contains("fn main() -> Result") {
        need.extend(["__exr_main_err", "process_exit", "eprintln", "__exr_eprint", "Debug"]);
        need.extend(FMT_NAMES.iter().copied());
    }
    // (`impl IntoIterator for &Coll` の into_iter はイテレータを返す)
    if user_src.contains("IntoIterator for") {
        need.extend(["__into_iter_mark", "Iterator", "next", "iter"]);
    }
    let iter_lib = need.contains("Iterator")
        || need.contains("HashMap")
        || need.contains("BTreeMap")
        || need.contains("BTreeSet")
        || need.contains("VecDeque")
        || ["split", "split_whitespace", "lines"].iter().any(|n| need.contains(n))
        || need.contains("chars")
        || chain
        || (producer && need.contains("next"));
    // 大小比較: `<` は識別子として現れないので、比較を使いそうな名前 (Ord / sort / cmp など) で glue を付ける
    // (イテレータの max / min も中で `>=` を使う)
    const ORD_NAMES: &[&str] = &[
        "Ord", "PartialOrd", "Ordering", "cmp", "partial_cmp", "sort", "sort_by", "sort_by_key",
        "sort_unstable", "sort_unstable_by", "sort_unstable_by_key", "max_by_key", "min_by_key",
        "max_by", "min_by", "Reverse", "clamp", "binary_search", "binary_search_by",
        "binary_search_by_key", "BinaryHeap", "sort_by_cached_key",
    ];
    // (prelude の Reverse も PartialOrd / Ord を impl する)
    let user_impls = user_impl_traits(user_src);
    let cmp_impl = user_impls.iter().any(|t| matches!(*t, "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Clone" | "Copy" | "Default"))
        || need.contains("Reverse");
    let op_impls: Vec<&str> = user_impls.iter().copied().filter(|t| OP_TRAITS.contains(t)).collect();
    if op_impls.contains(&"IntoIterator") {
        need.extend(["__into_iter_mark", "Iterator", "next", "iter"]);
    }
    // (再帰する型は Box / Vec を通るので、そのときは glue を付ける。それ以外の型の比較は codegen が直接展開する)
    if ["Box", "Vec", "vec", "String"].iter().any(|n| need.contains(n)) {
        need.insert("__exr_cmp");
    }
    if cmp_impl {
        need.extend(["eq", "cmp", "partial_cmp", "__exr_eq_marker", "Ordering", "__exr_copy_marker"]);
    }
    // (trait の名前は prelude の名前の候補にないので、derive(Ord) はソースの文字列で見る)
    if ORD_NAMES.iter().any(|n| need.contains(n))
        || (iter_lib && (need.contains("max") || need.contains("min")))
        || (user_src.contains("Ord)") || user_src.contains("Ord,") || user_src.contains("Ord for"))
            && (need.contains("max") || need.contains("min") || need.contains("clamp"))
    {
        need.insert("__exr_cmp");
        // (手書きの impl を glue が呼ぶときに Ordering を i32 にする)
        if cmp_impl || need.contains("Ordering") {
            need.extend(["__exr_ord_i32", "__exr_pord_i32"]);
        }
        if need.contains("Reverse") {
            need.extend(["cmp", "partial_cmp"]);
        }
        if need.contains("cmp") || need.contains("partial_cmp") {
            need.extend(["__exr_cmp_m", "__exr_pcmp_m"]);
        }
        if need.contains("max") || need.contains("min") {
            need.extend(["__exr_max", "__exr_min"]);
        }
        if need.contains("clamp") {
            need.insert("__exr_clamp");
        }
        // 配列・スライスのメソッドとして呼ぶとき用 (Vec のメソッドは名前から付く)
        for (m, f) in [
            ("sort", "__exr_slice_sort"),
            ("sort_unstable", "__exr_slice_sort"),
            ("sort_by", "__exr_slice_sort_by"),
            ("sort_unstable_by", "__exr_slice_sort_by"),
            ("sort_by_key", "__exr_slice_sort_by_key"),
            ("sort_unstable_by_key", "__exr_slice_sort_by_key"),
            ("sort_by_cached_key", "__exr_slice_sort_by_cached_key"),

            ("binary_search", "__exr_slice_bsearch"),
            ("binary_search_by", "__exr_slice_bsearch_by"),
            ("binary_search_by_key", "__exr_slice_bsearch_by_key"),
        ] {
            if need.contains(m) {
                need.insert(f);
            }
        }
    }
    if iter_lib {
        need.insert("Iterator");
        if need.contains("iter") {
            need.insert("__exr_slice_iter");
        }
        if need.contains("iter_mut") {
            need.insert("__exr_slice_iter_mut");
        }
        if need.contains("chars") {
            need.insert("__exr_str_chars");
        }
    }
    let mut chosen = vec![false; all.len()];
    // (書式つきの panic は書式化を使うプログラムだけ: String と Vec を引き込むため)
    if !user_src.contains("no_std") && need.contains("__exr_formatter") {
        need.insert("__exr_panic_fmt");
    }
    loop {
        let mut changed = false;
        for (i, (it, name, core_only)) in all.iter().enumerate() {
            if chosen[i] || matches!(it.kind, ItemKind::ImplHeader | ItemKind::TraitHeader) {
                continue;
            }
            // Iterator の (デフォルト) メソッドは Iterator を使うときだけ (`a.max(b)` の max では引かない)
            if let ItemKind::TraitMember(h) = it.kind {
                if !iter_lib && is_iterator_trait(h as usize) {
                    continue;
                }
                // 比較の trait の宣言は、ユーザが手で impl するときだけ (derive と境界は宣言なしで扱う)
                if !cmp_impl && is_cmp_trait(h as usize) {
                    continue;
                }
                if is_op_trait(h as usize)
                    && !all[h as usize].0.idents.iter().any(|&(a, b)| op_impls.contains(&&PRELUDE[a as usize..b as usize]))
                {
                    continue;
                }
            }
            // `impl Iterator for X` のメソッドと、イテレータを作るメソッドも同じ
            if let ItemKind::ImplMember(h) = it.kind {
                if !iter_lib
                    && (is_iterator_trait(h as usize)
                        || matches!(name.as_deref(), Some("iter" | "iter_mut" | "into_iter" | "chars" | "bytes")))
                {
                    continue;
                }
            }
            // Vec / String のメソッドは、その型の名前が要るときだけ (`push` だけでは引かない)。
            // `impl Drop for Vec<T>` の drop は、Vec が要るなら名前が現れなくても要る
            let mut drop_member = false;
            if let ItemKind::ImplMember(h) = it.kind {
                if let Some(o) = owner(h as usize) {
                    if !need.contains(o) {
                        continue;
                    }
                    // (`==` は識別子として現れないので eq も)
                    drop_member = name.as_deref() == Some("eq")
                        || all[h as usize]
                            .0
                            .idents
                            .iter()
                            .any(|&(a, b)| &PRELUDE[a as usize..b as usize] == "Drop");
                }
            }
            // core::cmp::max などは `core` / `std` という識別子 (パスか use) がなければ使えない
            // (`drop(x)` は prelude の関数として使える)
            if *core_only
                && !(need.contains("core") || need.contains("std"))
                && name.as_deref() != Some("drop")
            {
                continue;
            }
            // バリアント名 (Some など) で enum が要る
            let wanted = drop_member || match name {
                Some(n) => need.contains(n.as_str()),
                None => false,
            } || (name.as_deref() == Some("Option")
                && (need.contains("Some") || need.contains("None")))
                || (name.as_deref() == Some("Result")
                    && (need.contains("Ok") || need.contains("Err")))
                || (name.as_deref() == Some(BOX_ALLOC) && need.contains("Box"))
                // 文字列の < (sort / max など)
                || (name.as_deref() == Some("__exr_str_cmp")
                    && (need.contains("sort") || need.contains("String") || need.contains("max") || need.contains("min")))
                // 範囲を値として使うかもしれない (`(0..n).map(..)`)
                || (matches!(name.as_deref(), Some("Range") | Some("RangeInclusive"))
                    && need.contains("Iterator"))
                || (matches!(name.as_deref(), Some(BOX_FREE) | Some(DROP_GLUE))
                    && (need.contains("Box") || need.contains("Drop")))
                || (name.as_deref().is_some_and(|n| n.starts_with("__exr_fnptr"))
                    && need.contains("dyn"));
            if wanted {
                chosen[i] = true;
                changed = true;
                add_idents(&mut need, it);
                if let ItemKind::ImplMember(h) | ItemKind::TraitMember(h) = it.kind {
                    if !chosen[h as usize] {
                        chosen[h as usize] = true;
                        let hit = &all[h as usize].0;
                        add_idents(&mut need, hit);
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut out = String::new();
    let mut open_impl = false;
    for (i, (it, _, _)) in all.iter().enumerate() {
        let in_impl = matches!(it.kind, ItemKind::ImplMember(_) | ItemKind::TraitMember(_));
        if open_impl && !in_impl {
            out.push_str("}\n");
            open_impl = false;
        }
        // impl の関連型 (`type Item = &T;`) は、その impl が付くなら必ず付ける
        let assoc_member = matches!(it.kind, ItemKind::ImplMember(h) if chosen[h as usize])
            && it
                .idents
                .first()
                .is_some_and(|&(a, b)| &PRELUDE[a as usize..b as usize] == "type");
        if !chosen[i] && !assoc_member {
            continue;
        }
        out.push_str(&PRELUDE[it.start..it.end]);
        out.push('\n');
        if matches!(it.kind, ItemKind::ImplHeader | ItemKind::TraitHeader) {
            open_impl = true;
        }
    }
    if open_impl {
        out.push_str("}\n");
    }
    out
}
