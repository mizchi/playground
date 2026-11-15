// 型の違う PartialOrd (`impl PartialOrd<Feet> for Meters`)、手書きの partial_cmp の None、f64 の partial_cmp
use std::cmp::Ordering;
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
struct Meters(f64);
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
struct Feet(f64);
impl PartialEq<Feet> for Meters {
    fn eq(&self, o: &Feet) -> bool {
        self.0 == o.0 * 0.3048
    }
}
impl PartialOrd<Feet> for Meters {
    fn partial_cmp(&self, o: &Feet) -> Option<Ordering> {
        self.0.partial_cmp(&(o.0 * 0.3048))
    }
}
#[derive(Debug, PartialEq)]
struct Ver { major: u32, minor: u32 }
impl PartialEq<(u32, u32)> for Ver {
    fn eq(&self, o: &(u32, u32)) -> bool { self.major == o.0 && self.minor == o.1 }
}
impl PartialOrd<(u32, u32)> for Ver {
    fn partial_cmp(&self, o: &(u32, u32)) -> Option<Ordering> {
        Some((self.major, self.minor).cmp(o))
    }
}
impl PartialEq<u32> for Ver {
    fn eq(&self, o: &u32) -> bool { self.major == *o }
}
impl PartialOrd<u32> for Ver {
    fn partial_cmp(&self, o: &u32) -> Option<Ordering> {
        self.major.partial_cmp(o)
    }
}
#[derive(PartialEq)]
struct F(f64);
impl PartialOrd for F {
    fn partial_cmp(&self, o: &F) -> Option<Ordering> { self.0.partial_cmp(&o.0) }
}
fn main() {
    let m = Meters(1.0);
    for f in [Feet(3.0), Feet(1.0 / 0.3048), Feet(4.0), Feet(f64::NAN)] {
        println!("{:?}: {} {} {} {} {}", f, m < f, m <= f, m > f, m >= f, m == f);
    }
    let v = Ver { major: 1, minor: 4 };
    println!("{} {} {} {}", v < (1, 5), v > (1, 3), v >= (1, 4), v <= (0, 9));
    println!("{} {} {}", v < 2, v > 0, v >= 1);
    let r = &v;
    println!("{} {}", *r < (2, 0), r.partial_cmp(&(1, 4)) == Some(Ordering::Equal));
    println!("{}", Meters(2.0) < Meters(3.0));

    let (a, n) = (F(1.0), F(f64::NAN));
    println!("{} {} {} {}", a < n, a <= n, a > n, a >= n);
    println!("{} {} {} {}", a < F(2.0), a <= F(1.0), a > F(0.0), a >= F(3.0));
    let n = f64::NAN;
    println!("{:?} {:?} {:?}", 1.0f64.partial_cmp(&n), 1.0f64.partial_cmp(&2.0), 2.5f32.partial_cmp(&1.0));
}
