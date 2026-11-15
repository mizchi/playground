use std::io::{self, BufRead, Read, Write};
fn main() {
    let mut first = String::new();
    io::stdin().read_line(&mut first).unwrap();
    let n: usize = first.trim().parse().unwrap();
    let stdin = io::stdin();
    let mut nums = Vec::new();
    for line in stdin.lock().lines().take(n) {
        let line = line.unwrap();
        let v: Vec<i64> = line.split_whitespace().map(|x| x.parse().unwrap()).collect();
        nums.push(v.iter().sum::<i64>());
    }
    let mut rest = String::new();
    io::stdin().read_to_string(&mut rest).unwrap();
    let words = rest.split_whitespace().count();
    let out = io::stdout();
    let mut w = out.lock();
    writeln!(w, "{:?} words={}", nums, words).unwrap();
    write!(w, "done\n").unwrap();
    w.flush().unwrap();
}
