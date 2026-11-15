// Vec と String のサンプル (prelude の Vec<T> / String は Rust で書いてあり、libc の realloc / free を使う)。
// push / pop / 添字 / スライス / sort / insert / remove / clone / ==、vec![..]、&Vec<T> → &[T]、String::from / push_str / push(char)
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
fn puts(s: &str) {
    for &b in s.as_bytes() {
        put(b);
    }
    put(b'\n');
}
fn sum(xs: &[i64]) -> i64 {
    let mut s = 0;
    for &x in xs {
        s += x;
    }
    s
}
struct Person {
    name: String,
    scores: Vec<i64>,
}

impl Person {
    fn new(name: &str) -> Person {
        Person {
            name: name.to_string(),
            scores: Vec::new(),
        }
    }
    fn best(&self) -> i64 {
        let mut b = 0;
        for &s in &self.scores {
            if s > b {
                b = s;
            }
        }
        b
    }
}

fn range(n: i64) -> Vec<i64> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < n {
        v.push(i);
        i += 1;
    }
    v
}

fn main() {
    let mut v = Vec::new();
    for i in 0..10 {
        v.push(i * i);
    }
    line(v.len() as i64);
    line(v[3]);
    v[3] = 100;
    line(sum(&v));
    line(v.pop().unwrap());
    line(v.len() as i64);
    let w = vec![5, 3, 9, 1];
    let mut s = w.clone();
    s.sort();
    for x in &s {
        num(*x);
        put(b' ');
    }
    put(b'\n');
    line(if s.contains(&9) { 1 } else { 0 });
    s.insert(1, 2);
    line(s.remove(0));
    line(sum(&s[1..3]));
    let mut names: Vec<String> = Vec::new();
    names.push(String::from("hello"));
    names.push("world".to_string());
    let mut msg = String::new();
    for n in &names {
        msg.push_str(n.as_str());
        msg.push(' ');
    }
    msg.push('é');
    puts(&msg);
    line(msg.len() as i64);
    let words = vec![String::from("a"), String::from("bc")];
    let mut lens = 0;
    for w in words {
        lens += w.len();
    }
    line(lens as i64);
    let nested = vec![vec![1, 2], vec![3], vec![]];
    let mut t = 0;
    for row in &nested {
        t += sum(row);
    }
    line(t);
    let z = vec![0u8; 5];
    line(z.len() as i64);
    let mut st: Vec<Box<i64>> = Vec::new();
    st.push(Box::new(7));
    st.push(Box::new(8));
    line(*st[1] + *st[0]);
    let mut e = vec![1, 2, 3, 4, 5];
    e.reverse();
    e.truncate(3);
    for x in e.iter() {
        num(*x);
    }
    put(b'\n');
    line(match e.get(10) { Some(_) => 1, None => 0 });
    line(*e.last().unwrap());
    let eq = vec![1, 2] == vec![1, 2];
    line(eq as i64);
    // 文字列の比較 (String / &str)
    let hi = String::from("hi");
    line((hi == "hi") as i64);
    line((hi.as_str() == "ho") as i64);
    line((hi == hi.clone()) as i64);
    // タプルの Vec、iter_mut、while let + pop
    let mut ps: Vec<(i64, u8)> = Vec::new();
    ps.push((1, 2));
    ps.push((3, 4));
    line(ps[1].0 + ps[0].1 as i64);
    let mut m = vec![1i64, 2, 3];
    for x in m.iter_mut() {
        *x *= 10;
    }
    line(sum(&m));
    while let Some(x) = m.pop() {
        num(x);
        put(b' ');
    }
    put(b'\n');
    // フィールドに String / Vec を持つ struct と、Vec を返す関数
    let mut p = Person::new("ann");
    p.scores.push(7);
    p.scores.push(12);
    p.scores.push(9);
    puts(p.name.as_str());
    line(p.best());
    line(sum(&range(5)));
    let people = vec![Person::new("x"), Person::new("yz")];
    let mut nl = 0;
    for q in &people {
        nl += q.name.len();
    }
    line(nl as i64);
}
