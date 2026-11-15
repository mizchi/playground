use std::fmt;
struct Button { label: String, on_click: Box<dyn Fn(&str) -> String> }
struct Counter<F: Fn(u32) -> u32> { f: F, n: u32 }
impl<F: Fn(u32) -> u32> Counter<F> { fn step(&mut self) -> u32 { self.n = (self.f)(self.n); self.n } }
struct Wrapper<T>(Vec<T>);
impl<T: fmt::Display> fmt::Display for Wrapper<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[")?;
        for (i, x) in self.0.iter().enumerate() { if i > 0 { write!(f, ", ")?; } write!(f, "{}", x)?; }
        write!(f, "]")
    }
}
fn apply_all<T, F>(xs: &mut [T], f: F) where F: Fn(&T) -> T { for x in xs.iter_mut() { *x = f(x); } }
fn main() {
    let b = Button { label: "ok".to_string(), on_click: Box::new(|who| format!("clicked by {}", who)) };
    println!("{}: {}", b.label, (b.on_click)("me"));
    let mut c = Counter { f: |x| x * 2 + 1, n: 0 };
    println!("{} {} {}", c.step(), c.step(), c.step());
    println!("{} {}", Wrapper(vec![1, 2, 3]), Wrapper(vec!["a", "b"]));
    let mut v = vec![1, 2, 3];
    apply_all(&mut v, |x| x * 10);
    println!("{:?}", v);
    let mut handlers: Vec<(&str, fn(i32) -> i32)> = Vec::new();
    fn double(x: i32) -> i32 { x * 2 }
    fn neg(x: i32) -> i32 { -x }
    handlers.push(("double", double));
    handlers.push(("neg", neg));
    for (n, h) in &handlers { println!("{} {}", n, h(21)); }
}
