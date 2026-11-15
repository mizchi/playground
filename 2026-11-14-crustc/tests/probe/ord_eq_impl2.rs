// 手書きの Ord / Eq (enum・Reverse・タプルと Option の中・配列の ==・境界 T: Eq)
use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
#[derive(Debug, Clone, Copy)]
enum Suit { Clubs, Hearts }
impl PartialEq for Suit { fn eq(&self, o: &Self) -> bool { (*self as u8) == (*o as u8) } }
impl Eq for Suit {}
impl Ord for Suit {
    fn cmp(&self, o: &Self) -> Ordering { (*o as u8).cmp(&(*self as u8)) }
}
impl PartialOrd for Suit { fn partial_cmp(&self, o: &Self) -> Option<Ordering> { Some(self.cmp(o)) } }
#[derive(Debug, Clone, PartialEq, Eq)]
struct Name(String);
impl Ord for Name {
    fn cmp(&self, o: &Self) -> Ordering { self.0.len().cmp(&o.0.len()).then(self.0.cmp(&o.0)) }
}
impl PartialOrd for Name { fn partial_cmp(&self, o: &Self) -> Option<Ordering> { Some(self.cmp(o)) } }
#[derive(Debug)]
struct Ci(String);
impl PartialEq for Ci { fn eq(&self, o: &Ci) -> bool { self.0.to_lowercase() == o.0.to_lowercase() } }
impl Eq for Ci {}
fn uniq<T: Eq + Clone>(v: &[T]) -> Vec<T> {
    let mut out: Vec<T> = vec![];
    for x in v { if !out.contains(x) { out.push(x.clone()); } }
    out
}
fn main() {
    println!("{:?} {}", Suit::Clubs.cmp(&Suit::Hearts), Suit::Clubs > Suit::Hearts);
    let mut ns = vec![Name("ccc".into()), Name("a".into()), Name("bb".into()), Name("ab".into())];
    ns.sort_unstable();
    println!("{:?}", ns);
    println!("{:?}", ns.iter().max_by_key(|n| (*n).clone()));
    println!("{:?}", (1, Name("zz".into())).cmp(&(1, Name("b".into()))));
    println!("{}", Some(Name("a".into())) < Some(Name("bb".into())));
    let mut h = BinaryHeap::new();
    for n in ns.iter().cloned() { h.push(Reverse(n)); }
    println!("{:?}", h.pop().map(|r| r.0));
    println!("{}", Ci("Hello".into()) == Ci("hELLO".into()));
    println!("{}", vec![Ci("A".into())] == vec![Ci("a".into())]);
    println!("{}", [Ci("x".into())] != [Ci("X".into())]);
    let names = vec![Name("x".into()), Name("y".into()), Name("x".into())];
    println!("{:?}", uniq(&names));
    assert_eq!(Ci("Q".into()), Ci("q".into()));
    let mut ss = vec![Suit::Clubs, Suit::Hearts, Suit::Clubs];
    ss.sort();
    println!("{:?} {:?}", ss, ss.iter().min());
    println!("{:?}", Name("b".into()).max(Name("aa".into())));
}
