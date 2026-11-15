fn divide(a: i32, b: i32) -> Result<i32, String> {
    if b == 0 { return Err("div by zero".to_string()); }
    Ok(a / b)
}
fn first_even(v: &[i32]) -> Option<&i32> { v.iter().find(|x| *x % 2 == 0) }
fn main() {
    let mut stack = vec![1, 2, 3];
    while let Some(top) = stack.pop() { print!("{} ", top); }
    println!();
    match divide(7, 0) { Ok(v) => println!("{}", v), Err(e) => println!("error: {}", e) }
    println!("{:?} {:?}", divide(9, 3), first_even(&[1, 3, 4]));
    let x = 5;
    let x = x * 2;
    { let x = "shadow"; println!("{}", x); }
    println!("{}", x);
    let grade = |s: u32| match s { 90..=100 => 'A', 80..=89 => 'B', _ => 'F' };
    println!("{}{}{}", grade(95), grade(85), grade(10));
    let t = std::mem::replace(&mut stack, vec![9]);
    println!("{:?} {:?}", t, stack);
    let mut count = 0u64;
    for i in (0..20).rev().step_by(3) { count += i; }
    println!("{}", count);
    let nested = vec![vec![1, 2], vec![3], vec![]];
    let total: i32 = nested.iter().map(|v| v.iter().sum::<i32>()).sum();
    println!("{} {}", total, nested.iter().any(|v| v.is_empty()));
    assert!(total == 6, "total was {}", total);
    assert_eq!(x, 10);
    assert_ne!(x, 11, "x must not be 11");
    let r: Result<u8, _> = "300".parse::<u8>();
    println!("{:?}", r.is_err());
    let c = 'Z';
    println!("{} {} {:?} {}", c.is_uppercase(), c.to_ascii_lowercase(), c.to_digit(36), ('a'..='e').collect::<String>());
    println!("{}", i64::from(7u32) + i64::from(true));
    let n: u16 = 300u32.try_into().unwrap_or(0);
    println!("{}", n);
}
