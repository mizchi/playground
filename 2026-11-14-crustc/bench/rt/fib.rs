// 再帰呼び出し (関数呼び出しのオーバーヘッド)
fn fib(n: u32) -> u64 {
    if n < 2 { n as u64 } else { fib(n - 1) + fib(n - 2) }
}
fn main() {
    println!("{}", fib(35));
}
