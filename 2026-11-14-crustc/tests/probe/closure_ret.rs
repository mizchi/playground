fn make(n: i32) -> Box<dyn Fn(i32) -> i32> {
    Box::new(move |x| x + n)
}
fn compose<F: Fn(i32) -> i32, G: Fn(i32) -> i32>(f: F, g: G) -> impl Fn(i32) -> i32 {
    move |x| g(f(x))
}
fn main() {
    let f = make(3);
    let h = compose(|x| x * 2, |x| x + 1);
    println!("{} {}", f(1), h(5));
}
