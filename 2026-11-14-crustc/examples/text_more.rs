// format! の細かい指定 (精度・#x・b・o・f.pad・{x} の取り込み)、HashMap の or_insert_with(Vec::new) / and_modify /
// get_mut / iter_mut / into_iter、&str の lines / trim / starts_with(char) / split_whitespace
use std::collections::HashMap;
use std::fmt;
struct Name<'a>(&'a str);
impl fmt::Display for Name<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad(self.0)
    }
}
fn main() {
    let x = 255u32;
    println!("{:.3}|{:#x}|{:b}|{:o}|{:X}|{:+}|{:#b}", "abcdef", x, 5, 8, x, 7, 5);
    println!("[{:>6}] [{:<6}] [{:^7}]", Name("ab"), Name("cd"), Name("e"));
    let pi = 3;
    println!("{pi} {pi:>4} {0} {0:?} {name}", "s", name = "n");
    let mut m: HashMap<u32, Vec<u32>> = HashMap::with_capacity(4);
    println!("{}", m.is_empty());
    for i in 0..20u32 {
        m.entry(i % 3).or_insert_with(Vec::new).push(i);
    }
    m.entry(1).and_modify(|v| v.push(100)).or_insert(vec![]);
    if let Some(v) = m.get_mut(&2) {
        v.clear();
    }
    for (_, v) in m.iter_mut() {
        v.push(7);
    }
    let mut all: Vec<u32> = m.into_iter().map(|(k, v)| k * 100 + v.len() as u32).collect();
    all.sort();
    println!("{:?}", all);
    let text = "  alpha beta\n gamma  \ndelta";
    for l in text.lines() {
        println!("[{}] {}", l.trim(), l.trim().starts_with('a'));
    }
    let ws: Vec<&str> = text.split_whitespace().collect();
    println!("{:?} {}", ws, ws.len());
    let o: Option<&str> = Some("q");
    let r: Result<i32, String> = Err("bad".to_string());
    println!("{:?} {:?} {:#?}", o, r, (1, "t"));
}
