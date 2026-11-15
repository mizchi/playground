// impl とメソッドのサンプル (rustc でもそのままコンパイルできる)
unsafe extern "C" {
    fn putchar(c: i32) -> i32;
}

fn put(c: u8) {
    unsafe {
        putchar(c as i32);
    }
}

fn println_i64(n: i64) {
    print_i64(n);
    put(b'\n');
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

#[derive(Clone, Copy)]
struct Vec2 {
    x: i64,
    y: i64,
}

impl Vec2 {
    fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }

    fn zero() -> Vec2 {
        Vec2::new(0, 0)
    }

    fn add(self, o: Vec2) -> Vec2 {
        Vec2 { x: self.x + o.x, y: self.y + o.y }
    }

    fn dot(&self, o: &Vec2) -> i64 {
        self.x * o.x + self.y * o.y
    }

    fn len2(&self) -> i64 {
        self.dot(self)
    }

    fn scale(&mut self, k: i64) {
        self.x *= k;
        self.y *= k;
    }

    // self を書き換えても呼び出し元には影響しない (値渡し)
    fn doubled(mut self) -> Self {
        self.scale(2);
        self
    }
}

struct Counter {
    n: u32,
    step: u32,
}

impl Counter {
    fn new(step: u32) -> Counter {
        Counter { n: 0, step }
    }
    fn tick(&mut self) -> u32 {
        self.n += self.step;
        self.n
    }
    fn get(&self) -> u32 {
        self.n
    }
}

struct Body {
    pos: Vec2,
    vel: Vec2,
}

// impl ブロックは複数に分けてもよい
impl Body {
    fn step(&mut self) {
        self.pos = self.pos.add(self.vel);
    }
}

impl Body {
    fn energy(&self) -> i64 {
        self.vel.len2()
    }
    fn pos_ref(&self) -> &Vec2 {
        &self.pos
    }
}

fn tick_twice(c: &mut Counter) {
    c.tick();
    c.tick();
}

fn main() {
    let a = Vec2::new(1, 2);
    let b = Vec2 { x: 3, y: 4 };
    println_i64(a.dot(&b)); // 11
    println_i64(a.add(b).len2()); // 16 + 36 = 52
    let mut c = Vec2::zero().add(b);
    c.scale(10);
    println_i64(c.x + c.y); // 70
    let d = c.doubled();
    println_i64(d.x); // 60
    println_i64(c.x); // 30

    let mut k = Counter::new(3);
    k.tick();
    tick_twice(&mut k);
    println_i64(k.get() as i64); // 9
    let kr = &mut k;
    kr.tick();
    println_i64(kr.get() as i64); // 12

    let mut body = Body { pos: Vec2::new(0, 0), vel: Vec2::new(2, -1) };
    let mut i = 0;
    while i < 5 {
        body.step();
        i += 1;
    }
    println_i64(body.pos.x); // 10
    println_i64(body.pos.y); // -5
    println_i64(body.energy()); // 5
    println_i64(body.pos_ref().len2()); // 125
    println_i64(Vec2::dot(&a, &a)); // 5 (UFCS)
    let br = &&body;
    println_i64(br.vel.x + br.energy()); // 7
}
