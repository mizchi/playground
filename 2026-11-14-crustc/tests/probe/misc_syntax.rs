const fn sq(x: u32) -> u32 { x * x }
const N: u32 = sq(4);
static GREETING: &str = "hi";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Dir { N, E, S, W }
impl Dir { fn turn(self) -> Dir { match self { Dir::N => Dir::E, Dir::E => Dir::S, Dir::S => Dir::W, Dir::W => Dir::N } } }
fn classify(n: i32) -> &'static str {
    match n { i32::MIN..=-1 => "neg", 0 => "zero", 1 | 2 | 3 => "small", x if x % 2 == 0 => "even", _ => "odd" }
}
fn main() {
    println!("{} {}", N, GREETING);
    let d = Dir::W.turn();
    println!("{:?} {} {}", d, matches!(d, Dir::N | Dir::S), d as i32);
    for n in [-5, 0, 2, 10, 11] { print!("{} ", classify(n)); }
    println!();
    let t = (1, "a", 2.5);
    let (x, y, z) = t;
    println!("{} {} {}", x, y, z);
    let v = if x > 0 { "pos" } else { "neg" };
    let label = loop { break "done"; };
    println!("{} {}", v, label);
    let arr = [1, 2, 3, 4, 5];
    if let [first, .., last] = arr { println!("{} {}", first, last); }
    let Some(q) = arr.iter().find(|&&x| x > 3) else { return; };
    println!("{}", q);
    let mut count = 0;
    'outer: for i in 0..10 { for j in 0..10 { if i * j > 20 { break 'outer; } count += 1; } }
    println!("{}", count);
    let s = String::from("moved");
    let f = move || s.len();
    println!("{}", f());
    let mut ds = vec![Dir::S, Dir::N, Dir::W];
    ds.sort();
    println!("{:?}", ds);
    let x = 5u8 as char as u32 + 'a' as u32;
    println!("{}", x);
    println!("{}", i32::pow(2, 10));
    println!("{:?}", std::mem::size_of::<u64>());
}
