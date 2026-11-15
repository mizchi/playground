// 整数の sort / sort_by_key (基数ソートになる大きさ) が std と同じ順になるか: 符号つき・幅・安定性
fn lcg(x: &mut u64) -> u64 {
    *x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *x >> 33
}
fn main() {
    let mut x = 7u64;
    let mut a: Vec<i64> = (0..500).map(|_| lcg(&mut x) as i64 - (1 << 30)).collect();
    a.push(i64::MIN);
    a.push(i64::MAX);
    a.sort();
    println!("{:?} {:?} {}", &a[..3], &a[a.len() - 3..], a.windows(2).all(|w| w[0] <= w[1]));
    let mut b: Vec<i8> = (0..300).map(|_| lcg(&mut x) as i8).collect();
    b.sort_unstable();
    println!("{:?}", &b[..10]);
    let mut c: Vec<u16> = (0..300).map(|_| lcg(&mut x) as u16).collect();
    c.sort();
    println!("{:?} {:?}", &c[..5], &c[295..]);
    let mut d: Vec<(i32, usize)> = (0..400).map(|i| ((lcg(&mut x) % 7) as i32 - 3, i)).collect();
    d.sort_by_key(|p| p.0);
    let stable = d.windows(2).all(|w| w[0].0 < w[1].0 || (w[0].0 == w[1].0 && w[0].1 < w[1].1));
    println!("{:?} {}", &d[..4], stable);
    let mut e: Vec<String> = (0..100).map(|i| format!("s{}", (i * 37) % 100)).collect();
    e.sort_by_key(|s| s.len());
    println!("{:?}", &e[..5]);
    let mut f: Vec<u8> = (0..200).map(|i| (i * 31 % 256) as u8).collect();
    f.sort();
    println!("{:?}", &f[..8]);
    let mut g = [5i32, -3, 9, -3, 0, 7, -100, 42];
    g.sort();
    println!("{:?}", g);
    let mut h: Vec<i32> = (0..100).map(|i| -i).collect();
    h.sort_by_key(|&v| std::cmp::Reverse(v));
    println!("{:?}", &h[..5]);
}
