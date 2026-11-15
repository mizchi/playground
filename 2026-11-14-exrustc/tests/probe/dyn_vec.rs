trait Shape { fn area(&self) -> f64; fn name(&self) -> String { String::from("shape") } }
struct Sq(f64);
struct Circle { r: f64 }
impl Shape for Sq { fn area(&self) -> f64 { self.0 * self.0 } fn name(&self) -> String { "sq".to_string() } }
impl Shape for Circle { fn area(&self) -> f64 { 3.14 * self.r * self.r } }
fn main() {
    let v: Vec<Box<dyn Shape>> = vec![Box::new(Sq(2.0)), Box::new(Circle { r: 1.0 })];
    let total: f64 = v.iter().map(|s| s.area()).sum();
    for s in &v { println!("{} {:.2}", s.name(), s.area()); }
    println!("{:.3}", total);
}
