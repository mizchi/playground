enum E {
    A(i32),
    B(i32),
    C,
}
fn f(e: E) -> i32 {
    match e {
        E::A(x) | E::B(x) => x,
        E::C => 0,
    }
}
fn main() {
    println!("{} {}", f(E::A(3)), f(E::B(4)) + f(E::C));
}
