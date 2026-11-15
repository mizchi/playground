trait Shape {
    fn area(&self) -> f64;
}
struct Sq(f64);
impl Shape for Sq {
    fn area(&self) -> f64 {
        self.0 * self.0
    }
}
fn main() {
    let v: Vec<Box<dyn Shape>> = vec![Box::new(Sq(2.0))];
    println!("{}", v[0].area());
}
