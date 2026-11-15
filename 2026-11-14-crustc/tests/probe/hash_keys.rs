use std::collections::{HashMap, HashSet};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Pos { x: i32, y: i32 }
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key { Name(String), Id(u32) }
fn main() {
    let mut seen: HashSet<Pos> = HashSet::new();
    for (x, y) in [(1, 2), (3, 4), (1, 2)] { seen.insert(Pos { x, y }); }
    let mut v: Vec<_> = seen.iter().copied().collect(); v.sort();
    println!("{} {:?}", seen.len(), v);
    let mut m: HashMap<Key, Vec<i32>> = HashMap::new();
    m.entry(Key::Name("a".into())).or_default().push(1);
    m.entry(Key::Id(7)).or_default().push(2);
    m.entry(Key::Name("a".into())).or_default().push(3);
    println!("{:?} {:?}", m.get(&Key::Name("a".to_string())), m.get(&Key::Id(7)));
    let mut tup: HashMap<(i32, char), usize> = HashMap::new();
    *tup.entry((1, 'a')).or_insert(0) += 5;
    println!("{:?}", tup.get(&(1, 'a')));
    let words: HashSet<&str> = "a b a c".split(' ').collect();
    let mut w: Vec<_> = words.into_iter().collect(); w.sort();
    println!("{:?}", w);
}
