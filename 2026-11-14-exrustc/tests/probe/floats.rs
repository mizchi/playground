fn main() {
    let x: f64 = 3.5;
    let y = x * 2.0 + 1.25;
    println!("{} {:.2} {}", y, y.sqrt(), (y as i64));
    let v = vec![1.5f64, 2.5, 0.5];
    let s: f64 = v.iter().sum();
    println!("{} {}", s, v.iter().cloned().fold(f64::MIN, f64::max));
}
