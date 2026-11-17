// itoa: 整数を文字列に (各幅・符号・境界値)
fn main() {
    let mut b = itoa::Buffer::new();
    println!("{}", b.format(0u8));
    println!("{}", b.format(-128i8));
    println!("{}", b.format(u16::MAX));
    println!("{}", b.format(i32::MIN));
    println!("{}", b.format(u64::MAX));
    println!("{}", b.format(i64::MIN));
    println!("{}", b.format(u128::MAX));
    println!("{}", b.format(i128::MIN));
    println!("{}", b.format(12345usize));
    let mut len = 0usize;
    let mut x: u64 = 1;
    for i in 0..3_000_000u64 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let s = b.format(x >> (i % 64));
        len += s.len() + s.as_bytes()[0] as usize;
    }
    println!("len {len}");
    println!("done");
}
