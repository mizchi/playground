// 文字列・char のメソッドと演算子のオーバーロードのサンプル: to_uppercase / replace / repeat / split("..") /
// split_once / strip_prefix / char_indices / bytes / parse (ParseIntError の Display / Debug)、
// String の insert / remove / pop / retain / + / +=、&s[a..b]、char の is_alphabetic / to_digit / from_digit、
// impl Add / Sub / Neg / AddAssign / Mul<i32>
use std::ops::{Add, AddAssign, Mul, Neg, Sub};

#[derive(Debug, Clone, Copy, PartialEq)]
struct V2 {
    x: i32,
    y: i32,
}

impl Add for V2 {
    type Output = V2;
    fn add(self, o: V2) -> V2 {
        V2 { x: self.x + o.x, y: self.y + o.y }
    }
}

impl Sub for V2 {
    type Output = V2;
    fn sub(self, o: V2) -> V2 {
        V2 { x: self.x - o.x, y: self.y - o.y }
    }
}

impl Neg for V2 {
    type Output = V2;
    fn neg(self) -> V2 {
        V2 { x: -self.x, y: -self.y }
    }
}

impl Mul<i32> for V2 {
    type Output = V2;
    fn mul(self, k: i32) -> V2 {
        V2 { x: self.x * k, y: self.y * k }
    }
}

impl AddAssign for V2 {
    fn add_assign(&mut self, o: V2) {
        self.x += o.x;
        self.y += o.y;
    }
}

fn main() {
    let s = String::from("Hello, Wörld");
    println!("{} | {} | {}", s.to_uppercase(), s.to_lowercase(), s.replace("l", "L"));
    println!("{} | {} | {}", "ab".repeat(3), s.replacen('l', "_", 2), "a-b-c".replace('-', ""));
    let parts: Vec<&str> = "one::two::three".split("::").collect();
    println!("{:?} {:?} {:?}", parts, "k=v=w".split_once('='), "key: val".split_once(": "));
    println!("{:?} {:?} {:?}", "prefix_rest".strip_prefix("prefix_"), "file.rs".strip_suffix(".rs"), "x".strip_prefix("y"));
    println!("[{}] [{}] {:?}", "  pad  ".trim_start(), "  pad  ".trim_end(), "abcabc".rfind("bc"));
    for (i, c) in "aé😀b".char_indices() {
        print!("{}:{} ", i, c);
    }
    println!();
    let total: u32 = "abc".bytes().map(|b| b as u32).sum();
    println!("{} {}", total, "Hello".eq_ignore_ascii_case("hELLO"));
    // parse
    let n: i32 = "-42".parse().unwrap();
    let big = "300".parse::<u8>();
    let bad = "12a".parse::<i64>();
    let empty = "".parse::<u32>();
    println!("{} {:?} {:?} {:?}", n, big, bad, empty);
    match "99999999999".parse::<i32>() {
        Ok(v) => println!("ok {}", v),
        Err(e) => println!("error: {}", e),
    }
    let nums: Vec<i64> = "1 2 3 40".split_whitespace().map(|w| w.parse().unwrap()).collect();
    println!("{:?} {}", nums, nums.iter().sum::<i64>());
    // String の編集
    let mut t = String::from("hllo");
    t.insert(1, 'e');
    t.insert_str(0, ">> ");
    let r = t.remove(0);
    let p = t.pop();
    t.push('O');
    println!("{} {:?} {:?}", t, r, p);
    let mut u = String::from("a1b2c3");
    u.retain(|c| c.is_alphabetic());
    let w = u.clone() + "-" + &t + "!";
    let mut acc = String::new();
    acc += "x";
    acc += &w;
    println!("{} {} {}", u, w, acc);
    println!("{} {} {}", &s[0..5], &s[7..], &w[..3]);
    // char
    let cs = ['a', 'Z', '7', ' ', 'é', '\n', '_'];
    for c in cs {
        print!(
            "{}{}{}{}{}{} ",
            c.is_alphabetic() as u8,
            c.is_numeric() as u8,
            c.is_alphanumeric() as u8,
            c.is_whitespace() as u8,
            c.is_uppercase() as u8,
            c.is_lowercase() as u8
        );
    }
    println!();
    println!(
        "{:?} {:?} {:?} {:?} {} {}",
        '7'.to_digit(10),
        'f'.to_digit(16),
        'g'.to_digit(16),
        char::from_digit(5, 10),
        'a'.to_uppercase(),
        'Q'.to_lowercase().next().unwrap()
    );
    println!("{} {:?} {} {}", char::from(65u8), char::from_u32(0x263A), 'é'.len_utf8(), 'x'.is_digit(36));
    let up: String = "shout".chars().map(|c| c.to_ascii_uppercase()).collect();
    println!("{}", up);
    // 演算子
    let a = V2 { x: 1, y: 2 };
    let b = V2 { x: 10, y: 20 };
    let mut c = a + b - V2 { x: 1, y: 1 };
    c += a;
    println!("{:?} {:?} {:?}", c, -a, b * 3);
}
