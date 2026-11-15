trait Zero { const ZERO: Self; fn is_zero(&self) -> bool; }
impl Zero for i32 { const ZERO: i32 = 0; fn is_zero(&self) -> bool { *self == 0 } }
fn count_zeros<T: Zero>(xs: &[T]) -> usize { xs.iter().filter(|x| x.is_zero()).count() }
fn make<T: Default + std::fmt::Debug>() -> T { T::default() }
fn main() {
    println!("{} {}", count_zeros(&[0, 1, 0, 2]), i32::ZERO);
    let s: String = make(); let n: u8 = make(); let v: Vec<i32> = make();
    println!("{:?} {} {:?}", s, n, v);
    let mut a = vec![1, 2]; let mut b = vec![3];
    std::mem::swap(&mut a, &mut b);
    println!("{:?} {:?}", a, b);
    let mut v = vec![1, 5, 3];
    if let Some(x) = v.iter_mut().find(|x| **x == 5) { *x = 50; }
    println!("{:?}", v);
    let arr = [3, 1, 4, 1, 5];
    println!("{} {:?} {} {:?}", arr.len(), arr.iter().max(), arr.iter().sum::<i32>(), arr.iter().position(|&x| x == 4));
    let bs = b"hello";
    println!("{} {}", bs[1] as char, bs.iter().filter(|&&c| c == b'l').count());
    let s = "abc";
    println!("{:?}", s.char_indices().rev().collect::<Vec<_>>());
    println!("{:?}", s.bytes().rev().map(|b| b as char).collect::<String>());
    let r = 'outer: loop { for i in 0.. { if i * i > 50 { break 'outer i; } } };
    println!("{}", r);
    let x = dbg!(2 + 3);
    println!("{}", x);
    let mut cap = String::with_capacity(10);
    cap.push_str("hi");
    println!("{} {}", cap, cap.capacity() >= 10);
    let total: f64 = [1.0, 2.0, 4.0].iter().sum();
    println!("{:.3}", total / 3 as f64);
}
