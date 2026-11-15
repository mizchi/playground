// Iterator のサンプル: 自前の impl Iterator (関連型 Item)、アダプタ (map / filter / enumerate / zip /
// take / skip / chain / copied / filter_map / take_while)、消費 (collect / sum / count / fold / max / min / any /
// all / find / position / last)、範囲・Vec・配列・&str のイテレータ、for で回す
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
struct Counter {
    n: u32,
    limit: u32,
}

impl Iterator for Counter {
    type Item = u32;
    fn next(&mut self) -> Option<Self::Item> {
        if self.n < self.limit {
            self.n += 1;
            Some(self.n)
        } else {
            None
        }
    }
}

struct Fib {
    a: u64,
    b: u64,
}

impl Iterator for Fib {
    type Item = u64;
    fn next(&mut self) -> Option<u64> {
        let r = self.a;
        self.a = self.b;
        self.b = r + self.b;
        Some(r)
    }
}

fn total<I: Iterator<Item = i64>>(it: I) -> i64 {
    let mut s = 0;
    for x in it {
        s += x;
    }
    s
}

fn squares(n: i64) -> Vec<i64> {
    (1..=n).map(|x| x * x).collect()
}

fn main() {
    let v = vec![5i64, 3, 8, 1, 9, 2];
    // map / filter / collect
    let doubled: Vec<i64> = v.iter().map(|x| x * 2).collect();
    for x in &doubled {
        num(*x);
        put(b' ');
    }
    put(b'\n');
    let evens = v.iter().filter(|x| **x % 2 == 0).copied().collect::<Vec<_>>();
    line(evens.len() as i64);
    line(evens[0] + evens[1]);
    // 消費するメソッド
    let s: i64 = v.iter().sum();
    line(s);
    line(v.iter().count() as i64);
    line(*v.iter().max().unwrap());
    line(*v.iter().min().unwrap());
    line(v.iter().fold(100, |acc, x| acc - x));
    line(if v.iter().any(|x| *x > 8) { 1 } else { 0 });
    line(if v.iter().all(|x| *x > 0) { 1 } else { 0 });
    line(*v.iter().find(|x| **x > 5).unwrap());
    line(v.iter().position(|x| *x == 1).unwrap() as i64);
    line(*v.iter().last().unwrap());
    // 範囲
    let r: i64 = (1..=10).filter(|x| x % 3 == 0).sum();
    line(r);
    line(total(squares(4).into_iter()));
    line((0..5).map(|i| i * 10).fold(0, |a, b| a + b));
    // enumerate / zip / take / skip / chain / take_while / filter_map
    for (i, x) in v.iter().enumerate().skip(4) {
        line(i as i64 * 100 + x);
    }
    let names = ["a", "b", "c"];
    let mut zs = 0;
    for (n, x) in names.iter().zip(v.iter()) {
        zs += n.len() as i64 * x;
    }
    line(zs);
    let firsts: Vec<i64> = v.iter().copied().take(2).chain(v.iter().copied().skip(5)).collect();
    line(firsts.len() as i64 * 1000 + firsts[2]);
    let small: Vec<i64> = v.iter().copied().take_while(|x| *x != 1).collect();
    line(small.len() as i64);
    let halves: Vec<i64> = v.iter().filter_map(|x| if x % 2 == 0 { Some(x / 2) } else { None }).collect();
    line(halves[0] * 10 + halves[1]);
    // 自前のイテレータ
    let c = Counter { n: 0, limit: 5 };
    let cs: u32 = c.map(|x| x * x).sum();
    line(cs as i64);
    for x in (Counter { n: 0, limit: 3 }) {
        num(x as i64);
    }
    put(b'\n');
    let fibs: Vec<u64> = Fib { a: 0, b: 1 }.take(10).collect();
    line(fibs[9] as i64);
    line(Fib { a: 0, b: 1 }.skip(20).next().unwrap() as i64);
    // 文字列
    let word = String::from("héllo wörld");
    line(word.chars().count() as i64);
    let upper: String = "abc".chars().map(|c| ((c as u8) - 32) as char).collect();
    puts(upper.as_str());
    let vowels = word.chars().filter(|c| *c == 'o' || *c == 'é' || *c == 'ö').count();
    line(vowels as i64);
    // 所有するイテレータ (要素を取り出す)
    let strings = vec![String::from("x"), String::from("yy"), String::from("zzz")];
    let lens: Vec<usize> = strings.into_iter().map(|s| s.len()).collect();
    line(lens[0] as i64 + lens[1] as i64 + lens[2] as i64);
    // 配列
    let arr = [4i64, 5, 6];
    line(arr.iter().map(|x| x * x).sum::<i64>());
}
