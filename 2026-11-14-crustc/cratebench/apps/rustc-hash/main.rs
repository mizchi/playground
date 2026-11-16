// rustc-hash: FxHasher (整数・バイト列) と FxHashMap / FxHashSet / FxBuildHasher
use std::hash::{BuildHasher, Hash, Hasher};

use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet, FxHasher};

fn fx<T: Hash>(x: T) -> u64 {
    let mut h = FxHasher::default();
    x.hash(&mut h);
    h.finish()
}

fn main() {
    println!("{:x} {:x} {:x}", fx(0u64), fx(12345u32), fx("hello"));
    println!("{:x}", fx(b"a slightly longer byte string, more than 16".as_slice()));
    let mut h = FxHasher::with_seed(7);
    h.write(b"0123456789");
    println!("{:x}", h.finish());
    println!("{:x}", FxBuildHasher.hash_one(99u64));
    let mut m: FxHashMap<u64, u64> = FxHashMap::default();
    let mut s: FxHashSet<u32> = FxHashSet::default();
    let mut x: u64 = 1;
    let mut acc = 0u64;
    for i in 0..2_000_000u64 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *m.entry(x % 100_000).or_insert(0) += i;
        s.insert((x >> 45) as u32);
        acc = acc.wrapping_add(fx(x));
    }
    let mut keys: Vec<u64> = m.keys().copied().filter(|k| k % 10_000 == 0).collect();
    keys.sort();
    println!("{} {} {:x} {:?}", m.len(), s.len(), acc, keys);
    println!("done");
}
