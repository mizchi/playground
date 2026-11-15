use std::fmt;
use std::ops::{Add, Mul, Neg, AddAssign, Index};
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct V2 { x: i32, y: i32 }
impl Add for V2 { type Output = V2; fn add(self, o: V2) -> V2 { V2 { x: self.x + o.x, y: self.y + o.y } } }
impl Mul<i32> for V2 { type Output = V2; fn mul(self, k: i32) -> V2 { V2 { x: self.x * k, y: self.y * k } } }
impl Neg for V2 { type Output = V2; fn neg(self) -> V2 { V2 { x: -self.x, y: -self.y } } }
impl AddAssign for V2 { fn add_assign(&mut self, o: V2) { self.x += o.x; self.y += o.y; } }
impl fmt::Display for V2 { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "({}, {})", self.x, self.y) } }
struct Poly(Vec<i32>);
impl Index<usize> for Poly { type Output = i32; fn index(&self, i: usize) -> &i32 { &self.0[i] } }
impl From<(i32, i32)> for V2 { fn from(t: (i32, i32)) -> V2 { V2 { x: t.0, y: t.1 } } }
struct Counter { n: u32 }
impl Iterator for Counter { type Item = u32; fn next(&mut self) -> Option<u32> { if self.n < 5 { self.n += 1; Some(self.n) } else { None } } }
fn main() {
    let mut a = V2 { x: 1, y: 2 };
    let b = V2::from((3, 4));
    a += b;
    println!("{} {} {:?}", a + b, -a * 2, V2::default());
    let c: V2 = (5, 6).into();
    println!("{}", c);
    println!("{}", Poly(vec![4, 5])[1]);
    println!("{:?}", Counter { n: 0 }.zip(Counter { n: 2 }).map(|(a, b)| a * b).collect::<Vec<_>>());
    println!("{}", Counter { n: 0 }.sum::<u32>());
    println!("{}", a.to_string().len());
}
