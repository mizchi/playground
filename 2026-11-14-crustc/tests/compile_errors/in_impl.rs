struct S { n: i64 }
impl S {
    fn get(&self) -> i64 {
        self.n
    }
    fn bad(&self) -> i64 {
        self.nope
    }
}
fn main() {
    let s = S { n: 1 };
    println!("{}", s.get() + s.bad());
}
