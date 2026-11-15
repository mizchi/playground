use std::fmt::Display;
trait Describe { fn describe(&self) -> String; }
impl<T: Display> Describe for T { fn describe(&self) -> String { format!("[{}]", self) } }
trait Area { fn area(&self) -> f64; fn double(&self) -> f64 { self.area() * 2.0 } }
struct R(f64, f64);
impl Area for R { fn area(&self) -> f64 { self.0 * self.1 } }
fn total(shapes: &[&dyn Area]) -> f64 { shapes.iter().map(|s| s.area()).sum() }
fn biggest<'a>(shapes: &'a [Box<dyn Area>]) -> &'a dyn Area {
    let mut b = &shapes[0];
    for s in shapes { if s.area() > b.area() { b = s; } }
    b.as_ref()
}
fn main() {
    println!("{} {}", 5.describe(), "x".describe());
    let r = R(2.0, 3.0);
    println!("{} {}", r.area(), r.double());
    println!("{}", total(&[&r, &R(1.0, 1.0)]));
    let v: Vec<Box<dyn Area>> = vec![Box::new(R(1.0, 2.0)), Box::new(R(3.0, 3.0))];
    println!("{}", biggest(&v).area());
}
