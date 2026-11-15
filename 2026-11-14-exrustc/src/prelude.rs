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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    pub fn sort(&mut self) {
        __exr_merge_sort(self.ptr, self.len, |a: &T, b: &T| a < b);
    }
    pub fn sort_unstable(&mut self) {
        self.sort();
    }
    pub fn binary_search(&self, x: &T) -> Result<usize, usize> {
        __exr_bsearch_by(self.ptr, self.len, |e: &T| __exr_cmp_m::<T>(e, x))
    }
}

impl<T> Vec<T> {
    pub fn sort_by<F: FnMut(&T, &T) -> Ordering>(&mut self, f: F) {
        let mut f = f;
        __exr_merge_sort(self.ptr, self.len, |a: &T, b: &T| __exr_ord_i32::<()>(&f(a, b)) < 0);
    }
    pub fn sort_unstable_by<F: FnMut(&T, &T) -> Ordering>(&mut self, f: F) {
        self.sort_by(f);
    }
    pub fn sort_by_key<K: Ord, F: FnMut(&T) -> K>(&mut self, f: F) {
        let mut f = f;
        __exr_merge_sort(self.ptr, self.len, |a: &T, b: &T| __exr_cmp::<K>(&f(a), &f(b)) < 0);
    }
    pub fn sort_unstable_by_key<K: Ord, F: FnMut(&T) -> K>(&mut self, f: F) {
        self.sort_by_key(f);
    }
    pub fn binary_search_by<F: FnMut(&T) -> Ordering>(&self, f: F) -> Result<usize, usize> {
        __exr_bsearch_by(self.ptr, self.len, f)
    }
    pub fn binary_search_by_key<B: Ord, F: FnMut(&T) -> B>(&self, b: &B, f: F) -> Result<usize, usize> {
        let mut f = f;
        __exr_bsearch_by(self.ptr, self.len, |e: &T| __exr_cmp_m::<B>(&f(e), b))
    }
}

// 安定なマージソート (下から上へ)。16 個ずつ挿入ソートしてから、作業領域と交互にマージする。
// 要素は read / write でビット移動するので、Drop を持つ型でも二重解放しない
fn __exr_merge_sort<T, F: FnMut(&T, &T) -> bool>(p: *mut T, n: usize, lt: F) {
    let mut lt = lt;
    let sz = core::mem::size_of::<T>();
    if n < 2 || sz == 0 {
        return;
    }
    let run = 16;
    let mut s = 0;
    while s < n {
        let e = if s + run < n { s + run } else { n };
        let mut i = s + 1;
        while i < e {
            unsafe {
                let x = p.add(i).read();
                let mut j = i;
                while j > s && lt(&x, &*p.add(j - 1)) {
                    p.add(j).write(p.add(j - 1).read());
                    j -= 1;
                }
                p.add(j).write(x);
            }
            i += 1;
        }
        s = e;
    }
    if n <= run {
        return;
    }
    let buf = unsafe { __exr_realloc(0usize as *mut u8, n * sz) } as *mut T;
    let mut src = p;
    let mut dst = buf;
    let mut w = run;
    while w < n {
        let mut lo = 0;
        while lo < n {
            let mid = if lo + w < n { lo + w } else { n };
            let hi = if lo + 2 * w < n { lo + 2 * w } else { n };
            let mut i = lo;
            let mut j = mid;
            let mut k = lo;
            unsafe {
                while i < mid && j < hi {
                    // 右が真に小さいときだけ右を先に出す (安定)
                    if lt(&*src.add(j), &*src.add(i)) {
                        dst.add(k).write(src.add(j).read());
                        j += 1;
                    } else {
                        dst.add(k).write(src.add(i).read());
                        i += 1;
                    }
                    k += 1;
                }
                while i < mid {
                    dst.add(k).write(src.add(i).read());
                    i += 1;
                    k += 1;
                }
                while j < hi {
                    dst.add(k).write(src.add(j).read());
                    j += 1;
                    k += 1;
                }
            }
            lo = hi;
        }
        let t = src;
        src = dst;
        dst = t;
        w *= 2;
    }
    if src != p {
        let mut i = 0;
        while i < n {
            unsafe {
                p.add(i).write(src.add(i).read());
            }
            i += 1;
        }
    }
    unsafe {
        __exr_free(buf as *mut u8);
    }
}

// std と同じ二分探索 (同じ値が並ぶときに返す位置も std に合わせる)
fn __exr_bsearch_by<T, F: FnMut(&T) -> Ordering>(p: *mut T, n: usize, f: F) -> Result<usize, usize> {
    let mut f = f;
    let mut size = n;
    if size == 0 {
        return Err(0);
    }
    let mut base = 0;
    while size > 1 {
        let half = size / 2;
        let mid = base + half;
        let c = __exr_ord_i32::<()>(&f(unsafe { &*p.add(mid) }));
        if c <= 0 {
            base = mid;
        }
        size -= half;
    }
    let c = __exr_ord_i32::<()>(&f(unsafe { &*p.add(base) }));
    if c == 0 {
        Ok(base)
    } else if c < 0 {
        Err(base + 1)
    } else {
        Err(base)
    }
}

// 配列・スライスの sort / binary_search (codegen が `a.sort()` をこれにする)
fn __exr_slice_sort<T: Ord>(s: &mut [T]) {
    __exr_merge_sort(s.as_mut_ptr(), s.len(), |a: &T, b: &T| a < b);
}
fn __exr_slice_sort_by<T, F: FnMut(&T, &T) -> Ordering>(s: &mut [T], f: F) {
    let mut f = f;
    __exr_merge_sort(s.as_mut_ptr(), s.len(), |a: &T, b: &T| __exr_ord_i32::<()>(&f(a, b)) < 0);
}
fn __exr_slice_sort_by_key<T, K: Ord, F: FnMut(&T) -> K>(s: &mut [T], f: F) {
    let mut f = f;
    __exr_merge_sort(s.as_mut_ptr(), s.len(), |a: &T, b: &T| __exr_cmp::<K>(&f(a), &f(b)) < 0);
}
fn __exr_slice_bsearch<T: Ord>(s: &[T], x: &T) -> Result<usize, usize> {
    __exr_bsearch_by(s.as_ptr() as *mut T, s.len(), |e: &T| __exr_cmp_m::<T>(e, x))
}
fn __exr_slice_bsearch_by<T, F: FnMut(&T) -> Ordering>(s: &[T], f: F) -> Result<usize, usize> {
    __exr_bsearch_by(s.as_ptr() as *mut T, s.len(), f)
}
fn __exr_slice_bsearch_by_key<T, B: Ord, F: FnMut(&T) -> B>(s: &[T], b: &B, f: F) -> Result<usize, usize> {
    let mut f = f;
    __exr_bsearch_by(s.as_ptr() as *mut T, s.len(), |e: &T| __exr_cmp_m::<B>(&f(e), b))
}

// Ord::clamp (`x.clamp(lo, hi)`。lo > hi なら panic)
fn __exr_clamp<T>(x: T, lo: T, hi: T) -> T {
    if __exr_cmp::<T>(&lo, &hi) > 0 {
        panic!("assertion failed: min <= max");
    }
    if __exr_cmp::<T>(&x, &lo) < 0 {
        lo
    } else if __exr_cmp::<T>(&x, &hi) > 0 {
        hi
    } else {
        x
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
    fn max_by_key<B: Ord, F: FnMut(&Self::Item) -> B>(self, f: F) -> Option<Self::Item> {
        let mut it = self;
        let mut f = f;
        let mut best = match it.next() {
            Some(x) => x,
            None => return None,
        };
        let mut bk = f(&best);
        while let Some(x) = it.next() {
            let k = f(&x);
            if __exr_cmp::<B>(&k, &bk) >= 0 {
                best = x;
                bk = k;
            }
        }
        Some(best)
    }
    fn min_by_key<B: Ord, F: FnMut(&Self::Item) -> B>(self, f: F) -> Option<Self::Item> {
        let mut it = self;
        let mut f = f;
        let mut best = match it.next() {
            Some(x) => x,
            None => return None,
        };
        let mut bk = f(&best);
        while let Some(x) = it.next() {
            let k = f(&x);
            if __exr_cmp::<B>(&k, &bk) < 0 {
                best = x;
                bk = k;
            }
        }
        Some(best)
    }
    fn max_by<F: FnMut(&Self::Item, &Self::Item) -> Ordering>(self, f: F) -> Option<Self::Item> {
        let mut it = self;
        let mut f = f;
        let mut best = match it.next() {
            Some(x) => x,
            None => return None,
        };
        while let Some(x) = it.next() {
            if __exr_ord_i32::<()>(&f(&x, &best)) >= 0 {
                best = x;
            }
        }
        Some(best)
    }
    fn min_by<F: FnMut(&Self::Item, &Self::Item) -> Ordering>(self, f: F) -> Option<Self::Item> {
        let mut it = self;
        let mut f = f;
        let mut best = match it.next() {
            Some(x) => x,
            None => return None,
        };
        while let Some(x) = it.next() {
            if __exr_ord_i32::<()>(&f(&x, &best)) < 0 {
                best = x;
            }
        }
        Some(best)
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

fn __exr_str_chars<G>(s: &str) -> Chars {
    Chars {
        ptr: s.as_ptr(),
        len: s.len(),
        i: 0,
    }
}
// --- 書式化 (format! / println! / write! と Display / Debug) ----------------------------------
// マクロは codegen が展開し、値ごとに `__exr_display::<T>` / `__exr_debug::<T>` (型ごとに codegen が
// 本体を作る) を呼ぶ。数値・文字列の書き出しとパディング、Debug の整形はここの関数が行う

pub struct __FmtError {
    _p: u8,
}

pub struct Formatter {
    buf: &mut String,
    // 書式指定 ({:>8} / {:08.3x} など)
    width: usize,
    fill: char,
    // 0 = 指定なし, 1 = <, 2 = >, 3 = ^
    align: u8,
    zero: bool,
    plus: bool,
    alt: bool,
    prec: usize,
    has_prec: bool,
    // 0 = {} / {:?}, 1 = x, 2 = X, 3 = b, 4 = o
    radix: u8,
    // {:#?} のインデントの深さ
    indent: usize,
}

pub trait Display {
    fn fmt(&self, f: &mut Formatter) -> Result<(), __FmtError>;
}

pub trait Debug {
    fn fmt(&self, f: &mut Formatter) -> Result<(), __FmtError>;
}

impl Formatter {
    pub fn write_str(&mut self, s: &str) -> Result<(), __FmtError> {
        self.buf.push_str(s);
        Ok(())
    }
    pub fn write_char(&mut self, c: char) -> Result<(), __FmtError> {
        self.buf.push(c);
        Ok(())
    }
    pub fn alternate(&self) -> bool {
        self.alt
    }
    pub fn pad(&mut self, s: &str) -> Result<(), __FmtError> {
        __exr_fmt_str::<()>(self, s);
        Ok(())
    }
}

fn __exr_formatter<G>(buf: &mut String) -> Formatter {
    Formatter {
        buf: buf,
        width: 0,
        fill: ' ',
        align: 0,
        zero: false,
        plus: false,
        alt: false,
        prec: 0,
        has_prec: false,
        radix: 0,
        indent: 0,
    }
}

fn __exr_fmt_spec<G>(
    f: &mut Formatter,
    width: usize,
    fill: char,
    align: u8,
    flags: u8,
    prec: usize,
    has_prec: bool,
    radix: u8,
) {
    f.width = width;
    f.fill = fill;
    f.align = align;
    f.zero = flags & 1 != 0;
    f.plus = flags & 2 != 0;
    f.alt = flags & 4 != 0;
    f.prec = prec;
    f.has_prec = has_prec;
    f.radix = radix;
}

fn __exr_fmt_lit<G>(f: &mut Formatter, s: &str) {
    f.buf.push_str(s);
}

// 幅に足りない分を fill で埋める (既定の寄せは def_align)
fn __exr_pad<G>(f: &mut Formatter, s: &str, def_align: u8) {
    // 文字数 (UTF-8 の継続バイトは数えない)
    let mut n = 0;
    for &b in s.as_bytes() {
        if b & 0xC0 != 0x80 {
            n += 1;
        }
    }
    if n >= f.width {
        f.buf.push_str(s);
        return;
    }
    let pad = f.width - n;
    let align = if f.align == 0 { def_align } else { f.align };
    let (l, r) = if align == 1 {
        (0, pad)
    } else if align == 2 {
        (pad, 0)
    } else {
        (pad / 2, pad - pad / 2)
    };
    let mut i = 0;
    while i < l {
        f.buf.push(f.fill);
        i += 1;
    }
    f.buf.push_str(s);
    let mut i = 0;
    while i < r {
        f.buf.push(f.fill);
        i += 1;
    }
}

fn __exr_fmt_str<G>(f: &mut Formatter, s: &str) {
    if f.has_prec {
        // 精度は文字数での切り詰め
        let mut t = String::new();
        let mut n = 0;
        for &b in s.as_bytes() {
            if b & 0xC0 != 0x80 {
                if n == f.prec {
                    break;
                }
                n += 1;
            }
            t.vec.push(b);
        }
        __exr_pad::<()>(f, t.as_str(), 1);
    } else if f.width == 0 {
        f.buf.push_str(s);
    } else {
        __exr_pad::<()>(f, s, 1);
    }
}

fn __exr_fmt_int<G>(f: &mut Formatter, mag: u64, neg: bool) {
    let base: u64 = if f.radix == 1 || f.radix == 2 {
        16
    } else if f.radix == 3 {
        2
    } else if f.radix == 4 {
        8
    } else {
        10
    };
    let mut digits = [0u8; 64];
    let mut n = 0;
    let mut v = mag;
    loop {
        let d = (v % base) as u8;
        digits[n] = if d < 10 {
            b'0' + d
        } else if f.radix == 2 {
            b'A' + d - 10
        } else {
            b'a' + d - 10
        };
        n += 1;
        v /= base;
        if v == 0 {
            break;
        }
    }
    let mut s = String::new();
    if neg {
        s.push('-');
    } else if f.plus {
        s.push('+');
    }
    if f.alt {
        if base == 16 {
            s.push_str("0x");
        } else if base == 2 {
            s.push_str("0b");
        } else if base == 8 {
            s.push_str("0o");
        }
    }
    let prefix = s.len();
    // `{:05}`: 符号と 0x の後ろを 0 で埋める
    if f.zero && f.width > prefix + n {
        let mut z = f.width - prefix - n;
        while z > 0 {
            s.push('0');
            z -= 1;
        }
    }
    while n > 0 {
        n -= 1;
        s.push(digits[n] as char);
    }
    if f.zero {
        f.buf.push_str(s.as_str());
    } else {
        __exr_pad::<()>(f, s.as_str(), 2);
    }
}

fn __exr_fmt_bool<G>(f: &mut Formatter, b: bool) {
    __exr_fmt_str::<()>(f, if b { "true" } else { "false" });
}

fn __exr_fmt_char<G>(f: &mut Formatter, c: char) {
    let mut s = String::new();
    s.push(c);
    __exr_fmt_str::<()>(f, s.as_str());
}

fn __exr_escape<G>(s: &mut String, c: char, quote: char) {
    if c == quote || c == '\\' {
        s.push('\\');
        s.push(c);
    } else if c == '\n' {
        s.push_str("\\n");
    } else if c == '\r' {
        s.push_str("\\r");
    } else if c == '\t' {
        s.push_str("\\t");
    } else if c == '\0' {
        s.push_str("\\0");
    } else {
        s.push(c);
    }
}

fn __exr_fmt_str_debug<G>(f: &mut Formatter, x: &str) {
    let mut s = String::new();
    s.push('"');
    for &b in x.as_bytes() {
        if b < 0x80 {
            __exr_escape::<()>(&mut s, b as char, '"');
        } else {
            s.vec.push(b);
        }
    }
    s.push('"');
    __exr_pad::<()>(f, s.as_str(), 1);
}

fn __exr_fmt_char_debug<G>(f: &mut Formatter, c: char) {
    let mut s = String::new();
    s.push('\'');
    __exr_escape::<()>(&mut s, c, '\'');
    s.push('\'');
    __exr_pad::<()>(f, s.as_str(), 1);
}

fn __exr_indent<G>(f: &mut Formatter) {
    let mut i = 0;
    while i < f.indent {
        f.buf.push_str("    ");
        i += 1;
    }
}

// Debug の整形: kind 0 = `Name { a: .. }`, 1 = `Name(..)` / タプル, 2 = `[..]`, 3 = `{k: v}`
fn __exr_dbg_begin<G>(f: &mut Formatter, name: &str, kind: u8) {
    f.buf.push_str(name);
    let open = if kind == 0 {
        if f.alt { " {" } else { " { " }
    } else if kind == 1 {
        "("
    } else if kind == 2 {
        "["
    } else {
        "{"
    };
    f.buf.push_str(open);
    if f.alt {
        f.buf.push('\n');
        f.indent += 1;
    }
}

fn __exr_dbg_item<G>(f: &mut Formatter, i: usize, key: &str) {
    if f.alt {
        __exr_indent::<()>(f);
    } else if i > 0 {
        f.buf.push_str(", ");
    }
    if !key.is_empty() {
        f.buf.push_str(key);
        f.buf.push_str(": ");
    }
}

fn __exr_dbg_item_end<G>(f: &mut Formatter) {
    if f.alt {
        f.buf.push_str(",\n");
    }
}

fn __exr_dbg_end<G>(f: &mut Formatter, n: usize, kind: u8, tuple: bool) {
    let close = if kind == 0 {
        if f.alt { "}" } else { " }" }
    } else if kind == 1 {
        ")"
    } else if kind == 2 {
        "]"
    } else {
        "}"
    };
    if f.alt {
        f.indent -= 1;
        __exr_indent::<()>(f);
    } else if tuple && n == 1 {
        // `(1,)`
        f.buf.push(',');
    }
    f.buf.push_str(close);
}

// 書式指定の状態を保存して、入れ子の値を既定の書式で書く (`{:>5?}` の中の要素など)
fn __exr_dbg_value_begin(f: &mut Formatter) -> usize {
    let w = f.width;
    f.width = 0;
    w
}

fn __exr_print<G>(s: &String) {
    for &b in s.as_bytes() {
        unsafe {
            __exr_putchar(b as i32);
        }
    }
}

fn __exr_eprint<G>(s: &String) {
    unsafe {
        __exr_write(2, s.vec.ptr as *const u8, s.vec.len);
    }
}

unsafe extern "C" {
    #[link_name = "putchar"]
    fn __exr_putchar(c: i32) -> i32;
    #[link_name = "write"]
    fn __exr_write(fd: i32, p: *const u8, n: usize) -> isize;
}

fn __exr_display<T>(x: &T, f: &mut Formatter) {}
fn __exr_fmt_ok<G>() -> Result<(), __FmtError> {
    Ok(())
}
fn __exr_debug<T>(x: &T, f: &mut Formatter) {}

impl Display for String {
    fn fmt(&self, f: &mut Formatter) -> Result<(), __FmtError> {
        __exr_fmt_str::<()>(f, self.as_str());
        Ok(())
    }
}

impl Debug for String {
    fn fmt(&self, f: &mut Formatter) -> Result<(), __FmtError> {
        __exr_fmt_str_debug::<()>(f, self.as_str());
        Ok(())
    }
}

impl<T> Debug for Vec<T> {
    fn fmt(&self, f: &mut Formatter) -> Result<(), __FmtError> {
        __exr_dbg_begin::<()>(f, "", 2);
        let mut i = 0;
        while i < self.len {
            __exr_dbg_item::<()>(f, i, "");
            unsafe {
                __exr_debug(&*self.ptr.add(i), f);
            }
            __exr_dbg_item_end::<()>(f);
            i += 1;
        }
        if self.len == 0 && f.alt {
            // 空なら `[]`
            f.indent -= 1;
            f.buf.pop_byte();
            f.buf.push(']');
            return Ok(());
        }
        __exr_dbg_end::<()>(f, self.len, 2, false);
        Ok(())
    }
}

impl String {
    fn pop_byte(&mut self) {
        self.vec.len -= 1;
    }
}
// --- 順序 (Ordering / 大小比較の glue。型ごとの比較の本体は codegen が作る __exr_cmp) ---

pub trait PartialEq {
    fn eq(&self, other: &Self) -> bool;
}
pub trait Eq {
    fn __exr_eq_marker(&self) {}
}
pub trait PartialOrd {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering>;
}
pub trait Ord {
    fn cmp(&self, other: &Self) -> Ordering;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ordering {
    Less = -1,
    Equal = 0,
    Greater = 1,
}

impl Ordering {
    pub fn reverse(self) -> Ordering {
        match self {
            Ordering::Less => Ordering::Greater,
            Ordering::Equal => Ordering::Equal,
            Ordering::Greater => Ordering::Less,
        }
    }
    pub fn then(self, o: Ordering) -> Ordering {
        match self {
            Ordering::Equal => o,
            x => x,
        }
    }
    pub fn then_with<F: FnOnce() -> Ordering>(self, f: F) -> Ordering {
        match self {
            Ordering::Equal => f(),
            x => x,
        }
    }
    pub fn is_eq(self) -> bool {
        __exr_ord_i32::<()>(&self) == 0
    }
    pub fn is_ne(self) -> bool {
        __exr_ord_i32::<()>(&self) != 0
    }
    pub fn is_lt(self) -> bool {
        __exr_ord_i32::<()>(&self) < 0
    }
    pub fn is_le(self) -> bool {
        __exr_ord_i32::<()>(&self) <= 0
    }
    pub fn is_gt(self) -> bool {
        __exr_ord_i32::<()>(&self) > 0
    }
    pub fn is_ge(self) -> bool {
        __exr_ord_i32::<()>(&self) >= 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Reverse<T>(pub T);

impl<T> PartialOrd for Reverse<T> {
    fn partial_cmp(&self, o: &Reverse<T>) -> Option<Ordering> {
        Some(__exr_cmp_m::<T>(&o.0, &self.0))
    }
}
impl<T> Ord for Reverse<T> {
    fn cmp(&self, o: &Reverse<T>) -> Ordering {
        __exr_cmp_m::<T>(&o.0, &self.0)
    }
}

// a と b を比べて -1 / 0 / 1
fn __exr_cmp<T>(a: &T, b: &T) -> i32 {
    0
}

fn __exr_ordering<G>(c: i32) -> Ordering {
    if c < 0 {
        Ordering::Less
    } else if c > 0 {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

fn __exr_cmp_m<T>(a: &T, b: &T) -> Ordering {
    __exr_ordering::<()>(__exr_cmp::<T>(a, b))
}

fn __exr_pcmp_m<T>(a: &T, b: &T) -> Option<Ordering> {
    Some(__exr_cmp_m::<T>(a, b))
}

fn __exr_ord_i32<G>(o: &Ordering) -> i32 {
    match *o {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

fn __exr_pord_i32<G>(o: &Option<Ordering>) -> i32 {
    match *o {
        Some(x) => __exr_ord_i32::<()>(&x),
        None => 0,
    }
}

// Ord::max / min (`a.max(b)` の a が整数でないとき)
fn __exr_max<T>(a: T, b: T) -> T {
    if __exr_cmp::<T>(&a, &b) > 0 { a } else { b }
}

fn __exr_min<T>(a: T, b: T) -> T {
    if __exr_cmp::<T>(&a, &b) <= 0 { a } else { b }
}

// --- HashMap<K, V> (オープンアドレス法・線形探索。ハッシュは型ごとに codegen が作る __exr_hash) --

fn __exr_hash<T>(x: &T) -> u64 {
    0
}

fn __exr_hash_bytes<G>(b: &[u8]) -> u64 {
    // FNV-1a
    let mut h: u64 = 0xcbf29ce484222325;
    for &c in b {
        h = (h ^ c as u64).wrapping_mul(0x100000001b3);
    }
    h
}

pub struct HashMap<K, V> {
    // 0 = 空, 1 = 使用中, 2 = 削除済み
    st: *mut u8,
    ks: *mut K,
    vs: *mut V,
    cap: usize,
    len: usize,
    used: usize,
}

impl<K, V> HashMap<K, V> {
    pub fn new() -> HashMap<K, V> {
        HashMap {
            st: core::ptr::null_mut(),
            ks: core::ptr::null_mut(),
            vs: core::ptr::null_mut(),
            cap: 0,
            len: 0,
            used: 0,
        }
    }
    pub fn with_capacity(n: usize) -> HashMap<K, V> {
        let mut m = HashMap::new();
        let mut c = 8;
        while c * 3 < n * 4 {
            c *= 2;
        }
        m.alloc(c);
        m
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn alloc(&mut self, c: usize) {
        let ks = core::mem::size_of::<K>();
        let vs = core::mem::size_of::<V>();
        self.st = unsafe { __exr_realloc(core::ptr::null_mut(), c) };
        self.ks = unsafe { __exr_realloc(core::ptr::null_mut(), if ks == 0 { 1 } else { c * ks }) } as *mut K;
        self.vs = unsafe { __exr_realloc(core::ptr::null_mut(), if vs == 0 { 1 } else { c * vs }) } as *mut V;
        let mut i = 0;
        while i < c {
            unsafe {
                self.st.add(i).write(0);
            }
            i += 1;
        }
        self.cap = c;
        self.len = 0;
        self.used = 0;
    }
    // (見つかった位置, 見つかったか)。見つからなければ入れられる位置
    fn find(&self, k: &K) -> (usize, bool) {
        let mask = self.cap - 1;
        let mut i = (__exr_hash(k) as usize) & mask;
        let mut free = self.cap;
        loop {
            let s = unsafe { *self.st.add(i) };
            if s == 0 {
                return (if free < self.cap { free } else { i }, false);
            }
            if s == 2 {
                if free == self.cap {
                    free = i;
                }
            } else if unsafe { *self.ks.add(i) == *k } {
                return (i, true);
            }
            i = (i + 1) & mask;
        }
    }
    fn grow(&mut self) {
        if self.cap != 0 && (self.used + 1) * 4 <= self.cap * 3 {
            return;
        }
        let (ost, oks, ovs, ocap) = (self.st, self.ks, self.vs, self.cap);
        let nc = if ocap == 0 { 8 } else if self.len * 2 >= ocap { ocap * 2 } else { ocap };
        self.alloc(nc);
        let mut i = 0;
        while i < ocap {
            if unsafe { *ost.add(i) } == 1 {
                let k = unsafe { oks.add(i).read() };
                let v = unsafe { ovs.add(i).read() };
                let (j, _) = self.find(&k);
                unsafe {
                    self.st.add(j).write(1);
                    self.ks.add(j).write(k);
                    self.vs.add(j).write(v);
                }
                self.len += 1;
                self.used += 1;
            }
            i += 1;
        }
        if ocap != 0 {
            unsafe {
                __exr_free(ost);
                __exr_free(oks as *mut u8);
                __exr_free(ovs as *mut u8);
            }
        }
    }
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        self.grow();
        let (i, found) = self.find(&k);
        if found {
            let old = unsafe { self.vs.add(i).read() };
            unsafe {
                self.vs.add(i).write(v);
            }
            drop(k);
            return Some(old);
        }
        unsafe {
            if *self.st.add(i) == 0 {
                self.used += 1;
            }
            self.st.add(i).write(1);
            self.ks.add(i).write(k);
            self.vs.add(i).write(v);
        }
        self.len += 1;
        None
    }
    pub fn get(&self, k: &K) -> Option<&V> {
        if self.len == 0 {
            return None;
        }
        let (i, found) = self.find(k);
        if found {
            unsafe { Some(&*self.vs.add(i)) }
        } else {
            None
        }
    }
    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        if self.len == 0 {
            return None;
        }
        let (i, found) = self.find(k);
        if found {
            unsafe { Some(&mut *self.vs.add(i)) }
        } else {
            None
        }
    }
    pub fn contains_key(&self, k: &K) -> bool {
        if self.len == 0 {
            return false;
        }
        let (_, found) = self.find(k);
        found
    }
    pub fn remove(&mut self, k: &K) -> Option<V> {
        if self.len == 0 {
            return None;
        }
        let (i, found) = self.find(k);
        if !found {
            return None;
        }
        unsafe {
            self.st.add(i).write(2);
            __exr_drop_in_place(self.ks.add(i));
        }
        self.len -= 1;
        unsafe { Some(self.vs.add(i).read()) }
    }
    pub fn clear(&mut self) {
        let mut i = 0;
        while i < self.cap {
            if unsafe { *self.st.add(i) } == 1 {
                unsafe {
                    __exr_drop_in_place(self.ks.add(i));
                    __exr_drop_in_place(self.vs.add(i));
                }
            }
            unsafe {
                self.st.add(i).write(0);
            }
            i += 1;
        }
        self.len = 0;
        self.used = 0;
    }
    pub fn entry(&mut self, k: K) -> Entry<K, V> {
        self.grow();
        let (i, found) = self.find(&k);
        Entry {
            map: self,
            key: Some(k),
            idx: i,
            found: found,
        }
    }
    pub fn iter(&self) -> HmIter<K, V> {
        HmIter {
            st: self.st as *const u8,
            ks: self.ks as *const K,
            vs: self.vs as *const V,
            cap: self.cap,
            i: 0,
        }
    }
    pub fn iter_mut(&mut self) -> HmIterMut<K, V> {
        HmIterMut {
            st: self.st as *const u8,
            ks: self.ks as *const K,
            vs: self.vs,
            cap: self.cap,
            i: 0,
        }
    }
    pub fn keys(&self) -> HmKeys<K, V> {
        HmKeys { it: self.iter() }
    }
    pub fn values(&self) -> HmValues<K, V> {
        HmValues { it: self.iter() }
    }
    pub fn values_mut(&mut self) -> HmValuesMut<K, V> {
        HmValuesMut { it: self.iter_mut() }
    }
    pub fn into_iter(self) -> HmIntoIter<K, V> {
        HmIntoIter { m: self, i: 0 }
    }
}

impl<K, V> Drop for HashMap<K, V> {
    fn drop(&mut self) {
        if self.cap == 0 {
            return;
        }
        self.clear();
        unsafe {
            __exr_free(self.st);
            __exr_free(self.ks as *mut u8);
            __exr_free(self.vs as *mut u8);
        }
    }
}

pub struct Entry<K, V> {
    map: &mut HashMap<K, V>,
    key: Option<K>,
    idx: usize,
    found: bool,
}

impl<K, V> Entry<K, V> {
    pub fn or_insert(self, v: V) -> &mut V {
        let m = self.map;
        let i = self.idx;
        let mut key = self.key;
        if self.found {
            drop(v);
        } else {
            unsafe {
                if *m.st.add(i) == 0 {
                    m.used += 1;
                }
                m.st.add(i).write(1);
                m.ks.add(i).write(key.take().unwrap());
                m.vs.add(i).write(v);
            }
            m.len += 1;
        }
        drop(key);
        unsafe { &mut *m.vs.add(i) }
    }
    pub fn or_insert_with<F: FnOnce() -> V>(self, f: F) -> &mut V {
        if self.found {
            let m = self.map;
            let i = self.idx;
            drop(self.key);
            unsafe { &mut *m.vs.add(i) }
        } else {
            self.or_insert(f())
        }
    }
    pub fn and_modify<F: FnOnce(&mut V)>(self, f: F) -> Entry<K, V> {
        if self.found {
            unsafe {
                f(&mut *self.map.vs.add(self.idx));
            }
        }
        self
    }
}

pub struct HmIter<K, V> {
    st: *const u8,
    ks: *const K,
    vs: *const V,
    cap: usize,
    i: usize,
}

impl<K, V> Iterator for HmIter<K, V> {
    type Item = (&K, &V);
    fn next(&mut self) -> Option<(&K, &V)> {
        while self.i < self.cap {
            let i = self.i;
            self.i += 1;
            if unsafe { *self.st.add(i) } == 1 {
                return unsafe { Some((&*self.ks.add(i), &*self.vs.add(i))) };
            }
        }
        None
    }
}

pub struct HmIterMut<K, V> {
    st: *const u8,
    ks: *const K,
    vs: *mut V,
    cap: usize,
    i: usize,
}

impl<K, V> Iterator for HmIterMut<K, V> {
    type Item = (&K, &mut V);
    fn next(&mut self) -> Option<(&K, &mut V)> {
        while self.i < self.cap {
            let i = self.i;
            self.i += 1;
            if unsafe { *self.st.add(i) } == 1 {
                return unsafe { Some((&*self.ks.add(i), &mut *self.vs.add(i))) };
            }
        }
        None
    }
}

pub struct HmKeys<K, V> {
    it: HmIter<K, V>,
}

impl<K, V> Iterator for HmKeys<K, V> {
    type Item = &K;
    fn next(&mut self) -> Option<&K> {
        match self.it.next() {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
}

pub struct HmValues<K, V> {
    it: HmIter<K, V>,
}

impl<K, V> Iterator for HmValues<K, V> {
    type Item = &V;
    fn next(&mut self) -> Option<&V> {
        match self.it.next() {
            Some((_, v)) => Some(v),
            None => None,
        }
    }
}

pub struct HmValuesMut<K, V> {
    it: HmIterMut<K, V>,
}

impl<K, V> Iterator for HmValuesMut<K, V> {
    type Item = &mut V;
    fn next(&mut self) -> Option<&mut V> {
        match self.it.next() {
            Some((_, v)) => Some(v),
            None => None,
        }
    }
}

pub struct HmIntoIter<K, V> {
    m: HashMap<K, V>,
    i: usize,
}

impl<K, V> Iterator for HmIntoIter<K, V> {
    type Item = (K, V);
    fn next(&mut self) -> Option<(K, V)> {
        while self.i < self.m.cap {
            let i = self.i;
            self.i += 1;
            if unsafe { *self.m.st.add(i) } == 1 {
                unsafe {
                    self.m.st.add(i).write(0);
                }
                self.m.len -= 1;
                return unsafe { Some((self.m.ks.add(i).read(), self.m.vs.add(i).read())) };
            }
        }
        None
    }
}

impl<K, V> Debug for HashMap<K, V> {
    fn fmt(&self, f: &mut Formatter) -> Result<(), __FmtError> {
        __exr_dbg_begin::<()>(f, "", 3);
        let mut n = 0;
        let mut i = 0;
        while i < self.cap {
            if unsafe { *self.st.add(i) } == 1 {
                __exr_dbg_item::<()>(f, n, "");
                unsafe {
                    __exr_debug(&*self.ks.add(i), f);
                    f.buf.push_str(": ");
                    __exr_debug(&*self.vs.add(i), f);
                }
                __exr_dbg_item_end::<()>(f);
                n += 1;
            }
            i += 1;
        }
        __exr_dbg_end::<()>(f, n, 3, false);
        Ok(())
    }
}
impl<K, V> __FromIter for HashMap<K, V> {
    fn __from_iter<I: Iterator>(it: I) -> HashMap<K, V> {
        let mut it = it;
        let mut m = HashMap::new();
        while let Some((k, v)) = it.next() {
            m.insert(k, v);
        }
        m
    }
}

// --- &str の分割・検索 (split / split_whitespace / lines / trim / starts_with / ends_with / contains) ---

// 文字列の大小 (バイト列の辞書順): 負 / 0 / 正
fn __exr_str_cmp<G>(a: &str, b: &str) -> i32 {
    let x = a.as_bytes();
    let y = b.as_bytes();
    let mut i = 0;
    while i < x.len() && i < y.len() {
        if x[i] != y[i] {
            return if x[i] < y[i] { -1 } else { 1 };
        }
        i += 1;
    }
    if x.len() < y.len() {
        -1
    } else if x.len() > y.len() {
        1
    } else {
        0
    }
}

fn __exr_sub<G>(p: *const u8, n: usize) -> &str {
    unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(p, n)) }
}

pub struct Split {
    p: *const u8,
    len: usize,
    i: usize,
    sep: u8,
    // 0 = 区切り文字 1 個, 1 = 空白の連続 (split_whitespace), 2 = 行 (lines)
    mode: u8,
    done: bool,
}

impl Iterator for Split {
    type Item = &str;
    fn next(&mut self) -> Option<&str> {
        if self.mode == 1 {
            while self.i < self.len && __exr_is_space::<()>(unsafe { *self.p.add(self.i) }) {
                self.i += 1;
            }
            if self.i >= self.len {
                return None;
            }
            let st = self.i;
            while self.i < self.len && !__exr_is_space::<()>(unsafe { *self.p.add(self.i) }) {
                self.i += 1;
            }
            return Some(__exr_sub::<()>(unsafe { self.p.add(st) }, self.i - st));
        }
        if self.done {
            return None;
        }
        if self.mode == 2 && self.i >= self.len {
            return None;
        }
        let st = self.i;
        while self.i < self.len && unsafe { *self.p.add(self.i) } != self.sep {
            self.i += 1;
        }
        let mut end = self.i;
        if self.i >= self.len {
            self.done = true;
        } else {
            self.i += 1;
        }
        // lines は末尾の \r も取る
        if self.mode == 2 && end > st && unsafe { *self.p.add(end - 1) } == 13 {
            end -= 1;
        }
        Some(__exr_sub::<()>(unsafe { self.p.add(st) }, end - st))
    }
}

fn __exr_is_space<G>(b: u8) -> bool {
    b == 32 || b == 9 || b == 10 || b == 13 || b == 11 || b == 12
}

fn __exr_str_split<G>(s: &str, c: char) -> Split {
    Split { p: s.as_ptr(), len: s.len(), i: 0, sep: c as u8, mode: 0, done: false }
}

fn __exr_str_split_ws<G>(s: &str) -> Split {
    Split { p: s.as_ptr(), len: s.len(), i: 0, sep: 0, mode: 1, done: false }
}

fn __exr_str_lines<G>(s: &str) -> Split {
    Split { p: s.as_ptr(), len: s.len(), i: 0, sep: 10, mode: 2, done: false }
}

fn __exr_str_trim<G>(s: &str) -> &str {
    let b = s.as_bytes();
    let mut a = 0;
    let mut e = b.len();
    while a < e && __exr_is_space::<()>(b[a]) {
        a += 1;
    }
    while e > a && __exr_is_space::<()>(b[e - 1]) {
        e -= 1;
    }
    __exr_sub::<()>(unsafe { s.as_ptr().add(a) }, e - a)
}

fn __exr_str_find<G>(s: &str, t: &str) -> Option<usize> {
    let a = s.as_bytes();
    let b = t.as_bytes();
    if b.len() > a.len() {
        return None;
    }
    let mut i = 0;
    while i + b.len() <= a.len() {
        let mut j = 0;
        while j < b.len() && a[i + j] == b[j] {
            j += 1;
        }
        if j == b.len() {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn __exr_str_starts_with<G>(s: &str, t: &str) -> bool {
    let a = s.as_bytes();
    let b = t.as_bytes();
    if b.len() > a.len() {
        return false;
    }
    let mut j = 0;
    while j < b.len() {
        if a[j] != b[j] {
            return false;
        }
        j += 1;
    }
    true
}

// char の UTF-8 を p に書いて長さを返す (`s.contains('x')` の char を &str にする)
fn __exr_char_utf8<G>(c: char, p: *mut u8) -> usize {
    let c = c as u32;
    unsafe {
        if c < 0x80 {
            *p = c as u8;
            1
        } else if c < 0x800 {
            *p = 0xC0 | (c >> 6) as u8;
            *p.add(1) = 0x80 | (c & 0x3F) as u8;
            2
        } else if c < 0x10000 {
            *p = 0xE0 | (c >> 12) as u8;
            *p.add(1) = 0x80 | ((c >> 6) & 0x3F) as u8;
            *p.add(2) = 0x80 | (c & 0x3F) as u8;
            3
        } else {
            *p = 0xF0 | (c >> 18) as u8;
            *p.add(1) = 0x80 | ((c >> 12) & 0x3F) as u8;
            *p.add(2) = 0x80 | ((c >> 6) & 0x3F) as u8;
            *p.add(3) = 0x80 | (c & 0x3F) as u8;
            4
        }
    }
}

fn __exr_str_ends_with<G>(s: &str, t: &str) -> bool {
    let a = s.as_bytes();
    let b = t.as_bytes();
    if b.len() > a.len() {
        return false;
    }
    let off = a.len() - b.len();
    let mut j = 0;
    while j < b.len() {
        if a[off + j] != b[j] {
            return false;
        }
        j += 1;
    }
    true
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

/// 比較の trait (PartialEq / Eq / PartialOrd / Ord) の宣言か
fn is_cmp_trait(h: usize) -> bool {
    let it = &items()[h].0;
    matches!(it.kind, ItemKind::TraitHeader)
        && it
            .idents
            .iter()
            .any(|&(a, b)| matches!(&PRELUDE[a as usize..b as usize], "PartialEq" | "Eq" | "PartialOrd" | "Ord"))
}

/// ユーザが比較の trait を手で実装しているか (`impl Ord for S` / `impl<T> PartialEq for W<T>`)
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

/// 書式化で使う prelude の名前
const FMT_NAMES: &[&str] = &[
    "String", "new", "Formatter", "Display", "Debug", "__exr_formatter", "__exr_fmt_spec",
    "__exr_fmt_lit", "__exr_fmt_int", "__exr_fmt_str", "__exr_fmt_bool", "__exr_fmt_char",
    "__exr_fmt_str_debug", "__exr_fmt_char_debug", "__exr_dbg_begin", "__exr_dbg_item",
    "__exr_dbg_item_end", "__exr_dbg_end", "__exr_print", "__exr_eprint", "__exr_display",
    "__exr_debug", "__exr_fmt_ok", "fmt", "__FmtError",
];

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
                    .find_map(|w| match w {
                        "Vec" | "String" | "HashMap" | "Reverse" => Some(w),
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
            "cloned", "format", "print", "println", "eprint", "eprintln", "write", "writeln",
            "split", "split_whitespace", "lines", "trim", "starts_with", "ends_with", "contains",
            "clamp",
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
    if need.is_empty() {
        return String::new();
    }
    // 書式化のマクロ: Formatter と書き出しの関数一式 (codegen が名前を書かずに呼ぶ)
    if ["format", "print", "println", "eprint", "eprintln", "write", "writeln", "fmt", "Formatter"]
        .iter()
        .any(|n| need.contains(n))
        || nonliteral_to_string(user_src)
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
    if need.contains("HashMap") {
        need.extend(["__exr_hash", "__exr_hash_bytes"]);
    }
    let iter_lib = need.contains("Iterator")
        || need.contains("HashMap")
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
        "binary_search_by_key",
    ];
    let cmp_impl = impls_cmp_trait(user_src);
    // (再帰する型は Box / Vec を通るので、そのときは glue を付ける。それ以外の型の比較は codegen が直接展開する)
    if ["Box", "Vec", "vec", "String"].iter().any(|n| need.contains(n)) {
        need.insert("__exr_cmp");
    }
    if cmp_impl {
        need.extend(["eq", "cmp", "partial_cmp", "__exr_eq_marker", "Ordering"]);
    }
    if ORD_NAMES.iter().any(|n| need.contains(n))
        || (iter_lib && (need.contains("max") || need.contains("min")))
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
