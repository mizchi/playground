fn sum(xs: &[i32]) -> i32 {
    match xs {
        [] => 0,
        [x, rest @ ..] => x + sum(rest),
    }
}
fn main() {
    println!("{}", sum(&[1, 2, 3]));
}
