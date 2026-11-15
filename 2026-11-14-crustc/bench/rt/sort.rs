// 乱数列の sort / sort_by_key / binary_search (ジェネリックなマージソート)
fn main() {
    let mut x: u64 = 88172645463325252;
    let mut v: Vec<u64> = Vec::with_capacity(2_000_000);
    for _ in 0..2_000_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.push(x % 1_000_000_007);
    }
    let mut a = v.clone();
    a.sort();
    let mut b: Vec<(u64, u64)> = v.iter().map(|&x| (x % 1000, x)).collect();
    b.sort_by_key(|p| p.0);
    let mut hits = 0;
    for i in 0..100_000 {
        if a.binary_search(&v[i * 7]).is_ok() {
            hits += 1;
        }
    }
    println!("{} {} {} {}", a[0], a[a.len() - 1], b[0].1, hits);
}
