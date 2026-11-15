fn main() {
    let a: u8 = 200;
    println!("{:?} {:?}", a.checked_add(100), a.overflowing_add(100));
    println!("{} {}", a.saturating_add(100), a.wrapping_add(100));
    let b: i32 = i32::MAX;
    println!("{:?}", b.checked_mul(2));
    println!("{}", (-7i32).rem_euclid(3));
    println!("{}", (-7i32).div_euclid(3));
    println!("{}", 10u32.pow(3).trailing_zeros());
    println!("{}", i64::MAX.count_ones());
    println!("{}", 17u32.next_power_of_two());
    println!("{}", 255u8.leading_ones());
    println!("{:?}", 5u32.checked_pow(20));
    println!("{}", 100u64.isqrt());
    println!("{}", u32::from_str_radix("ff", 16).unwrap());
    println!("{}", 12i32.to_string() + &(-3i64).abs().to_string());
}
