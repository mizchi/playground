// HashMap への挿入と検索 (u64 のキー、entry)
use std::collections::HashMap;
fn main() {
    let mut m: HashMap<u64, u64> = HashMap::new();
    let mut x: u64 = 12345;
    for _ in 0..1_000_000 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *m.entry(x >> 44).or_insert(0) += 1;
    }
    let mut best = (0, 0);
    for (&k, &c) in &m {
        if c > best.1 || (c == best.1 && k < best.0) {
            best = (k, c);
        }
    }
    let mut found = 0u64;
    for k in 0..1_000_000u64 {
        if let Some(c) = m.get(&k) {
            found += c;
        }
    }
    println!("{} {:?} {}", m.len(), best, found);
}
