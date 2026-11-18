//! Shapes.

use super::Describe;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone)]
pub enum Shape {
    Circle(f64),
    Rect { w: f64, h: f64 },
    Empty,
}

impl Point {
    pub const ZERO: Point = Point { x: 0, y: 0 };

    pub fn new(x: i32, y: i32) -> Point {
        Point { x, y }
    }

    pub fn dist2(&self, o: &Point) -> i32 {
        let dx = self.x - o.x;
        let dy = self.y - o.y;
        dx * dx + dy * dy
    }

    pub fn moved(self, by: i32) -> Self {
        Self { x: self.x + by, ..self }
    }
}

impl Describe for Point {
    fn describe(&self) -> String {
        let z = Point::ZERO;
        format!("{} {}", self.x - z.x, self.y)
    }
}

impl Shape {
    pub fn name(&self) -> &'static str {
        match self {
            Shape::Circle(_) => "circle",
            Shape::Rect { .. } => "rect",
            Shape::Empty => "empty",
        }
    }
}

/// Area of a shape.
pub fn area(s: &Shape) -> f64 {
    match s {
        Shape::Circle(r) => 3.0 * r * r,
        Shape::Rect { w, h } => w * h,
        Shape::Empty => 0.0,
    }
}

pub(crate) fn origin() -> Point {
    Point::default()
}
