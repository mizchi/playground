// ast_codegen v1: struct / enum / match / 配列 / スライス / 文字列 / メソッド
#![no_std]
#![no_main]

use core::panic::PanicInfo;

unsafe extern "C" {
    fn write(fd: i32, p: *const u8, n: usize) -> isize;
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

fn puts(s: &str) {
    unsafe {
        write(1, s.as_bytes().as_ptr(), s.len());
    }
}

fn put_i(n: i64) {
    let mut buf = [0u8; 24];
    let mut i = 24;
    let neg = n < 0;
    let mut m: u64 = if neg { (0 - n) as u64 } else { n as u64 };
    loop {
        i -= 1;
        buf[i] = b'0' + (m % 10) as u8;
        m /= 10;
        if m == 0 {
            break;
        }
    }
    if neg {
        i -= 1;
        buf[i] = b'-';
    }
    let s = &buf[i..];
    unsafe {
        write(1, s.as_ptr(), s.len());
    }
}

#[derive(Clone, Copy)]
struct Point {
    x: i64,
    y: i64,
}

impl Point {
    fn new(x: i64, y: i64) -> Point {
        Point { x, y }
    }
    fn add(self, o: Point) -> Point {
        Point { x: self.x + o.x, y: self.y + o.y }
    }
    fn norm1(&self) -> i64 {
        let ax = if self.x < 0 { -self.x } else { self.x };
        let ay = if self.y < 0 { -self.y } else { self.y };
        ax + ay
    }
    fn scale(&mut self, k: i64) {
        self.x *= k;
        self.y *= k;
    }
}

enum Shape {
    Circle(i64),
    Rect { w: i64, h: i64 },
    Empty,
}

fn area(s: &Shape) -> i64 {
    match s {
        Shape::Circle(r) => 3 * r * r,
        Shape::Rect { w, h } => w * h,
        Shape::Empty => 0,
    }
}

enum Op {
    Add,
    Sub,
    Mul,
}

fn apply(op: Op, a: i64, b: i64) -> i64 {
    match op {
        Op::Add => a + b,
        Op::Sub => a - b,
        Op::Mul => a * b,
    }
}

fn sum(xs: &[i64]) -> i64 {
    let mut t = 0;
    for x in xs {
        t += *x;
    }
    t
}

fn classify(n: i64) -> &'static str {
    match n {
        0 => "zero",
        1..=9 => "small",
        10 | 20 | 30 => "round",
        _ => "big",
    }
}

fn divmod(a: i64, b: i64) -> (i64, i64) {
    (a / b, a % b)
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    let mut p = Point::new(3, -4);
    let q = p.add(Point { x: 1, y: 1 });
    put_i(q.x);
    puts(" ");
    put_i(q.y);
    puts(" ");
    put_i(p.norm1());
    puts("\n");
    p.scale(10);
    put_i(p.x + p.y);
    puts("\n");
    let shapes = [Shape::Circle(2), Shape::Rect { w: 3, h: 5 }, Shape::Empty];
    let mut total = 0;
    for s in &shapes {
        total += area(s);
    }
    put_i(total);
    puts("\n");
    put_i(apply(Op::Add, 2, 3) + apply(Op::Sub, 10, 4) * apply(Op::Mul, 2, 2));
    puts("\n");
    let arr = [5i64, 6, 7, 8, 9];
    put_i(sum(&arr));
    puts(" ");
    put_i(sum(&arr[1..3]));
    puts(" ");
    put_i(arr.len() as i64);
    puts("\n");
    for n in [0i64, 5, 20, 77] {
        puts(classify(n));
        puts(" ");
    }
    puts("\n");
    let (d, m) = divmod(17, 5);
    put_i(d * 100 + m);
    puts("\n");
    let mut grid = [[0i64; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            grid[i][j] = (i * 3 + j) as i64;
        }
    }
    put_i(grid[2][1] + grid[1][2]);
    puts("\n");
    let s = "hello";
    if s == "hello" && s != "world" {
        puts("eq ok\n");
    }
    let t = (1i64, Point { x: 7, y: 8 });
    let (a, Point { x, y }) = t;
    put_i(a + x + y);
    puts("\n");
    0
}
