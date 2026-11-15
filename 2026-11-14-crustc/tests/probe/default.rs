#[derive(Debug, Default, Clone)]
struct C {
    a: i32,
    b: String,
    c: Vec<u8>,
}
fn main() {
    let c = C {
        a: 3,
        ..Default::default()
    };
    let d: C = Default::default();
    println!("{:?} {:?} {}", c, d, i32::default());
}
