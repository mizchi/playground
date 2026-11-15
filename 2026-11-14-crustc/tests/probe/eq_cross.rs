// 型の違う ==: Vec と配列・スライス、要素の型が違う比較、手書きの `impl PartialEq<R> for T`
#[derive(Debug)]
struct Meters(f64);
#[derive(Debug)]
struct Feet(f64);
impl PartialEq<Feet> for Meters {
    fn eq(&self, o: &Feet) -> bool {
        (self.0 - o.0 * 0.3048).abs() < 1e-9
    }
}
#[derive(Debug, PartialEq)]
struct P { x: i32 }
impl PartialEq<i32> for P {
    fn eq(&self, o: &i32) -> bool { self.x == *o }
}
fn main() {
    let v: Vec<i64> = vec![1, 2, 3];
    println!("{}", v == [1, 2, 3]);
    let s = vec![String::from("a"), String::from("b")];
    println!("{} {}", s == ["a", "b"], s == ["a", "c"]);
    let w: Vec<&str> = vec!["a", "b"];
    println!("{}", s == w);
    let a = [1u8, 2, 3];
    let sl: &[u8] = &a[..];
    println!("{} {} {}", sl == a, a == sl, a[..2] == [1, 2]);
    let e: Vec<i32> = Vec::new();
    println!("{}", e == []);
    assert_eq!(v, [1, 2, 3]);
    assert_eq!(s, ["a", "b"]);
    assert_ne!(v, &[1i64, 2][..]);
    let mut words = "x y".split(' ').collect::<Vec<_>>();
    words.sort();
    assert_eq!(words, ["x", "y"]);
    println!("ok");

    let m = Meters(0.3048);
    let f = Feet(1.0);
    println!("{} {}", m == f, m != Feet(2.0));
    let p = P { x: 3 };
    println!("{} {} {}", p == 3, p == P { x: 3 }, p != 4);
    let ps = vec![P { x: 1 }];
    println!("{}", ps == [P { x: 1 }]);
    assert_eq!(p, 3);
}
