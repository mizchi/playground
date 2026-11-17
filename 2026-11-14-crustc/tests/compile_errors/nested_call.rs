fn g(a: &str) -> usize { a.len() }
fn main() {
    let n: u8 = 7;
    let s = format!("{}", g(n));
    println!("{s}");
}
