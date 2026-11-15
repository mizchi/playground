fn main() {
    let mut a = 1;
    let mut b = 2;
    std::mem::swap(&mut a, &mut b);
    let mut v = vec![1];
    let old = std::mem::take(&mut v);
    let r = std::mem::replace(&mut a, 10);
    println!("{} {} {:?} {:?} {}", a, b, v, old, r);
}
