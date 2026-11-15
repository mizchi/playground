// dyn Trait のサンプル: &dyn / &mut dyn、デフォルトメソッド、スライス・struct・戻り値の中の dyn、
// プリミティブ型への実装、ジェネリックな struct の実装、&dyn Fn
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
trait Shape {
    fn area(&self) -> i64;
    fn name(&self) -> u8;
    fn scaled(&self, k: i64) -> i64 {
        self.area() * k
    }
}
struct Rect {
    w: i64,
    h: i64,
}
struct Sq(i64);
impl Shape for Rect {
    fn area(&self) -> i64 {
        self.w * self.h
    }
    fn name(&self) -> u8 {
        b'R'
    }
}
impl Shape for Sq {
    fn area(&self) -> i64 {
        self.0 * self.0
    }
    fn name(&self) -> u8 {
        b'S'
    }
    fn scaled(&self, k: i64) -> i64 {
        self.0 * k
    }
}
trait Counter {
    fn bump(&mut self) -> i64;
}
impl Counter for i64 {
    fn bump(&mut self) -> i64 {
        *self += 1;
        *self
    }
}
fn total(xs: &[&dyn Shape]) -> i64 {
    let mut s = 0;
    for x in xs {
        put(x.name());
        s += x.area();
    }
    put(b'\n');
    s
}
fn pick(big: bool) -> &'static dyn Shape {
    if big { &Rect { w: 10, h: 10 } } else { &Sq(2) }
}
fn bump_twice(c: &mut dyn Counter) -> i64 {
    c.bump();
    c.bump()
}
struct Holder<'a> {
    s: &'a dyn Shape,
    f: &'a dyn Fn(i64) -> i64,
}
struct Wrap<T> {
    v: T,
}
impl<T: Copy> Shape for Wrap<T> {
    fn area(&self) -> i64 {
        99
    }
    fn name(&self) -> u8 {
        b'W'
    }
}
trait Visitor {
    fn visit(&mut self, x: i64);
    fn result(&self) -> i64;
}
struct SumV {
    s: i64,
}
struct MaxV {
    m: i64,
}
impl Visitor for SumV {
    fn visit(&mut self, x: i64) {
        self.s += x;
    }
    fn result(&self) -> i64 {
        self.s
    }
}
impl Visitor for MaxV {
    fn visit(&mut self, x: i64) {
        if x > self.m {
            self.m = x;
        }
    }
    fn result(&self) -> i64 {
        self.m
    }
}
fn walk(xs: &[i64], v: &mut dyn Visitor) -> i64 {
    for &x in xs {
        v.visit(x);
    }
    v.result()
}
fn main() {
    let r = Rect { w: 3, h: 4 };
    let q = Sq(5);
    line(total(&[&r, &q, &Sq(1)]));
    let d: &dyn Shape = &q;
    line(d.scaled(3));
    let d2: &dyn Shape = &r;
    line(d2.scaled(3));
    line(pick(true).area() + pick(false).area());
    let mut n: i64 = 40;
    line(bump_twice(&mut n));
    line(n);
    let k = 7;
    let add = |x: i64| x + k;
    let h = Holder { s: &r, f: &add };
    line((h.f)(h.s.area()));
    let w = Wrap { v: 1u8 };
    line(total(&[&w, &r]) + w.v as i64);
    let xs = [3, 9, 2, 7];
    let mut sv = SumV { s: 0 };
    let mut mv = MaxV { m: 0 };
    line(walk(&xs, &mut sv));
    line(walk(&xs, &mut mv));
    let vs: [&dyn Visitor; 2] = [&sv, &mv];
    let mut t = 0;
    for v in vs {
        t += v.result();
    }
    line(t);
}
