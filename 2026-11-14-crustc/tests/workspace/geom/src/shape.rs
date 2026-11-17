pub trait Shape { fn area(&self) -> i64; fn name(&self) -> String { "shape".to_string() } }
pub struct Rect { pub w: i64, pub h: i64 }
impl Shape for Rect { fn area(&self) -> i64 { self.w * self.h } }
pub fn total(v: &[Box<dyn Shape>]) -> i64 { v.iter().map(|s| s.area()).sum() }
