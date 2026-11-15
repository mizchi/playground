// Drop のサンプル (続き): return / ? での drop、enum の中の Box、ジェネリック関数、値で受ける self、
// Option::replace、FnMut の中の一時値、ブロックから返す Box<dyn Fn>、配列の要素への代入
unsafe extern "C" {
    fn putchar(c: i32) -> i32;
}
fn put(c: u8) {
    unsafe {
        putchar(c as i32);
    }
}
fn num(n: i64) {
    if n < 0 {
        put(b'-');
        num(-n);
        return;
    }
    if n >= 10 {
        num(n / 10);
    }
    put(b'0' + (n % 10) as u8);
}
fn line(n: i64) {
    num(n);
    put(b'\n');
}

struct Noisy(u8);

impl Drop for Noisy {
    fn drop(&mut self) {
        put(b'~');
        put(self.0);
        put(b' ');
    }
}

fn make(c: u8) -> Noisy {
    Noisy(c)
}

fn consume(n: Noisy) {
    put(b'c');
    put(n.0);
    put(b' ');
}

fn early(flag: bool) -> u8 {
    let _a = Noisy(b'p');
    if flag {
        let _b = Noisy(b'q');
        return 1;
    }
    let _c = Noisy(b'r');
    2
}

struct Pair {
    a: Noisy,
    b: Box<Noisy>,
}

struct Guard<T> {
    v: T,
    tag: u8,
}

impl<T> Drop for Guard<T> {
    fn drop(&mut self) {
        put(b'G');
        put(self.tag);
        put(b' ');
    }
}

trait Animal {
    fn name(&self) -> u8;
}

struct Dog(Noisy);

impl Animal for Dog {
    fn name(&self) -> u8 {
        b'D'
    }
}

struct Node {
    v: i64,
    next: Option<Box<Node>>,
}

fn push(list: Option<Box<Node>>, v: i64) -> Option<Box<Node>> {
    Some(Box::new(Node { v, next: list }))
}

fn total(mut cur: &Option<Box<Node>>) -> i64 {
    let mut s = 0;
    while let Some(n) = cur {
        s += n.v;
        cur = &n.next;
    }
    s
}

struct Stack {
    top: Option<Box<Node>>,
    len: i64,
}

impl Stack {
    fn push(&mut self, v: i64) {
        let old = self.top.take();
        self.top = Some(Box::new(Node { v, next: old }));
        self.len += 1;
    }
    fn pop(&mut self) -> Option<i64> {
        match self.top.take() {
            Some(n) => {
                self.top = n.next;
                self.len -= 1;
                Some(n.v)
            }
            None => None,
        }
    }
}

fn find(xs: &[i64], k: i64) -> Option<usize> {
    let _g = Noisy(b'f');
    for (i, &x) in xs.iter().enumerate() {
        if x == k {
            return Some(i);
        }
    }
    None
}
fn try_it(v: Option<i64>) -> Option<i64> {
    let _n = Noisy(b'?');
    let x = v?;
    Some(x + 1)
}
enum Shape {
    Circle(Box<Noisy>),
    Empty,
}
fn wrap<T>(x: T) -> Option<T> {
    Some(x)
}
impl Noisy {
    fn into_char(self) -> u8 {
        self.0
    }
    fn boxed(c: u8) -> Box<Noisy> {
        Box::new(Noisy(c))
    }
    fn get(&self) -> u8 {
        self.0
    }
}
fn main() {
    let xs = [1, 2, 3];
    put(b'0' + find(&xs, 2).unwrap() as u8);
    put(b'\n');
    let a = try_it(None);
    let b = try_it(Some(1));
    put(if a.is_none() { b'N' } else { b'S' });
    put(b'0' + b.unwrap() as u8);
    put(b'\n');
    let s1 = Shape::Circle(Box::new(Noisy(b'C')));
    let s2 = Shape::Empty;
    match &s1 {
        Shape::Circle(n) => put(n.0),
        Shape::Empty => put(b'E'),
    }
    match s2 {
        Shape::Circle(_) => {}
        Shape::Empty => put(b'E'),
    }
    put(b'\n');
    let w = wrap(Noisy(b'W'));
    put(w.map(|n| n.0 + 1).unwrap());
    put(b'\n');
    put(Noisy(b'i').into_char());
    put(b'\n');
    put(Noisy::boxed(b'T').get());
    put(b'\n');
    let mut slot = Some(Box::new(Noisy(b'1')));
    let old = slot.replace(Box::new(Noisy(b'2')));
    put(old.unwrap().0);
    put(b'\n');
    let mut count = 0;
    let mut bump = || {
        let _t = Noisy(b't');
        count += 1;
    };
    bump();
    bump();
    put(b'0' + count);
    put(b'\n');
    let cb: Box<dyn Fn() -> u8> = {
        let n = Noisy(b'q');
        Box::new(move || n.0)
    };
    put(cb());
    put(b'\n');
    let mut arr = [Some(Noisy(b'x')), None, Some(Noisy(b'y'))];
    arr[1] = Some(Noisy(b'z'));
    arr[0] = None;
    put(b'\n');
}
