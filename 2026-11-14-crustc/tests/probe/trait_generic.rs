use std::fmt::Display;
fn show<T: Display>(xs: &[T]) -> String {
    xs.iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join("-")
}
fn main() {
    println!("{}", show(&[1, 2, 3]));
}
