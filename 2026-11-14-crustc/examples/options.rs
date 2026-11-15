// Option / Result / ? / ジェネリックな struct・enum のサンプル (rustc でもそのままコンパイルできる)
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

fn checked_div(a: i64, b: i64) -> Option<i64> {
    if b == 0 { None } else { Some(a / b) }
}

#[derive(Clone, Copy)]
enum MathError {
    DivByZero,
    Negative(i64),
}

fn sqrt_floor(n: i64) -> Result<i64, MathError> {
    if n < 0 {
        return Err(MathError::Negative(n));
    }
    let mut r = 0;
    while (r + 1) * (r + 1) <= n {
        r += 1;
    }
    Ok(r)
}

fn div_then_sqrt(a: i64, b: i64) -> Result<i64, MathError> {
    let q = match checked_div(a, b) {
        Some(q) => q,
        None => return Err(MathError::DivByZero),
    };
    let r = sqrt_floor(q)?;
    Ok(r * 10)
}

fn first_positive(a: i64, b: i64, c: i64) -> Option<i64> {
    let x = checked_div(100, a)?;
    if x > 0 {
        return Some(x);
    }
    if b > 0 { Some(b) } else if c > 0 { Some(c) } else { None }
}

fn code(e: MathError) -> i64 {
    match e {
        MathError::DivByZero => -1,
        MathError::Negative(n) => n,
    }
}

#[derive(Clone, Copy)]
struct Pair<T> {
    a: T,
    b: T,
}

impl<T: Copy> Pair<T> {
    fn swap(self) -> Pair<T> {
        Pair { a: self.b, b: self.a }
    }
    fn first(&self) -> T {
        self.a
    }
}

#[derive(Clone, Copy)]
struct Wrapper<T>(T);

#[derive(Clone, Copy)]
enum Tree<T> {
    Leaf(T),
    Empty,
}

fn leaf_or<T: Copy>(t: Tree<T>, d: T) -> T {
    match t {
        Tree::Leaf(v) => v,
        Tree::Empty => d,
    }
}

fn main() {
    println_i64(checked_div(10, 3).unwrap()); // 3
    println_i64(checked_div(1, 0).unwrap_or(-7)); // -7
    println_i64(if checked_div(1, 0).is_none() { 1 } else { 0 }); // 1
    match div_then_sqrt(81, 1) {
        Ok(v) => println_i64(v), // 90
        Err(e) => println_i64(code(e)),
    }
    match div_then_sqrt(81, 0) {
        Ok(v) => println_i64(v),
        Err(e) => println_i64(code(e)), // -1
    }
    match div_then_sqrt(-8, 1) {
        Ok(v) => println_i64(v),
        Err(e) => println_i64(code(e)), // -8
    }
    println_i64(first_positive(0, 5, 6).unwrap_or(0)); // 0 (? で None)
    println_i64(first_positive(-50, 5, 6).unwrap_or(0)); // 5
    println_i64(first_positive(-50, -1, -1).unwrap_or(-3)); // -3

    let mut best = None;
    let mut i = 1;
    while i <= 5 {
        if i % 2 == 0 {
            best = Some(i * 100);
        }
        i += 1;
    }
    if let Some(b) = best {
        println_i64(b); // 400
    }
    let mut o: Option<i64> = Some(9);
    let t = o.take();
    println_i64(t.unwrap() + if o.is_some() { 1 } else { 0 }); // 9
    let r: Result<i64, MathError> = Ok(5);
    println_i64(r.ok().unwrap_or(0) + if r.is_ok() { 10 } else { 0 }); // 15
    let p = Pair { a: 1, b: 2 }.swap();
    println_i64(p.first() * 10 + p.b); // 21
    let pb = Pair { a: true, b: false };
    println_i64(if pb.swap().first() { 0 } else { 1 }); // 1
    let w = Wrapper(77);
    let Wrapper(inner) = w;
    println_i64(inner + w.0); // 154
    println_i64(leaf_or(Tree::Leaf(4), 0) + leaf_or(Tree::Empty, 30)); // 34
    let nested: Option<Option<i64>> = Some(None);
    let v = match nested {
        Some(Some(x)) => x,
        Some(None) => 2,
        None => 3,
    };
    println_i64(v); // 2
    let x: Option<&i64> = Some(&p.a);
    println_i64(x.copied().unwrap()); // 2
    println_i64(if matches!(best, Some(400)) { 1 } else { 0 }); // 1
    assert!(v == 2);
    assert_eq!(inner, 77);
}
