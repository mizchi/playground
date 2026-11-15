// Default のサンプル: #[derive(Default)]、手書きの impl Default、Default::default() / T::default() / i32::default() /
// Vec::default()、構造体の更新記法 `..Default::default()` / `..base`、unwrap_or_default、std::mem::take / swap / replace
use std::collections::HashMap;

#[derive(Debug, Default, Clone, PartialEq)]
struct Config {
    name: String,
    retries: u32,
    verbose: bool,
    tags: Vec<String>,
    limit: Option<u64>,
}

#[derive(Debug)]
struct Port(u16);

impl Default for Port {
    fn default() -> Port {
        Port(8080)
    }
}

#[derive(Debug, Default)]
struct Server {
    port: Port,
    host: String,
    weights: [u8; 3],
}

fn make<T: Default>() -> T {
    T::default()
}

fn main() {
    let c = Config::default();
    let d = Config { retries: 3, name: String::from("svc"), ..Default::default() };
    let e = Config { verbose: true, ..d.clone() };
    println!("{:?}\n{:?}\n{:?}", c, d, e);
    let s: Server = Default::default();
    println!("{:?}", s);
    let n: i32 = make();
    let v: Vec<u8> = make();
    let t: (u8, String, bool) = Default::default();
    println!("{} {:?} {:?} {} {:?}", n, v, t, i64::default(), String::default());
    let m: HashMap<String, i32> = HashMap::default();
    println!("{}", m.len());
    let none: Option<Vec<i32>> = None;
    println!("{:?} {} {}", none.unwrap_or_default(), Some(5).unwrap_or_default(), "x".parse::<i32>().unwrap_or_default());
    let mut names = vec![String::from("a"), String::from("b")];
    let taken = std::mem::take(&mut names);
    let mut x = String::from("left");
    let mut y = String::from("right");
    std::mem::swap(&mut x, &mut y);
    let old = std::mem::replace(&mut x, String::from("new"));
    println!("{:?} {:?} {} {} {}", names, taken, x, y, old);
}
