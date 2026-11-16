pub mod shape;
pub use crate::shape::Shape;
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct P { pub x: i64, pub y: i64 }
impl P {
    pub fn new(x: i64, y: i64) -> P { P { x, y } }
    pub fn scaled(&self) -> P { P::new(util::helper(self.x), util::helper(self.y)) }
}
impl std::fmt::Display for P {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "({}, {})", self.x, self.y) }
}
pub fn helper() -> u32 { 5 }
pub fn div(a: i64, b: i64) -> i64 { a / b }
