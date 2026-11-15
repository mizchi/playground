struct S;
impl S {
    const N: usize = 4;
}
trait T {
    const K: i32;
    fn k(&self) -> i32 {
        Self::K
    }
}
impl T for S {
    const K: i32 = 9;
}
fn main() {
    println!("{} {}", S::N, S.k());
}
