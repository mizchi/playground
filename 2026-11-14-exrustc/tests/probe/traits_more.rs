use std::collections::HashMap;
trait Account { fn id(&self) -> u32; fn balance(&self) -> i64; fn deposit(&mut self, amt: i64); fn kind(&self) -> &'static str { "basic" } }
#[derive(Debug)] struct Checking { id: u32, bal: i64 }
#[derive(Debug)] struct Savings { id: u32, bal: i64, rate: f64 }
impl Account for Checking { fn id(&self) -> u32 { self.id } fn balance(&self) -> i64 { self.bal } fn deposit(&mut self, a: i64) { self.bal += a; } }
impl Account for Savings {
    fn id(&self) -> u32 { self.id } fn balance(&self) -> i64 { self.bal }
    fn deposit(&mut self, a: i64) { self.bal += a + (a as f64 * self.rate) as i64; }
    fn kind(&self) -> &'static str { "savings" }
}
struct Bank { accounts: Vec<Box<dyn Account>> }
impl Bank {
    fn total(&self) -> i64 { self.accounts.iter().map(|a| a.balance()).sum() }
    fn find_mut(&mut self, id: u32) -> Option<&mut Box<dyn Account>> { self.accounts.iter_mut().find(|a| a.id() == id) }
}
trait Shape { fn area(&self) -> f64; fn name(&self) -> String; }
#[derive(Clone)] struct Circle(f64);
impl Shape for Circle { fn area(&self) -> f64 { 3.0 * self.0 * self.0 } fn name(&self) -> String { format!("circle({})", self.0) } }
fn largest<S: Shape>(xs: &[S]) -> Option<&S> {
    let mut best: Option<&S> = None;
    for x in xs { if best.map_or(true, |b| x.area() > b.area()) { best = Some(x); } }
    best
}
fn main() {
    let mut bank = Bank { accounts: vec![Box::new(Checking { id: 1, bal: 100 }), Box::new(Savings { id: 2, bal: 200, rate: 0.1 })] };
    if let Some(a) = bank.find_mut(2) { a.deposit(50); }
    for a in &bank.accounts { println!("{} {} {}", a.id(), a.kind(), a.balance()); }
    println!("{}", bank.total());
    let cs = vec![Circle(1.0), Circle(3.0), Circle(2.0)];
    println!("{}", largest(&cs).map(|c| c.name()).unwrap_or_default());
    let mut by_kind: HashMap<&str, i64> = HashMap::new();
    for a in &bank.accounts { *by_kind.entry(a.kind()).or_insert(0) += a.balance(); }
    let mut ks: Vec<_> = by_kind.into_iter().collect(); ks.sort();
    println!("{:?}", ks);
}
