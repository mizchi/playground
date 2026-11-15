use std::ops::Add;
#[derive(Debug, Clone, Copy, PartialEq)]
struct V(i32, i32);
impl Add for V {
    type Output = V;
    fn add(self, o: V) -> V {
        V(self.0 + o.0, self.1 + o.1)
    }
}
fn main() {
    println!("{:?}", V(1, 2) + V(3, 4));
}
