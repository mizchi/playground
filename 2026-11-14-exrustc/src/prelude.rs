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
// --- Iterator ------------------------------------------------------------------------------
// (このファイルは exrustc だけがコンパイルする。アダプタの struct には、関連型を決めるための
//  使わない型パラメータを持たせている: `Map<I, F, B>` の B など)

pub trait Iterator {
    type Item;
    fn next(&mut self) -> Option<Self::Item>;
    fn map<B, F: FnMut(Self::Item) -> B>(self, f: F) -> Map<Self, F, B> {
        Map { iter: self, f: f }
    }
    fn filter<P: FnMut(&Self::Item) -> bool>(self, p: P) -> Filter<Self, P> {
        Filter { iter: self, p: p }
    }
    fn filter_map<B, F: FnMut(Self::Item) -> Option<B>>(self, f: F) -> FilterMap<Self, F, B> {
        FilterMap { iter: self, f: f }
    }
    fn enumerate(self) -> Enumerate<Self> {
        Enumerate { iter: self, n: 0 }
    }
    fn take(self, n: usize) -> Take<Self> {
        Take { iter: self, n: n }
    }
    fn skip(self, n: usize) -> Skip<Self> {
        Skip { iter: self, n: n }
    }
    fn take_while<P: FnMut(&Self::Item) -> bool>(self, p: P) -> TakeWhile<Self, P> {
        TakeWhile { iter: self, p: p, done: false }
    }
    fn zip<U: Iterator>(self, other: U) -> Zip<Self, U> {
        Zip { a: self, b: other }
    }
    fn chain<U: Iterator>(self, other: U) -> Chain<Self, U> {
        Chain { a: self, b: other, first: true }
    }
    fn copied<T>(self) -> Copied<Self, T>
    where
        Self: Iterator<Item = &T>,
    {
        Copied { iter: self }
    }
    fn cloned<T>(self) -> Copied<Self, T>
    where
        Self: Iterator<Item = &T>,
    {
        Copied { iter: self }
    }
    fn count(self) -> usize {
        let mut it = self;
        let mut n = 0;
        while let Some(_) = it.next() {
            n += 1;
        }
        n
    }
    // (Item = S の制約は型推論の手がかり: `let s: i64 = (1..=10).sum();` の 1 を i64 にする)
    fn sum<S>(self) -> S
    where
        Self: Iterator<Item = S>,
    {
        let mut it = self;
        let mut acc: S = 0;
        while let Some(x) = it.next() {
            acc = acc + x;
        }
        acc
    }
    fn product<S>(self) -> S
    where
        Self: Iterator<Item = S>,
    {
        let mut it = self;
        let mut acc: S = 1;
        while let Some(x) = it.next() {
            acc = acc * x;
        }
        acc
    }
    fn fold<B, F: FnMut(B, Self::Item) -> B>(self, init: B, f: F) -> B {
        let mut it = self;
        let mut f = f;
        let mut acc = init;
        while let Some(x) = it.next() {
            acc = f(acc, x);
        }
        acc
    }
    fn for_each<F: FnMut(Self::Item)>(self, f: F) {
        let mut it = self;
        let mut f = f;
        while let Some(x) = it.next() {
            f(x);
        }
    }
    fn any<F: FnMut(Self::Item) -> bool>(&mut self, f: F) -> bool {
        let mut f = f;
        while let Some(x) = self.next() {
            if f(x) {
                return true;
            }
        }
        false
    }
    fn all<F: FnMut(Self::Item) -> bool>(&mut self, f: F) -> bool {
        let mut f = f;
        while let Some(x) = self.next() {
            if !f(x) {
                return false;
            }
        }
        true
    }
    fn find<P: FnMut(&Self::Item) -> bool>(&mut self, p: P) -> Option<Self::Item> {
        let mut p = p;
        while let Some(x) = self.next() {
            if p(&x) {
                return Some(x);
            }
        }
        None
    }
    fn position<P: FnMut(Self::Item) -> bool>(&mut self, p: P) -> Option<usize> {
        let mut p = p;
        let mut i = 0;
        while let Some(x) = self.next() {
            if p(x) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
    fn last(self) -> Option<Self::Item> {
        let mut it = self;
        let mut r = None;
        while let Some(x) = it.next() {
            r = Some(x);
        }
        r
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        let mut i = 0;
        while let Some(x) = self.next() {
            if i == n {
                return Some(x);
            }
            i += 1;
        }
        None
    }
    fn max(self) -> Option<Self::Item> {
        let mut it = self;
        let mut r = it.next();
        while let Some(x) = it.next() {
            let bigger = match &r {
                Some(m) => x >= *m,
                None => true,
            };
            if bigger {
                r = Some(x);
            }
        }
        r
    }
    fn min(self) -> Option<Self::Item> {
        let mut it = self;
        let mut r = it.next();
        while let Some(x) = it.next() {
            let smaller = match &r {
                Some(m) => x < *m,
                None => true,
            };
            if smaller {
                r = Some(x);
            }
        }
        r
    }
    fn collect<C: __FromIter>(self) -> C {
        <C as __FromIter>::__from_iter(self)
    }
}

pub trait __FromIter {
    fn __from_iter<I: Iterator>(it: I) -> Self;
}

impl<T> __FromIter for Vec<T> {
    fn __from_iter<I: Iterator>(it: I) -> Vec<T> {
        let mut it = it;
        let mut v = Vec::new();
        while let Some(x) = it.next() {
            v.push(x);
        }
        v
    }
}

impl __FromIter for String {
    fn __from_iter<I: Iterator>(it: I) -> String {
        let mut it = it;
        let mut s = String::new();
        while let Some(c) = it.next() {
            s.push(c);
        }
        s
    }
}

pub struct Map<I, F, B> {
    iter: I,
    f: F,
}

impl<I: Iterator, F: FnMut(I::Item) -> B, B> Iterator for Map<I, F, B> {
    type Item = B;
    fn next(&mut self) -> Option<B> {
        match self.iter.next() {
            Some(x) => Some((self.f)(x)),
            None => None,
        }
    }
}

pub struct Filter<I, P> {
    iter: I,
    p: P,
}

impl<I: Iterator, P: FnMut(&I::Item) -> bool> Iterator for Filter<I, P> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        while let Some(x) = self.iter.next() {
            if (self.p)(&x) {
                return Some(x);
            }
        }
        None
    }
}

pub struct FilterMap<I, F, B> {
    iter: I,
    f: F,
}

impl<I: Iterator, F: FnMut(I::Item) -> Option<B>, B> Iterator for FilterMap<I, F, B> {
    type Item = B;
    fn next(&mut self) -> Option<B> {
        while let Some(x) = self.iter.next() {
            if let Some(y) = (self.f)(x) {
                return Some(y);
            }
        }
        None
    }
}

pub struct Enumerate<I> {
    iter: I,
    n: usize,
}

impl<I: Iterator> Iterator for Enumerate<I> {
    type Item = (usize, I::Item);
    fn next(&mut self) -> Option<(usize, I::Item)> {
        match self.iter.next() {
            Some(x) => {
                let i = self.n;
                self.n += 1;
                Some((i, x))
            }
            None => None,
        }
    }
}

pub struct Take<I> {
    iter: I,
    n: usize,
}

impl<I: Iterator> Iterator for Take<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        if self.n == 0 {
            return None;
        }
        self.n -= 1;
        self.iter.next()
    }
}

pub struct Skip<I> {
    iter: I,
    n: usize,
}

impl<I: Iterator> Iterator for Skip<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        while self.n > 0 {
            self.n -= 1;
            if self.iter.next().is_none() {
                return None;
            }
        }
        self.iter.next()
    }
}

pub struct TakeWhile<I, P> {
    iter: I,
    p: P,
    done: bool,
}

impl<I: Iterator, P: FnMut(&I::Item) -> bool> Iterator for TakeWhile<I, P> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        if self.done {
            return None;
        }
        match self.iter.next() {
            Some(x) => {
                if (self.p)(&x) {
                    Some(x)
                } else {
                    self.done = true;
                    None
                }
            }
            None => None,
        }
    }
}

pub struct Zip<A, B> {
    a: A,
    b: B,
}

impl<A: Iterator, B: Iterator> Iterator for Zip<A, B> {
    type Item = (A::Item, B::Item);
    fn next(&mut self) -> Option<(A::Item, B::Item)> {
        match self.a.next() {
            Some(x) => match self.b.next() {
                Some(y) => Some((x, y)),
                None => None,
            },
            None => None,
        }
    }
}

pub struct Chain<A, B> {
    a: A,
    b: B,
    first: bool,
}

impl<A: Iterator, B: Iterator> Iterator for Chain<A, B> {
    type Item = A::Item;
    fn next(&mut self) -> Option<A::Item> {
        if self.first {
            match self.a.next() {
                Some(x) => return Some(x),
                None => self.first = false,
            }
        }
        self.b.next()
    }
}

pub struct Copied<I, T> {
    iter: I,
}

impl<I: Iterator, T: Copy> Iterator for Copied<I, T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        match self.iter.next() {
            Some(x) => Some(*x),
            None => None,
        }
    }
}

// `a..b` / `a..=b` を値として使ったとき (for で直接回すときは codegen が数える)
pub struct Range<T> {
    pub start: T,
    pub end: T,
}

impl<T: Copy + Ord> Iterator for Range<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.start < self.end {
            let v = self.start;
            self.start = v + 1;
            Some(v)
        } else {
            None
        }
    }
}

pub struct RangeInclusive<T> {
    pub start: T,
    pub end: T,
    pub done: bool,
}

impl<T: Copy + Ord> Iterator for RangeInclusive<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.done || self.start > self.end {
            return None;
        }
        let v = self.start;
        if v == self.end {
            self.done = true;
        } else {
            self.start = v + 1;
        }
        Some(v)
    }
}

// スライス・配列・Vec の iter() / iter_mut() / into_iter()
pub struct Iter<T> {
    ptr: *const T,
    len: usize,
    i: usize,
}

impl<T> Iterator for Iter<T> {
    type Item = &T;
    fn next(&mut self) -> Option<&T> {
        if self.i < self.len {
            let p = unsafe { &*self.ptr.add(self.i) };
            self.i += 1;
            Some(p)
        } else {
            None
        }
    }
}

pub struct IterMut<T> {
    ptr: *mut T,
    len: usize,
    i: usize,
}

impl<T> Iterator for IterMut<T> {
    type Item = &mut T;
    fn next(&mut self) -> Option<&mut T> {
        if self.i < self.len {
            let p = unsafe { &mut *self.ptr.add(self.i) };
            self.i += 1;
            Some(p)
        } else {
            None
        }
    }
}

fn __exr_slice_iter<T>(s: &[T]) -> Iter<T> {
    Iter {
        ptr: s.as_ptr(),
        len: s.len(),
        i: 0,
    }
}

fn __exr_slice_iter_mut<T>(s: &mut [T]) -> IterMut<T> {
    IterMut {
        ptr: s.as_mut_ptr(),
        len: s.len(),
        i: 0,
    }
}

pub struct IntoIter<T> {
    v: Vec<T>,
    i: usize,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.i < self.v.len {
            let x = unsafe { self.v.ptr.add(self.i).read() };
            self.i += 1;
            Some(x)
        } else {
            None
        }
    }
}

impl<T> Drop for IntoIter<T> {
    // 取り出していない要素を drop する (バッファは Vec の drop が解放する)
    fn drop(&mut self) {
        while self.i < self.v.len {
            unsafe {
                __exr_drop_in_place(self.v.ptr.add(self.i));
            }
            self.i += 1;
        }
        self.v.len = 0;
    }
}

impl<T> Vec<T> {
    pub fn iter(&self) -> Iter<T> {
        Iter {
            ptr: self.ptr as *const T,
            len: self.len,
            i: 0,
        }
    }
    pub fn iter_mut(&mut self) -> IterMut<T> {
        IterMut {
            ptr: self.ptr,
            len: self.len,
            i: 0,
        }
    }
    pub fn into_iter(self) -> IntoIter<T> {
        IntoIter { v: self, i: 0 }
    }
}

impl String {
    pub fn chars(&self) -> Chars {
        Chars {
            ptr: self.vec.ptr as *const u8,
            len: self.vec.len,
            i: 0,
        }
    }
    pub fn bytes(&self) -> Copied<Iter<u8>, u8> {
        Copied {
            iter: __exr_slice_iter(self.as_bytes()),
        }
    }
}

// UTF-8 を char に復号しながら回す
pub struct Chars {
    ptr: *const u8,
    len: usize,
    i: usize,
}

impl Iterator for Chars {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        if self.i >= self.len {
            return None;
        }
        let b0 = unsafe { *self.ptr.add(self.i) } as u32;
        let (c, n) = if b0 < 0x80 {
            (b0, 1)
        } else if b0 < 0xE0 {
            ((b0 & 0x1F) << 6 | (unsafe { *self.ptr.add(self.i + 1) } as u32 & 0x3F), 2)
        } else if b0 < 0xF0 {
            (
                (b0 & 0x0F) << 12
                    | (unsafe { *self.ptr.add(self.i + 1) } as u32 & 0x3F) << 6
                    | (unsafe { *self.ptr.add(self.i + 2) } as u32 & 0x3F),
                3,
            )
        } else {
            (
                (b0 & 0x07) << 18
                    | (unsafe { *self.ptr.add(self.i + 1) } as u32 & 0x3F) << 12
                    | (unsafe { *self.ptr.add(self.i + 2) } as u32 & 0x3F) << 6
                    | (unsafe { *self.ptr.add(self.i + 3) } as u32 & 0x3F),
                4,
            )
        };
        self.i += n;
        Some(unsafe { __exr_char_from_u32(c) })
    }
}

fn __exr_str_chars(s: &str) -> Chars {
    Chars {
        ptr: s.as_ptr(),
        len: s.len(),
        i: 0,
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

/// これらの名前が現れたら Iterator を使っているとみなす
const ITER_NAMES: &[&str] = &[
    "Iterator", "next", "map", "filter", "filter_map", "collect", "sum", "product", "count",
    "fold", "for_each", "any", "all", "find", "position", "nth", "zip", "chain", "take", "skip",
    "take_while", "copied", "cloned", "into_iter",
];

/// prelude のアイテムの識別子を need に足す (Iterator のメソッド名はメソッド呼び出しの位置だけ)
fn add_idents(need: &mut HashSet<&'static str>, it: &SrcItem) {
    let bytes = PRELUDE.as_bytes();
    for &(a, b) in &it.idents {
        let w = &PRELUDE[a as usize..b as usize];
        if w != "Iterator" && ITER_NAMES.contains(&w) && !method_call_at(bytes, a as usize, b as usize) {
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
    select_with(user_src, user_items, &mut SelectCache::default())
}

/// アイテムごとの「当たった名前」と「イテレータの列があるか」の記憶 (アイテムのハッシュ → 結果)。
/// LSP / インクリメンタルビルドでは変わっていないアイテムを走査し直さない
#[derive(Default)]
pub struct SelectCache {
    map: crate::fxhash::FxMap<crate::cache::Key, (Vec<&'static str>, bool)>,
}

pub fn select_with(user_src: &str, user_items: &[SrcItem], cache: &mut SelectCache) -> String {
    let all = items();
    // ユーザの識別子のうち、prelude が定義する名前 (と関係する名前) に当たるものだけを集める。
    // 全識別子の集合を作ると大きなプログラムで毎回 ms 単位かかるので、小さい集合に対して引く
    // 先頭の 1 文字ごとの候補 (数個) と直接比べる。ハッシュしないので大きなプログラムでも速い
    static NAMES: OnceLock<Vec<Vec<&'static str>>> = OnceLock::new();
    let by_first = NAMES.get_or_init(|| {
        let mut h: Vec<&'static str> = all.iter().filter_map(|x| x.1.as_deref()).collect();
        h.extend([
            "Some", "None", "Ok", "Err", "core", "std", "Box", "dyn", "Drop", "vec", "to_string",
            "to_owned", "iter", "iter_mut", "chars", "into_iter", "next", "map", "filter",
            "filter_map", "collect", "sum", "product", "count", "fold", "for_each", "any", "all",
            "find", "position", "nth", "zip", "chain", "take", "skip", "take_while", "copied",
            "cloned",
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
                if *n != "Iterator" && ITER_NAMES.contains(n) && !method_call_at(bytes, a as usize, b as usize) {
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
    // Iterator (アダプタ・消費するメソッド) を使うときだけ、イテレータの部品を付ける。
    // `for x in v.iter()` だけなら codegen が直接回すので要らない
    // (イテレータの列は必ず作るもの (iter / into_iter / chars / 範囲 / 自前の impl Iterator) から始まる。
    //  Option::map や自分の型の fold だけでは付けない)
    let producer = ["iter", "iter_mut", "into_iter"].iter().any(|n| need.contains(n));
    let iter_lib = need.contains("Iterator")
        || need.contains("chars")
        || chain
        || (producer && need.contains("next"));
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
