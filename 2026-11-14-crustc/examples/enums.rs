// enum と match のサンプル (rustc でもそのままコンパイルできる)
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

#[derive(Clone, Copy)]
struct Vec2 {
    x: i64,
    y: i64,
}

#[derive(Clone, Copy)]
enum Shape {
    Circle(i64),
    Rect { w: i64, h: i64 },
    Poly(Vec2, Vec2, u8),
    Empty,
}

impl Shape {
    fn area(&self) -> i64 {
        // &self を match すると束縛は参照になる (デフォルト束縛モード)
        match self {
            Shape::Circle(r) => 3 * *r * *r,
            Shape::Rect { w, h } => *w * *h,
            Shape::Poly(a, b, _) => (b.x - a.x) * (b.y - a.y),
            Shape::Empty => 0,
        }
    }

    fn scale(&mut self, k: i64) {
        match self {
            Shape::Circle(r) => *r *= k,
            Shape::Rect { w, h } => {
                *w *= k;
                *h *= k;
            }
            Shape::Poly(a, b, _) => {
                a.x *= k;
                b.x *= k;
            }
            Self::Empty => {}
        }
    }

    fn name_len(self) -> i64 {
        match self {
            Shape::Circle(_) => 6,
            Shape::Rect { .. } => 4,
            Shape::Poly(..) => 4,
            Shape::Empty => 5,
        }
    }
}

#[derive(Clone, Copy)]
enum Color {
    Red = 1,
    Green = 2,
    Blue = 40,
}

#[derive(Clone, Copy)]
enum OptI64 {
    Some(i64),
    None,
}

fn find_even(xs0: i64, xs1: i64, xs2: i64) -> OptI64 {
    if xs0 % 2 == 0 {
        OptI64::Some(xs0)
    } else if xs1 % 2 == 0 {
        OptI64::Some(xs1)
    } else if xs2 % 2 == 0 {
        OptI64::Some(xs2)
    } else {
        OptI64::None
    }
}

fn classify(n: i64) -> i64 {
    match n {
        0 => 100,
        1 | 2 | 3 => 200,
        4..=9 => 300,
        -5..0 => 400,
        x if x % 2 == 0 => 500 + x,
        _ => 600,
    }
}

fn depth(s: &Shape) -> i64 {
    if let Shape::Poly(a, _, n) = s {
        a.y + *n as i64
    } else {
        -1
    }
}

enum Expr {
    Num(i64),
    Neg(i64),
    Add(i64, i64),
}

fn eval(e: Expr) -> i64 {
    match e {
        Expr::Num(n) => n,
        Expr::Neg(n) => 0 - n,
        Expr::Add(a, b) => a + b,
    }
}

fn countdown(mut o: OptI64) -> i64 {
    let mut steps = 0;
    while let OptI64::Some(n) = o {
        steps += 1;
        o = if n > 0 { OptI64::Some(n - 3) } else { OptI64::None };
    }
    steps
}

fn main() {
    let c = Shape::Circle(2);
    let r = Shape::Rect { w: 3, h: 4 };
    let p = Shape::Poly(Vec2 { x: 1, y: 2 }, Vec2 { x: 4, y: 6 }, 7);
    let e = Shape::Empty;
    println_i64(c.area()); // 12
    println_i64(r.area()); // 12
    println_i64(p.area()); // 12
    println_i64(e.area()); // 0

    let mut r2 = r;
    r2.scale(2);
    println_i64(r2.area()); // 48
    println_i64(r.area()); // 12 (コピー)
    let mut p2 = p;
    p2.scale(10);
    println_i64(p2.area()); // (40-10)*(6-2) = 120

    println_i64(c.name_len() + e.name_len()); // 11

    println_i64(Color::Red as i64 + Color::Green as i64 + Color::Blue as i64); // 43
    let col = Color::Green;
    let v = match col {
        Color::Red => 1,
        Color::Green => 2,
        Color::Blue => 3,
    };
    println_i64(v); // 2

    match find_even(3, 5, 8) {
        OptI64::Some(x) => println_i64(x), // 8
        OptI64::None => println_i64(-1),
    }
    if let OptI64::None = find_even(1, 3, 5) {
        println_i64(7); // 7
    }

    let mut i = -6;
    let mut acc = 0;
    while i < 14 {
        acc += classify(i);
        i += 1;
    }
    println_i64(acc);

    println_i64(depth(&p)); // 9
    println_i64(depth(&c)); // -1
    println_i64(eval(Expr::Add(2, 3)) + eval(Expr::Neg(4)) + eval(Expr::Num(10))); // 11
    println_i64(countdown(OptI64::Some(10))); // 10,7,4,1,-2 → 5

    let nested = OptI64::Some(42);
    let got = match nested {
        OptI64::Some(n @ 40..=49) => n * 2,
        OptI64::Some(n) => n,
        OptI64::None => 0,
    };
    println_i64(got); // 84
    let s = Vec2 { x: 5, y: 6 };
    let Vec2 { x, y } = s;
    println_i64(x * y); // 30
}
