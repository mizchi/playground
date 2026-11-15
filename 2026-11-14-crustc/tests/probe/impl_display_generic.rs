use std::fmt;
struct W<T>(T);
impl<T: fmt::Display> fmt::Display for W<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[{}]", self.0)
    }
}
fn main() {
    println!("{} {}", W(3), W("x"));
}
