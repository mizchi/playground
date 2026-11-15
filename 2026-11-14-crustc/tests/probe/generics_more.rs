use std::fmt::Display;
#[derive(Debug, Clone, PartialEq)]
struct Stack<T> { items: Vec<T> }
impl<T: Clone + Display> Stack<T> {
    fn new() -> Self { Stack { items: Vec::new() } }
    fn push(&mut self, t: T) -> &mut Self { self.items.push(t); self }
    fn peek(&self) -> Option<&T> { self.items.last() }
    fn show(&self) -> String { self.items.iter().map(|x| format!("<{}>", x)).collect() }
}
trait Animal { fn speak(&self) -> String; }
struct Dog;
impl Animal for Dog { fn speak(&self) -> String { "woof".into() } }
fn talk<A: Animal + ?Sized>(a: &A) -> String { a.speak() }
fn largest<T: PartialOrd + Copy>(xs: &[T]) -> T { let mut m = xs[0]; for &x in xs { if x > m { m = x; } } m }
fn apply<F>(f: F, n: i32) -> i32 where F: Fn(i32) -> i32 { f(f(n)) }
fn make_adder(k: i32) -> impl Fn(i32) -> i32 { move |x| x + k }
fn boxed(k: i32) -> Box<dyn Fn(i32) -> i32> { Box::new(move |x| x * k) }
fn main() {
    let mut s = Stack::new();
    s.push(1).push(2).push(3);
    println!("{:?} {} {:?}", s.peek(), s.show(), s.clone() == s);
    println!("{}", talk(&Dog));
    println!("{} {}", largest(&[3, 7, 2]), largest(&[1.5, 0.2]));
    println!("{} {} {}", apply(|x| x * 3, 2), make_adder(5)(1), boxed(4)(3));
    let fs: Vec<Box<dyn Fn(i32) -> i32>> = vec![Box::new(|x| x + 1), boxed(2)];
    println!("{:?}", fs.iter().map(|f| f(10)).collect::<Vec<_>>());
}
