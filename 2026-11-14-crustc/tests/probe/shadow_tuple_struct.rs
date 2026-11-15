#[derive(Debug)]
struct Meters(f64);
fn main() {
    let x = 5;
    let x = x * 2;
    let (a, b) = (x, x + 1);
    println!("{} {} {:?}", a, b, Meters(1.5));
}
