// モジュールのサンプル (1 ファイル): 入れ子の mod、pub / pub(crate)、use (self:: / super:: / crate::)、
// mod の中の struct / enum / impl / trait / const、パスでの呼び出し、#[cfg(test)] mod tests (コンパイルしない)
mod geometry {
    pub const UNIT: i32 = 1;

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Point {
        pub x: i32,
        pub y: i32,
    }

    impl Point {
        pub fn new(x: i32, y: i32) -> Point {
            Point { x, y }
        }
        pub fn manhattan(&self, o: &Point) -> i32 {
            shapes::abs(self.x - o.x) + shapes::abs(self.y - o.y)
        }
    }

    pub mod shapes {
        use super::Point;

        pub(crate) fn abs(v: i32) -> i32 {
            if v < 0 { -v } else { v }
        }

        #[derive(Debug)]
        pub enum Shape {
            Dot(Point),
            Rect { a: Point, b: Point },
        }

        pub trait Area {
            fn area(&self) -> i32;
        }

        impl Area for Shape {
            fn area(&self) -> i32 {
                match self {
                    Shape::Dot(_) => 0,
                    Shape::Rect { a, b } => abs(b.x - a.x) * abs(b.y - a.y),
                }
            }
        }
    }
}

mod util {
    pub fn twice(v: i32) -> i32 {
        v * crate::geometry::UNIT * 2
    }
}

use geometry::shapes::{Area, Shape};
use geometry::Point;

fn main() {
    let p = Point::new(1, 2);
    let q = geometry::Point { x: 4, y: -2 };
    println!("{:?} {:?} {}", p, q, p.manhattan(&q));
    let shapes = vec![Shape::Dot(p), geometry::shapes::Shape::Rect { a: p, b: q }];
    for s in &shapes {
        println!("{:?} {}", s, s.area());
    }
    println!("{} {} {}", util::twice(21), geometry::shapes::abs(-5), geometry::UNIT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twice_works() {
        assert_eq!(util::twice(2), 4);
    }
}
