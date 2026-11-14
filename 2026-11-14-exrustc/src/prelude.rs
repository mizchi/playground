//! 組み込みの prelude。core の代わりに、よく使う型をこのコンパイラが扱えるサブセットで書いたもの。
//! ユーザのソースの後ろに付けて一緒にコンパイルする (使われたものだけがコード生成される)。

pub const PRELUDE: &str = r#"
pub struct PanicInfo {
    _private: u8,
}

// core::cmp / core::mem / core::ptr の関数。ユーザが同じ名前の関数を定義してもぶつからないよう
// `__core_` を付けて置き、`core::cmp::max` などのパス (と未定義の名前) からだけ引く
pub fn __core_min<T: Ord + Copy>(a: T, b: T) -> T {
    if b < a { b } else { a }
}
pub fn __core_max<T: Ord + Copy>(a: T, b: T) -> T {
    if b < a { a } else { b }
}
pub fn __core_swap<T: Copy>(a: &mut T, b: &mut T) {
    let t = *a;
    *a = *b;
    *b = t;
}
pub fn __core_replace<T: Copy>(dest: &mut T, src: T) -> T {
    let old = *dest;
    *dest = src;
    old
}
pub unsafe fn __core_read_volatile<T: Copy>(p: *const T) -> T {
    unsafe { *p }
}
pub unsafe fn __core_write_volatile<T: Copy>(p: *mut T, v: T) {
    unsafe {
        *p = v;
    }
}

#[derive(Clone, Copy)]
pub enum Option<T> {
    None,
    Some(T),
}

impl<T> Option<T> {
    pub fn is_some(&self) -> bool {
        match self {
            Some(_) => true,
            None => false,
        }
    }
    pub fn is_none(&self) -> bool {
        match self {
            Some(_) => false,
            None => true,
        }
    }
    pub fn unwrap(self) -> T {
        match self {
            Some(v) => v,
            None => panic!("called `Option::unwrap()` on a `None` value"),
        }
    }
    pub fn unwrap_or(self, default: T) -> T {
        match self {
            Some(v) => v,
            None => default,
        }
    }
    pub fn ok_or<E>(self, err: E) -> Result<T, E> {
        match self {
            Some(v) => Ok(v),
            None => Err(err),
        }
    }
    pub fn or(self, other: Option<T>) -> Option<T> {
        match self {
            Some(v) => Some(v),
            None => other,
        }
    }
    pub fn xor(self, other: Option<T>) -> Option<T> {
        match self {
            Some(v) => match other {
                Some(_) => None,
                None => Some(v),
            },
            None => other,
        }
    }
    pub fn take(&mut self) -> Option<T> {
        let v = *self;
        *self = None;
        v
    }
    pub fn replace(&mut self, value: T) -> Option<T> {
        let v = *self;
        *self = Some(value);
        v
    }
    pub fn as_ref(&self) -> Option<&T> {
        match self {
            Some(v) => Some(v),
            None => None,
        }
    }
    pub fn as_mut(&mut self) -> Option<&mut T> {
        match self {
            Some(v) => Some(v),
            None => None,
        }
    }
}

impl<T> Option<&T> {
    pub fn copied(self) -> Option<T> {
        match self {
            Some(v) => Some(*v),
            None => None,
        }
    }
    pub fn cloned(self) -> Option<T> {
        match self {
            Some(v) => Some(*v),
            None => None,
        }
    }
}

#[derive(Clone, Copy)]
pub enum Result<T, E> {
    Ok(T),
    Err(E),
}

impl<T, E> Result<T, E> {
    pub fn is_ok(&self) -> bool {
        match self {
            Ok(_) => true,
            Err(_) => false,
        }
    }
    pub fn is_err(&self) -> bool {
        match self {
            Ok(_) => false,
            Err(_) => true,
        }
    }
    pub fn unwrap(self) -> T {
        match self {
            Ok(v) => v,
            Err(_) => panic!("called `Result::unwrap()` on an `Err` value"),
        }
    }
    pub fn unwrap_err(self) -> E {
        match self {
            Ok(_) => panic!("called `Result::unwrap_err()` on an `Ok` value"),
            Err(e) => e,
        }
    }
    pub fn unwrap_or(self, default: T) -> T {
        match self {
            Ok(v) => v,
            Err(_) => default,
        }
    }
    pub fn ok(self) -> Option<T> {
        match self {
            Ok(v) => Some(v),
            Err(_) => None,
        }
    }
    pub fn err(self) -> Option<E> {
        match self {
            Ok(_) => None,
            Err(e) => Some(e),
        }
    }
}
"#;

use std::collections::HashSet;
use std::sync::OnceLock;

use crate::lexer::{self, ItemKind, SrcItem};

/// prelude のアイテム (一度だけ字句解析する)。(アイテム, 定義する名前, core:: 経由でしか使えないか)
fn items() -> &'static [(SrcItem, Option<String>, bool)] {
    static ITEMS: OnceLock<Vec<(SrcItem, Option<String>, bool)>> = OnceLock::new();
    ITEMS.get_or_init(|| {
        lexer::split(PRELUDE)
            .expect("prelude must lex")
            .into_iter()
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

/// `fn x` / `struct X` / `enum X` の名前 (`__core_x` は x として引かれる)
fn defined_name(src: &str, it: &SrcItem) -> Option<String> {
    let ids: Vec<&str> = it
        .idents
        .iter()
        .map(|&(a, b)| &src[a as usize..b as usize])
        .collect();
    let k = ids
        .iter()
        .position(|w| matches!(*w, "fn" | "struct" | "enum"))?;
    ids.get(k + 1)
        .map(|s| s.strip_prefix("__core_").unwrap_or(s).to_string())
}

/// ユーザのコードから (識別子を辿って) 使われうる prelude のアイテムだけを、ソースとして返す。
/// Option を使わないプログラムには何も足さないので、キャッシュも大きくならない
pub fn select(user_src: &str, user_items: &[SrcItem]) -> String {
    let all = items();
    // ユーザの識別子のうち、prelude が定義する名前 (と関係する名前) に当たるものだけを集める。
    // 全識別子の集合を作ると大きなプログラムで毎回 ms 単位かかるので、小さい集合に対して引く
    // 先頭の 1 文字ごとの候補 (数個) と直接比べる。ハッシュしないので大きなプログラムでも速い
    static NAMES: OnceLock<Vec<Vec<&'static str>>> = OnceLock::new();
    let by_first = NAMES.get_or_init(|| {
        let mut h: Vec<&'static str> = all.iter().filter_map(|x| x.1.as_deref()).collect();
        h.extend(["Some", "None", "Ok", "Err", "core", "std"]);
        let mut t: Vec<Vec<&'static str>> = vec![vec![]; 256];
        for n in h {
            if !t[n.as_bytes()[0] as usize].contains(&n) {
                t[n.as_bytes()[0] as usize].push(n);
            }
        }
        t
    });
    let mut need: HashSet<&str> = HashSet::new();
    let bytes = user_src.as_bytes();
    for it in user_items {
        for &(a, b) in &it.idents {
            let cands = &by_first[bytes[a as usize] as usize];
            if cands.is_empty() {
                continue;
            }
            let w = &user_src[a as usize..b as usize];
            if let Some(n) = cands.iter().find(|n| **n == w) {
                need.insert(n);
            }
        }
    }
    if need.is_empty() {
        return String::new();
    }
    let mut chosen = vec![false; all.len()];
    loop {
        let mut changed = false;
        for (i, (it, name, core_only)) in all.iter().enumerate() {
            if chosen[i] || matches!(it.kind, ItemKind::ImplHeader | ItemKind::TraitHeader) {
                continue;
            }
            // core::cmp::max などは `core` / `std` という識別子 (パスか use) がなければ使えない
            if *core_only && !(need.contains("core") || need.contains("std")) {
                continue;
            }
            // バリアント名 (Some など) で enum が要る
            let wanted = match name {
                Some(n) => need.contains(n.as_str()),
                None => false,
            } || (name.as_deref() == Some("Option")
                && (need.contains("Some") || need.contains("None")))
                || (name.as_deref() == Some("Result")
                    && (need.contains("Ok") || need.contains("Err")));
            if wanted {
                chosen[i] = true;
                changed = true;
                need.extend(
                    it.idents
                        .iter()
                        .map(|&(a, b)| &PRELUDE[a as usize..b as usize]),
                );
                if let ItemKind::ImplMember(h) = it.kind {
                    if !chosen[h as usize] {
                        chosen[h as usize] = true;
                        let hit = &all[h as usize].0;
                        need.extend(
                            hit.idents
                                .iter()
                                .map(|&(a, b)| &PRELUDE[a as usize..b as usize]),
                        );
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
        let in_impl = matches!(it.kind, ItemKind::ImplMember(_));
        if open_impl && !in_impl {
            out.push_str("}\n");
            open_impl = false;
        }
        if !chosen[i] {
            continue;
        }
        out.push_str(&PRELUDE[it.start..it.end]);
        out.push('\n');
        if matches!(it.kind, ItemKind::ImplHeader) {
            open_impl = true;
        }
    }
    if open_impl {
        out.push_str("}\n");
    }
    out
}
