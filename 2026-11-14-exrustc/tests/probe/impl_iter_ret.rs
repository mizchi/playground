// (未対応: `-> impl Iterator` の戻り値)
fn evens(n: u32) -> impl Iterator<Item = u32> { (0..n).filter(|x| x % 2 == 0) }
fn main() {
    println!("{:?}", evens(10).collect::<Vec<_>>());
}
