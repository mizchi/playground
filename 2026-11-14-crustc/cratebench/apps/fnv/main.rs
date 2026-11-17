// fnv: FnvHasher を直接使う・FnvHashMap / FnvHashSet で数える
use std::hash::Hasher;

use fnv::{FnvHashMap, FnvHashSet, FnvHasher};

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = FnvHasher::default();
    h.write(bytes);
    h.finish()
}

fn main() {
    for s in ["", "a", "foobar", "hello world"] {
        println!("{s:?} {:016x}", fnv1a(s.as_bytes()));
    }
    let mut keyed = FnvHasher::with_key(42);
    keyed.write_u32(7);
    println!("keyed {:x}", keyed.finish());
    // (疑似乱数の単語を数える)
    let mut counts: FnvHashMap<u64, u32> = FnvHashMap::default();
    let mut seen: FnvHashSet<u32> = FnvHashSet::default();
    let mut x: u64 = 1;
    let mut total: u64 = 0;
    for _ in 0..2_000_000 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let k = (x >> 40) % 50_000;
        *counts.entry(k).or_insert(0) += 1;
        seen.insert((x >> 50) as u32);
        total = total.wrapping_add(fnv1a(&k.to_le_bytes()));
    }
    let mut best: Vec<(u64, u32)> = counts.iter().map(|(&k, &v)| (k, v)).collect();
    best.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("distinct {} seen {} total {:x}", counts.len(), seen.len(), total);
    println!("top {:?}", &best[..3]);
    println!("done");
}
