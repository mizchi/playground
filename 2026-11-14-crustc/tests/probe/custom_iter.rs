struct Words<'a> { s: &'a str }
impl<'a> Iterator for Words<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        let t = self.s.trim_start();
        if t.is_empty() { return None; }
        let end = t.find(' ').unwrap_or(t.len());
        let (w, rest) = t.split_at(end);
        self.s = rest;
        Some(w)
    }
}
struct Fib { a: u64, b: u64 }
impl Iterator for Fib {
    type Item = u64;
    fn next(&mut self) -> Option<u64> { let r = self.a; self.a = self.b; self.b += r; Some(r) }
}
fn main() {
    let w = Words { s: "  the quick  brown fox " };
    println!("{:?}", w.collect::<Vec<_>>());
    println!("{:?}", Fib { a: 0, b: 1 }.take_while(|&x| x < 100).collect::<Vec<_>>());
    let s: u64 = Fib { a: 0, b: 1 }.skip(5).step_by(2).take(3).sum();
    println!("{}", s);
    let longest = Words { s: "a bbb cc" }.max_by_key(|w| w.len());
    println!("{:?}", longest);
}
