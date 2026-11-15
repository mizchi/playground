#[derive(Debug)]
struct Inner { a: i32, b: Vec<u8> }
#[derive(Debug)]
struct Outer { name: String, inner: Inner, opt: Option<(i32, char)> }
#[derive(Debug)]
enum E { Unit, Tup(i32, String), Rec { x: f64 } }
fn main() {
    let o = Outer { name: "n".into(), inner: Inner { a: 1, b: vec![2, 3] }, opt: Some((4, 'c')) };
    println!("{:#?}", o);
    println!("{:?}", vec![E::Unit, E::Tup(1, "s".to_string()), E::Rec { x: 1.5 }]);
    println!("{:#?}", E::Rec { x: 2.0 });
    let w = 10; let p = 3;
    println!("[{:>w$}] [{:<w$.p$}] [{:^1$}]", "r", 7, 3.14159, w = w, p = p);
    println!("[{:>1$}] [{:.*}]", 5, 6, 1.23456);
    println!("{:e} {:E} {:e}", 1234.5f64, 0.00012f64, 100u32);
    println!("{:+.2} {:08.3} {:x?}", 2.5f64, -3.14159f64, vec![10, 255]);
    let name = "z"; let n = 3;
    println!("{name}={n:>4} {n:#b} {0} {0:?}", name);
    eprintln!("to stderr");
}
