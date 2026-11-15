// 文字列の組み立て・format!・split・parse
fn main() {
    let mut total = 0u64;
    let mut longest = String::new();
    for i in 0..200_000u64 {
        let s = format!("{}-{}-{}", i, i * 7 % 13, i * i % 1000);
        let parts: Vec<&str> = s.split('-').collect();
        let n: u64 = parts.iter().map(|p| p.parse::<u64>().unwrap()).sum();
        total += n;
        let mut t = String::new();
        for p in parts.iter().rev() {
            t.push_str(p);
            t.push(':');
        }
        if t.len() > longest.len() {
            longest = t;
        }
    }
    println!("{} {}", total, longest);
}
