// 手書きの impl PartialEq / Eq / PartialOrd / Ord、ジェネリックな derive、String を持つ enum の sort、std::cmp::max
use std::cmp::Ordering;
struct Money { cents: i64 }
impl PartialEq for Money {
    fn eq(&self, o: &Self) -> bool { self.cents == o.cents }
}
impl Eq for Money {}
impl PartialOrd for Money {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> { self.cents.partial_cmp(&o.cents) }
}
impl Ord for Money {
    fn cmp(&self, o: &Self) -> Ordering { self.cents.cmp(&o.cents) }
}
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone, Copy)]
struct Pair<T> { a: T, b: T }
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Tok { Num(i64), Word(String), End }
fn main() {
    let x = Money { cents: 5 };
    let y = Money { cents: 7 };
    println!("{} {} {} {:?}", x < y, x == y, x >= y, x.cmp(&y));
    let mut ms = vec![Money { cents: 3 }, Money { cents: 1 }, Money { cents: 2 }];
    ms.sort();
    println!("{} {} {}", ms[0].cents, ms[1].cents, ms[2].cents);
    let p = Pair { a: 1u8, b: 2 };
    let q = Pair { a: 1u8, b: 1 };
    println!("{} {:?}", p > q, std::cmp::max(p, q));
    let mut ts = vec![Tok::End, Tok::Word(String::from("b")), Tok::Num(3), Tok::Word(String::from("a")), Tok::Num(-1)];
    ts.sort();
    println!("{:?}", ts);
    println!("{:?}", std::cmp::max((1, 2), (1, 3)));
    let t = (2, 'x', "s");
    println!("{}", t < (2, 'y', "a"));
}
