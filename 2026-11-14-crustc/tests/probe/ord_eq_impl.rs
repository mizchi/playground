// 手書きの Ord / Eq (sort・max・BinaryHeap・BTreeSet・clamp・cmp・==)
use std::cmp::Ordering;
use std::collections::{BinaryHeap, BTreeMap, BTreeSet};
#[derive(Debug, Clone)]
struct Task { pri: u32, name: String }
impl PartialEq for Task {
    fn eq(&self, o: &Self) -> bool { self.pri == o.pri }
}
impl Eq for Task {}
impl PartialOrd for Task {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> { Some(self.cmp(o)) }
}
impl Ord for Task {
    fn cmp(&self, o: &Self) -> Ordering { o.pri.cmp(&self.pri).then_with(|| self.name.cmp(&o.name)) }
}
#[derive(Debug, Clone, Copy)]
struct Abs(i32);
impl PartialEq for Abs { fn eq(&self, o: &Abs) -> bool { self.0.abs() == o.0.abs() } }
impl Eq for Abs {}
impl Ord for Abs { fn cmp(&self, o: &Abs) -> Ordering { self.0.abs().cmp(&o.0.abs()) } }
impl PartialOrd for Abs { fn partial_cmp(&self, o: &Abs) -> Option<Ordering> { Some(self.cmp(o)) } }
fn largest<T: Ord + Copy>(v: &[T]) -> T {
    let mut m = v[0];
    for &x in v { if x > m { m = x; } }
    m
}
fn main() {
    let t = |p, n: &str| Task { pri: p, name: n.to_string() };
    let mut v = vec![t(1, "b"), t(3, "a"), t(2, "c"), t(3, "0")];
    v.sort();
    println!("{:?}", v.iter().map(|x| &x.name).collect::<Vec<_>>());
    println!("{} {}", t(1, "x") == t(1, "y"), t(1, "x") < t(2, "x"));
    println!("{:?}", v.iter().max().map(|x| &x.name));
    println!("{:?}", v.iter().min().map(|x| &x.name));
    let mut h = BinaryHeap::new();
    for x in v.clone() { h.push(x); }
    println!("{:?}", h.pop().map(|x| x.name));
    let mut a = vec![Abs(-5), Abs(3), Abs(-1), Abs(4)];
    a.sort();
    println!("{:?}", a);
    println!("{:?} {:?}", largest(&a), a.iter().copied().max());
    println!("{:?} {:?}", Abs(-3).cmp(&Abs(2)), Ord::cmp(&Abs(1), &Abs(-1)));
    println!("{:?} {:?}", Abs(-7).max(Abs(6)), Abs(-7).min(Abs(6)));
    println!("{:?}", Abs(9).clamp(Abs(-2), Abs(4)));
    println!("{}", Abs(-2) == Abs(2));
    let s: BTreeSet<Abs> = [Abs(2), Abs(-2), Abs(1)].into_iter().collect();
    println!("{:?}", s);
    let mut m = BTreeMap::new();
    m.insert(Abs(-3), "a"); m.insert(Abs(3), "b"); m.insert(Abs(1), "c");
    println!("{:?}", m);
    a.sort_by_key(|x| std::cmp::Reverse(*x));
    println!("{:?}", a);
    println!("{}", a.contains(&Abs(5)));
    a.dedup();
    println!("{:?} {:?}", a, a.binary_search_by(|p| p.cmp(&Abs(4)).reverse()));
}
