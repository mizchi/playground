mod geo {
    pub struct P {
        pub x: i32,
    }
    impl P {
        pub fn new(x: i32) -> P {
            P { x }
        }
    }
    pub fn twice(p: &P) -> i32 {
        p.x * 2
    }
    pub mod inner {
        pub fn k() -> i32 {
            7
        }
    }
}
use geo::P;
fn main() {
    let p = P::new(3);
    println!("{} {}", geo::twice(&p), geo::inner::k());
}
