// trait とジェネリクス (単相化) のサンプル (rustc でもそのままコンパイルできる)
unsafe extern "C" {
    fn putchar(c: i32) -> i32;
}

fn put(c: u8) {
    unsafe {
        putchar(c as i32);
    }
}

fn print_i64(n: i64) {
    if n < 0 {
        put(b'-');
        print_i64(0 - n);
        return;
    }
    if n >= 10 {
        print_i64(n / 10);
    }
    put(b'0' + (n % 10) as u8);
}

fn println_i64(n: i64) {
    print_i64(n);
    put(b'\n');
}

trait Shape {
    fn area(&self) -> i64;
    fn perimeter(&self) -> i64;
    // デフォルトメソッド: 実装する型ごとに単相化される
    fn score(&self) -> i64 {
        self.area() * 10 + self.perimeter()
    }
    fn bigger_than(&self, n: i64) -> bool {
        self.area() > n
    }
}

trait Named {
    fn id(&self) -> i64;
}

#[derive(Clone, Copy)]
struct Square {
    side: i64,
}

#[derive(Clone, Copy)]
struct Rect {
    w: i64,
    h: i64,
}

#[derive(Clone, Copy)]
enum Tri {
    Right(i64, i64),
    Equi(i64),
}

impl Shape for Square {
    fn area(&self) -> i64 {
        self.side * self.side
    }
    fn perimeter(&self) -> i64 {
        4 * self.side
    }
}

impl Shape for Rect {
    fn area(&self) -> i64 {
        self.w * self.h
    }
    fn perimeter(&self) -> i64 {
        2 * (self.w + self.h)
    }
    // デフォルトを上書き
    fn score(&self) -> i64 {
        -1
    }
}

impl Shape for Tri {
    fn area(&self) -> i64 {
        match self {
            Tri::Right(a, b) => *a * *b / 2,
            Tri::Equi(s) => *s * *s * 43 / 100,
        }
    }
    fn perimeter(&self) -> i64 {
        match *self {
            Tri::Right(a, b) => a + b + (a + b) * 7 / 10,
            Tri::Equi(s) => 3 * s,
        }
    }
}

impl Named for Square {
    fn id(&self) -> i64 {
        1
    }
}

impl Named for Rect {
    fn id(&self) -> i64 {
        2
    }
}

impl Rect {
    fn square(s: i64) -> Self {
        Rect { w: s, h: s }
    }
}

// ジェネリック関数: T ごとにインスタンスが作られる
fn total_area<T: Shape>(a: &T, b: &T) -> i64 {
    a.area() + b.area()
}

fn describe<T>(x: &T) -> i64
where
    T: Shape + Named,
{
    x.id() * 1000 + x.score()
}

fn larger<T: Shape + Copy>(a: T, b: T) -> T {
    if a.area() >= b.area() { a } else { b }
}

fn twice<T: Shape>(x: &T) -> i64 {
    // ジェネリック関数から別のジェネリック関数を呼ぶ (インスタンスが連鎖する)
    total_area(x, x)
}

fn pick<T: Copy>(c: bool, a: T, b: T) -> T {
    if c { a } else { b }
}

fn main() {
    let s = Square { side: 3 };
    let r = Rect { w: 2, h: 5 };
    let t = Tri::Right(3, 4);
    let e = Tri::Equi(10);

    println_i64(s.area()); // 9
    println_i64(s.score()); // 90 + 12 = 102
    println_i64(r.score()); // -1 (上書き)
    println_i64(t.score()); // 60 + 11 = 71
    println_i64(e.perimeter()); // 30
    println_i64(if s.bigger_than(8) { 1 } else { 0 }); // 1

    println_i64(total_area(&s, &Square { side: 1 })); // 10
    println_i64(total_area(&t, &e)); // 6 + 43 = 49
    println_i64(describe(&s)); // 1102
    println_i64(describe(&r)); // 1999
    println_i64(larger(r, Rect::square(4)).w); // 4
    println_i64(twice(&e)); // 86
    println_i64(Shape::area(&r)); // 10
    println_i64(<Square as Shape>::perimeter(&s)); // 12
    println_i64(pick::<i64>(false, 7, 8)); // 8
    let q = pick(true, s, Square { side: 9 });
    println_i64(q.side); // 3
}
