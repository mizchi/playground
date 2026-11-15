use std::fmt;
struct P { x: i32, y: i32 }
impl fmt::Debug for P {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.debug_struct("P").field("x", &self.x).field("y", &self.y).finish() }
}
struct L(Vec<i32>);
impl fmt::Debug for L {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.debug_list().entries(self.0.iter()).finish() }
}
struct T2(i32, String);
impl fmt::Debug for T2 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.debug_tuple("T2").field(&self.0).field(&self.1).finish() }
}
struct Hex(u32);
impl fmt::Display for Hex {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if f.alternate() { write!(f, "0x{:X}", self.0) } else { write!(f, "{:x}", self.0) }
    }
}
struct Name(String);
impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.pad(&self.0) }
}
fn main() {
    let p = P { x: 1, y: -2 };
    println!("{:?}", p);
    println!("{:#?}", p);
    println!("{:?} {:?}", L(vec![1, 2]), T2(3, "s".into()));
    println!("{} {:#}", Hex(255), Hex(255));
    println!("[{:>6}] [{:<6}] [{:^7}]", Name("ab".into()), Name("cd".into()), Name("e".into()));
}
