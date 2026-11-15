use std::fmt;
#[derive(Debug, Clone)]
struct Student { name: String, grade: u8, scores: Vec<f32> }
impl Student {
    fn avg(&self) -> f32 { if self.scores.is_empty() { 0.0 } else { self.scores.iter().sum::<f32>() / self.scores.len() as f32 } }
}
impl fmt::Display for Student {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{:<6} g{} avg={:.2}", self.name, self.grade, self.avg()) }
}
fn fizzbuzz(n: u32) -> String {
    match (n % 3, n % 5) { (0, 0) => "FizzBuzz".to_string(), (0, _) => "Fizz".into(), (_, 0) => "Buzz".into(), _ => n.to_string() }
}
fn is_prime(n: u64) -> bool { n >= 2 && (2..).take_while(|i| i * i <= n).all(|i| n % i != 0) }
fn c_to_f(c: f64) -> f64 { c * 9.0 / 5.0 + 32.0 }
fn reverse_words(s: &str) -> String { s.split_whitespace().rev().collect::<Vec<_>>().join(" ") }
fn main() {
    println!("{}", (1..=15).map(fizzbuzz).collect::<Vec<_>>().join(","));
    println!("{:?}", (1..30).filter(|&n| is_prime(n)).collect::<Vec<u64>>());
    for c in [-40.0, 0.0, 37.5, 100.0] { println!("{:>6.1}C = {:>6.1}F", c, c_to_f(c)); }
    println!("{}", reverse_words("the quick brown fox"));
    let mut ss = vec![
        Student { name: "amy".into(), grade: 3, scores: vec![90.0, 85.5] },
        Student { name: "bob".into(), grade: 2, scores: vec![70.0, 95.0, 80.0] },
        Student { name: "cy".into(), grade: 3, scores: vec![] },
    ];
    ss.sort_by(|a, b| b.avg().partial_cmp(&a.avg()).unwrap());
    for s in &ss { println!("{}", s); }
    let by_grade: Vec<(u8, usize)> = {
        let mut m = std::collections::BTreeMap::new();
        for s in &ss { *m.entry(s.grade).or_insert(0) += 1; }
        m.into_iter().collect()
    };
    println!("{:?}", by_grade);
    let best = ss.iter().max_by(|a, b| a.avg().partial_cmp(&b.avg()).unwrap()).map(|s| s.name.as_str());
    println!("{:?}", best);
    let names: Vec<String> = ss.iter().map(|s| s.name.to_uppercase()).collect();
    println!("{}", names.join("/"));
    let total_scores: usize = ss.iter().map(|s| s.scores.len()).sum();
    println!("{}", total_scores);
    let mut counter = [0usize; 26];
    for b in "hello world".bytes().filter(|b| b.is_ascii_lowercase()) { counter[(b - b'a') as usize] += 1; }
    let top: Vec<(char, usize)> = counter.iter().enumerate().filter(|&(_, &c)| c > 1).map(|(i, &c)| ((b'a' + i as u8) as char, c)).collect();
    println!("{:?}", top);
}
