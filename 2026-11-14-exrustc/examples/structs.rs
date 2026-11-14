// struct と参照のサンプル (rustc でもそのままコンパイルできる)
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
struct Rect {
    min: Vec2,
    max: Vec2,
    tag: u8,
}

fn vec2(x: i64, y: i64) -> Vec2 {
    Vec2 { x, y }
}

fn add(a: Vec2, b: Vec2) -> Vec2 {
    Vec2 { x: a.x + b.x, y: a.y + b.y }
}

fn dot(a: &Vec2, b: &Vec2) -> i64 {
    a.x * b.x + a.y * b.y
}

fn scale(v: &mut Vec2, k: i64) {
    v.x *= k;
    v.y = v.y * k;
}

fn area(r: &Rect) -> i64 {
    (r.max.x - r.min.x) * (r.max.y - r.min.y)
}

fn grow(r: &mut Rect, d: i64) {
    r.min.x -= d;
    r.min.y -= d;
    let m = &mut r.max;
    m.x += d;
    m.y += d;
}

fn swap(a: &mut i64, b: &mut i64) {
    let t = *a;
    *a = *b;
    *b = t;
}

fn bump(n: &mut u32) {
    *n += 1;
}

// 値渡しした struct は呼び出し側のコピーなので、変更しても呼び出し元に影響しない
fn consume(mut v: Vec2) -> i64 {
    v.x = 1000;
    v.x + v.y
}

fn longest<'a>(a: &'a Vec2, b: &'a Vec2) -> i64 {
    if dot(a, a) > dot(b, b) { a.x } else { b.x }
}

fn main() {
    let a = vec2(1, 2);
    let b = Vec2 { x: 3, y: 4 };
    let mut c = add(a, b);
    println_i64(c.x); // 4
    println_i64(c.y); // 6
    println_i64(dot(&a, &b)); // 11
    scale(&mut c, 3);
    println_i64(c.x + c.y); // 30

    let mut r = Rect { min: vec2(0, 0), max: c, tag: 7 };
    println_i64(area(&r)); // 216
    grow(&mut r, 1);
    println_i64(area(&r)); // 280
    println_i64(r.tag as i64); // 7

    let r2 = r; // コピー
    r.min.x = 100;
    println_i64(r2.min.x); // -1
    println_i64(r.min.x); // 100

    let mut p = 5;
    let mut q = 9;
    swap(&mut p, &mut q);
    println_i64(p * 10 + q); // 95

    let mut count = 0;
    let mut i = 0;
    while i < 10 {
        bump(&mut count);
        i += 1;
    }
    println_i64(count as i64); // 10

    let v = vec2(5, 6);
    println_i64(consume(v)); // 1006
    println_i64(v.x); // 5

    let rr = &r;
    let rrr = &rr;
    println_i64(rrr.max.y); // 13
    println_i64((*rr).min.y); // -1

    let pick = if p > q { a } else { b };
    println_i64(pick.y); // 2 (p=9 > q=5)
    println_i64(longest(&a, &b)); // 3
    println_i64(dot(&vec2(2, 3), &Vec2 { x: 4, y: 5 })); // 23
}
