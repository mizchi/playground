//! 組み込みの prelude。core の代わりに、よく使う型をこのコンパイラが扱えるサブセットで書いたもの。
//! ユーザのソースの後ろに付けて一緒にコンパイルする (使われたものだけがコード生成される)。

/// `Box::new` が呼ぶアロケータ (libc の malloc)
pub const BOX_ALLOC: &str = "__exr_malloc";
/// Box の drop が呼ぶ解放関数 (libc の free)
pub const BOX_FREE: &str = "__exr_free";
/// drop glue: `__exr_drop_in_place::<T>` の本体は codegen が型ごとに作る
pub const DROP_GLUE: &str = "__exr_drop_in_place";

pub const PRELUDE: &str = r#"
pub struct PanicInfo {
    _private: u8,
}

unsafe extern "C" {
    #[link_name = "malloc"]
    fn __exr_malloc(size: usize) -> *mut u8;
}
unsafe extern "C" {
    #[link_name = "free"]
    fn __exr_free(p: *mut u8);
}
fn __exr_drop_in_place<T>(p: *mut T) {}
pub trait Drop {
    fn drop(&mut self);
}
pub fn __core_drop<T>(_x: T) {}

// 関数ポインタを `dyn Fn` にするときの vtable の中身 (データは関数ポインタを置いた場所)
fn __exr_fnptr0<R>(f: &fn() -> R) -> R {
    (*f)()
}
fn __exr_fnptr1<A, R>(f: &fn(A) -> R, a: A) -> R {
    (*f)(a)
}
fn __exr_fnptr2<A, B, R>(f: &fn(A, B) -> R, a: A, b: B) -> R {
    (*f)(a, b)
}
fn __exr_fnptr3<A, B, C, R>(f: &fn(A, B, C) -> R, a: A, b: B, c: C) -> R {
    (*f)(a, b, c)
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
    // 中身を drop せずに入れ替える (生ポインタの read / write は drop しない)
    pub fn take(&mut self) -> Option<T> {
        let p = self as *mut Option<T>;
        unsafe {
            let v = p.read();
            p.write(None);
            v
        }
    }
    pub fn replace(&mut self, value: T) -> Option<T> {
        let p = self as *mut Option<T>;
        unsafe {
            let v = p.read();
            p.write(Some(value));
            v
        }
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
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Option<U> {
        match self {
            Some(v) => Some(f(v)),
            None => None,
        }
    }
    pub fn and_then<U, F: FnOnce(T) -> Option<U>>(self, f: F) -> Option<U> {
        match self {
            Some(v) => f(v),
            None => None,
        }
    }
    pub fn unwrap_or_else<F: FnOnce() -> T>(self, f: F) -> T {
        match self {
            Some(v) => v,
            None => f(),
        }
    }
    pub fn map_or<U, F: FnOnce(T) -> U>(self, default: U, f: F) -> U {
        match self {
            Some(v) => f(v),
            None => default,
        }
    }
    pub fn is_some_and<F: FnOnce(T) -> bool>(self, f: F) -> bool {
        match self {
            Some(v) => f(v),
            None => false,
        }
    }
    pub fn filter<P: FnOnce(&T) -> bool>(self, p: P) -> Option<T> {
        match self {
            Some(v) => {
                if p(&v) {
                    Some(v)
                } else {
                    None
                }
            }
            None => None,
        }
    }
    pub fn ok_or_else<E, F: FnOnce() -> E>(self, f: F) -> Result<T, E> {
        match self {
            Some(v) => Ok(v),
            None => Err(f()),
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
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Result<U, E> {
        match self {
            Ok(v) => Ok(f(v)),
            Err(e) => Err(e),
        }
    }
    pub fn map_err<G, F: FnOnce(E) -> G>(self, f: F) -> Result<T, G> {
        match self {
            Ok(v) => Ok(v),
            Err(e) => Err(f(e)),
        }
    }
    pub fn and_then<U, F: FnOnce(T) -> Result<U, E>>(self, f: F) -> Result<U, E> {
        match self {
            Ok(v) => f(v),
            Err(e) => Err(e),
        }
    }
    pub fn unwrap_or_else<F: FnOnce(E) -> T>(self, f: F) -> T {
        match self {
            Ok(v) => v,
            Err(e) => f(e),
        }
    }
}
// --- Vec<T> / String (alloc の代わり。libc の realloc / free を使う) -------------------------

unsafe extern "C" {
    #[link_name = "realloc"]
    fn __exr_realloc(p: *mut u8, size: usize) -> *mut u8;
}

pub struct Vec<T> {
    ptr: *mut T,
    cap: usize,
    len: usize,
}

impl<T> Vec<T> {
    pub fn new() -> Vec<T> {
        Vec {
            ptr: core::ptr::null_mut(),
            cap: 0,
            len: 0,
        }
    }
    pub fn with_capacity(n: usize) -> Vec<T> {
        let mut v = Vec::new();
        v.reserve(n);
        v
    }
    pub fn reserve(&mut self, extra: usize) {
        let need = self.len + extra;
        if need <= self.cap {
            return;
        }
        let mut c = if self.cap == 0 { 4 } else { self.cap * 2 };
        if c < need {
            c = need;
        }
        let sz = core::mem::size_of::<T>();
        let bytes = if sz == 0 { 1 } else { c * sz };
        self.ptr = unsafe { __exr_realloc(self.ptr as *mut u8, bytes) } as *mut T;
        self.cap = c;
    }
    pub fn push(&mut self, x: T) {
        if self.len == self.cap {
            self.reserve(1);
        }
        unsafe {
            self.ptr.add(self.len).write(x);
        }
        self.len += 1;
    }
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            None
        } else {
            self.len -= 1;
            unsafe { Some(self.ptr.add(self.len).read()) }
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn capacity(&self) -> usize {
        self.cap
    }
    pub fn truncate(&mut self, n: usize) {
        while self.len > n {
            self.len -= 1;
            unsafe {
                __exr_drop_in_place(self.ptr.add(self.len));
            }
        }
    }
    pub fn clear(&mut self) {
        self.truncate(0);
    }
    pub fn get(&self, i: usize) -> Option<&T> {
        if i < self.len {
            unsafe { Some(&*self.ptr.add(i)) }
        } else {
            None
        }
    }
    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        if i < self.len {
            unsafe { Some(&mut *self.ptr.add(i)) }
        } else {
            None
        }
    }
    pub fn first(&self) -> Option<&T> {
        self.get(0)
    }
    pub fn last(&self) -> Option<&T> {
        if self.len == 0 { None } else { self.get(self.len - 1) }
    }
    pub fn insert(&mut self, i: usize, x: T) {
        if i > self.len {
            panic!("insertion index is out of bounds");
        }
        if self.len == self.cap {
            self.reserve(1);
        }
        let mut j = self.len;
        while j > i {
            unsafe {
                self.ptr.add(j).write(self.ptr.add(j - 1).read());
            }
            j -= 1;
        }
        unsafe {
            self.ptr.add(i).write(x);
        }
        self.len += 1;
    }
    pub fn remove(&mut self, i: usize) -> T {
        if i >= self.len {
            panic!("removal index is out of bounds");
        }
        let x = unsafe { self.ptr.add(i).read() };
        let mut j = i;
        while j + 1 < self.len {
            unsafe {
                self.ptr.add(j).write(self.ptr.add(j + 1).read());
            }
            j += 1;
        }
        self.len -= 1;
        x
    }
    pub fn swap_remove(&mut self, i: usize) -> T {
        if i >= self.len {
            panic!("swap_remove index is out of bounds");
        }
        let x = unsafe { self.ptr.add(i).read() };
        self.len -= 1;
        if i != self.len {
            unsafe {
                self.ptr.add(i).write(self.ptr.add(self.len).read());
            }
        }
        x
    }
    pub fn swap(&mut self, a: usize, b: usize) {
        if a >= self.len || b >= self.len {
            panic!("index out of bounds");
        }
        unsafe {
            let x = self.ptr.add(a).read();
            let y = self.ptr.add(b).read();
            self.ptr.add(a).write(y);
            self.ptr.add(b).write(x);
        }
    }
    pub fn reverse(&mut self) {
        let mut i = 0;
        while i + 1 < self.len - i {
            let j = self.len - 1 - i;
            self.swap(i, j);
            i += 1;
        }
    }
    pub fn append(&mut self, other: &mut Vec<T>) {
        self.reserve(other.len);
        let mut i = 0;
        while i < other.len {
            unsafe {
                self.ptr.add(self.len + i).write(other.ptr.add(i).read());
            }
            i += 1;
        }
        self.len += other.len;
        other.len = 0;
    }
    pub fn as_slice(&self) -> &[T] {
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl<T: Copy> Vec<T> {
    pub fn extend_from_slice(&mut self, xs: &[T]) {
        self.reserve(xs.len());
        for &x in xs {
            self.push(x);
        }
    }
    pub fn clone(&self) -> Vec<T> {
        let mut v = Vec::with_capacity(self.len);
        v.extend_from_slice(self.as_slice());
        v
    }
}

impl<T: PartialEq> Vec<T> {
    pub fn contains(&self, x: &T) -> bool {
        let mut i = 0;
        while i < self.len {
            if unsafe { *self.ptr.add(i) == *x } {
                return true;
            }
            i += 1;
        }
        false
    }
    pub fn eq(&self, other: &Vec<T>) -> bool {
        if self.len != other.len {
            return false;
        }
        let mut i = 0;
        while i < self.len {
            if unsafe { *self.ptr.add(i) != *other.ptr.add(i) } {
                return false;
            }
            i += 1;
        }
        true
    }
}

impl<T: Ord> Vec<T> {
    // 挿入ソート (安定)
    pub fn sort(&mut self) {
        let mut i = 1;
        while i < self.len {
            let mut j = i;
            while j > 0 && unsafe { *self.ptr.add(j) < *self.ptr.add(j - 1) } {
                self.swap(j, j - 1);
                j -= 1;
            }
            i += 1;
        }
    }
}

impl<T> Drop for Vec<T> {
    fn drop(&mut self) {
        self.clear();
        if !self.ptr.is_null() {
            unsafe {
                __exr_free(self.ptr as *mut u8);
            }
        }
    }
}

pub struct String {
    vec: Vec<u8>,
}

impl String {
    pub fn new() -> String {
        String { vec: Vec::new() }
    }
    pub fn with_capacity(n: usize) -> String {
        String {
            vec: Vec::with_capacity(n),
        }
    }
    pub fn from(s: &str) -> String {
        let mut r = String::with_capacity(s.len());
        r.push_str(s);
        r
    }
    pub fn push_str(&mut self, s: &str) {
        self.vec.extend_from_slice(s.as_bytes());
    }
    // UTF-8 に符号化して足す
    pub fn push(&mut self, c: char) {
        let c = c as u32;
        if c < 0x80 {
            self.vec.push(c as u8);
        } else if c < 0x800 {
            self.vec.push(0xC0 | (c >> 6) as u8);
            self.vec.push(0x80 | (c & 0x3F) as u8);
        } else if c < 0x10000 {
            self.vec.push(0xE0 | (c >> 12) as u8);
            self.vec.push(0x80 | ((c >> 6) & 0x3F) as u8);
            self.vec.push(0x80 | (c & 0x3F) as u8);
        } else {
            self.vec.push(0xF0 | (c >> 18) as u8);
            self.vec.push(0x80 | ((c >> 12) & 0x3F) as u8);
            self.vec.push(0x80 | ((c >> 6) & 0x3F) as u8);
            self.vec.push(0x80 | (c & 0x3F) as u8);
        }
    }
    pub fn len(&self) -> usize {
        self.vec.len()
    }
    pub fn is_empty(&self) -> bool {
        self.vec.is_empty()
    }
    pub fn capacity(&self) -> usize {
        self.vec.capacity()
    }
    pub fn clear(&mut self) {
        self.vec.clear();
    }
    pub fn truncate(&mut self, n: usize) {
        self.vec.truncate(n);
    }
    pub fn as_bytes(&self) -> &[u8] {
        self.vec.as_slice()
    }
    pub fn as_str(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(self.vec.as_slice()) }
    }
    pub fn clone(&self) -> String {
        String {
            vec: self.vec.clone(),
        }
    }
    pub fn eq(&self, other: &String) -> bool {
        self.vec.eq(&other.vec)
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
                    .find(|w| matches!(*w, "Vec" | "String"))
            })
            .collect()
    })[h]
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
        h.extend([
            "Some", "None", "Ok", "Err", "core", "std", "Box", "dyn", "Drop", "vec", "to_string",
            "to_owned",
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
    for it in user_items {
        for &(a, b) in &it.idents {
            let cands = &by_first[bucket(bytes[a as usize], (b - a) as usize)];
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
    // `vec![..]` は Vec::with_capacity + push に、`"..".to_string()` は String::from になる
    if need.contains("vec") {
        need.extend(["Vec", "with_capacity", "push"]);
    }
    if need.contains("to_string") || need.contains("to_owned") {
        need.extend(["String", "from"]);
    }
    let mut chosen = vec![false; all.len()];
    loop {
        let mut changed = false;
        for (i, (it, name, core_only)) in all.iter().enumerate() {
            if chosen[i] || matches!(it.kind, ItemKind::ImplHeader | ItemKind::TraitHeader) {
                continue;
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
                || (matches!(name.as_deref(), Some(BOX_FREE) | Some(DROP_GLUE))
                    && (need.contains("Box") || need.contains("Drop")))
                || (name.as_deref().is_some_and(|n| n.starts_with("__exr_fnptr"))
                    && need.contains("dyn"));
            if wanted {
                chosen[i] = true;
                changed = true;
                need.extend(
                    it.idents
                        .iter()
                        .map(|&(a, b)| &PRELUDE[a as usize..b as usize]),
                );
                if let ItemKind::ImplMember(h) | ItemKind::TraitMember(h) = it.kind {
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
