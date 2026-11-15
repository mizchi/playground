struct Parser<'a> { src: &'a str, pos: usize }
impl<'a> Parser<'a> {
    fn next_word(&mut self) -> Option<&'a str> {
        let rest = &self.src[self.pos..];
        let rest_t = rest.trim_start();
        if rest_t.is_empty() { return None; }
        let start = self.pos + (rest.len() - rest_t.len());
        let end = rest_t.find(' ').map(|i| start + i).unwrap_or(self.src.len());
        self.pos = end;
        Some(&self.src[start..end])
    }
}
fn longest<'a>(a: &'a str, b: &'a str) -> &'a str { if a.len() >= b.len() { a } else { b } }
fn main() {
    let mut p = Parser { src: "hello  big world", pos: 0 };
    while let Some(w) = p.next_word() { println!("[{}]", w); }
    println!("{}", longest("abc", "de"));
}
