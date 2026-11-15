use std::collections::{HashMap, HashSet, BTreeMap};
fn main() {
    let mut m = HashMap::from([("a", 1), ("b", 2), ("c", 3)]);
    m.retain(|_, v| *v != 2);
    let mut ks: Vec<_> = m.keys().copied().collect(); ks.sort();
    println!("{:?}", ks);
    let mut m2: HashMap<&str, i32> = HashMap::new();
    m2.extend(vec![("x", 1), ("y", 2)]);
    m2.extend(m.iter().map(|(k, v)| (*k, *v * 10)));
    let mut v: Vec<_> = m2.into_iter().collect(); v.sort();
    println!("{:?}", v);
    let mut s = String::from("ab");
    s.extend(['c', 'd'].iter());
    s.extend(vec!["ef", "g"]);
    println!("{}", s);
    let hs: HashSet<i32> = HashSet::from([1, 2, 3]);
    let hs2: HashSet<i32> = [2, 3, 4].into_iter().collect();
    let mut i: Vec<_> = hs.intersection(&hs2).copied().collect(); i.sort();
    let mut u: Vec<_> = hs.union(&hs2).copied().collect(); u.sort();
    let mut d: Vec<_> = hs.difference(&hs2).copied().collect(); d.sort();
    println!("{:?} {:?} {:?}", i, u, d);
    let bt = BTreeMap::from([(3, 'c'), (1, 'a')]);
    println!("{:?} {:?} {:?}", bt, bt.first_key_value(), bt.keys().rev().collect::<Vec<_>>());
    let mut counts = HashMap::new();
    for c in "hello".chars() { *counts.entry(c).or_insert(0) += 1; }
    let mut cv: Vec<(char, i32)> = counts.into_iter().collect(); cv.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("{:?}", cv);
    let mut drained: Vec<_> = HashMap::from([(1, 2)]).drain().collect(); drained.sort();
    println!("{:?}", drained);
}
