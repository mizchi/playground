// 関数ポインタと Box のサンプル: 関数アイテムを値として渡す・返す・struct や配列に入れる、
// ジェネリック関数の関数ポインタ化、Box<T> / Box<dyn Trait> / Box<dyn Fn>、再帰的なデータ構造
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
fn double(x: i64) -> i64 {
    x * 2
}
fn inc(x: i64) -> i64 {
    x + 1
}
fn apply<F: Fn(i64) -> i64>(f: F, x: i64) -> i64 {
    f(x)
}
fn apply_ptr(f: fn(i64) -> i64, x: i64) -> i64 {
    f(x)
}
fn pick(b: bool) -> fn(i64) -> i64 {
    if b { double } else { inc }
}
fn id<T>(x: T) -> T {
    x
}
struct Op {
    name: u8,
    f: fn(i64, i64) -> i64,
}
fn add(a: i64, b: i64) -> i64 {
    a + b
}
fn sub(a: i64, b: i64) -> i64 {
    a - b
}
trait Shape {
    fn area(&self) -> i64;
}
struct Sq(i64);
struct Rect(i64, i64);
impl Shape for Sq {
    fn area(&self) -> i64 {
        self.0 * self.0
    }
}
impl Shape for Rect {
    fn area(&self) -> i64 {
        self.0 * self.1
    }
}
fn make(big: bool) -> Box<dyn Shape> {
    if big { Box::new(Rect(10, 20)) } else { Box::new(Sq(3)) }
}
fn adder(n: i64) -> Box<dyn Fn(i64) -> i64> {
    Box::new(move |x| x + n)
}
struct Node {
    v: i64,
    next: Option<Box<Node>>,
}
fn sum(n: &Node) -> i64 {
    match &n.next {
        Some(m) => n.v + sum(m),
        None => n.v,
    }
}
fn tri(a: i64, b: i64, c: i64) -> i64 {
    a * b + c
}
fn run_all(fs: &[Box<dyn Fn(i64) -> i64>], x: i64) -> i64 {
    let mut s = 0;
    for f in fs {
        s += f(x);
    }
    s
}
fn push_front(list: Option<Box<Node>>, v: i64) -> Option<Box<Node>> {
    Some(Box::new(Node { v, next: list }))
}
fn main() {
    line(apply(double, 5));
    line(apply_ptr(inc, 5));
    line(pick(true)(7) + pick(false)(7));
    let f: fn(i64) -> i64 = double;
    line(f(f(3)));
    let g: fn(i64) -> i64 = id;
    line(g(42));
    let ops = [Op { name: b'+', f: add }, Op { name: b'-', f: sub }];
    for op in &ops {
        put(op.name);
        line((op.f)(10, 3));
    }
    let o = Some(4);
    line(o.map(double).unwrap_or(0));
    let b = Box::new(5i64);
    line(*b + 1);
    let mut c = Box::new(10i64);
    *c += 5;
    line(*c);
    let shapes = [make(true), make(false)];
    let mut t = 0;
    for s in &shapes {
        t += s.area();
    }
    line(t);
    let a = adder(100);
    line(a(1));
    let list = Node { v: 1, next: Some(Box::new(Node { v: 2, next: Some(Box::new(Node { v: 3, next: None })) })) };
    line(sum(&list));
    let bs: Box<dyn Shape> = Box::new(Sq(4));
    line(bs.area());
    let d: &dyn Fn(i64) -> i64 = &double;
    line(d(4));
    let t3: &dyn Fn(i64, i64, i64) -> i64 = &tri;
    line(t3(2, 3, 4));
    let k = 3;
    let fs: [Box<dyn Fn(i64) -> i64>; 3] = [Box::new(double), Box::new(inc), Box::new(move |x| x * k)];
    line(run_all(&fs, 5));
    let mut l = None;
    for i in 1..=4 {
        l = push_front(l, i * 10);
    }
    match &l {
        Some(n) => line(sum(n)),
        None => line(-1),
    }
}
