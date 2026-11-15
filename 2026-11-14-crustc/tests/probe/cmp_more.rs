use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::cmp::Reverse;
#[derive(Debug, PartialEq, Eq)]
struct Task { pri: u32, name: String }
impl Ord for Task { fn cmp(&self, o: &Self) -> Ordering { self.pri.cmp(&o.pri).then_with(|| o.name.cmp(&self.name)) } }
impl PartialOrd for Task { fn partial_cmp(&self, o: &Self) -> Option<Ordering> { Some(self.cmp(o)) } }
fn main() {
    let mut h = BinaryHeap::new();
    h.push(Task { pri: 2, name: "b".into() });
    h.push(Task { pri: 5, name: "a".into() });
    h.push(Task { pri: 2, name: "a".into() });
    while let Some(t) = h.pop() { print!("{}{} ", t.pri, t.name); }
    println!();
    println!("{:?} {:?}", Ord::cmp(&3, &5), 2.5f64.partial_cmp(&1.0));
    let mut mh = BinaryHeap::new();
    for x in [5, 1, 8, 3] { mh.push(Reverse(x)); }
    let firsts: Vec<i32> = (0..2).map(|_| mh.pop().unwrap().0).collect();
    println!("{:?} {:?}", firsts, mh.peek());
    let mut v = vec!["bb", "a", "ccc"];
    v.sort_by(|a, b| match a.len().cmp(&b.len()) { Ordering::Equal => a.cmp(b), o => o.reverse() });
    println!("{:?}", v);
    println!("{:?}", "abc".cmp("abd"));
    println!("{:?}", [1, 2].cmp(&[1, 3]));
    println!("{}", std::cmp::max("x", "y"));
}
