// コレクションのサンプル: HashSet (insert / contains / remove / 和・積・差 / collect)、BTreeMap (キーの順に回る・
// entry / or_insert / or_default / first_key_value / pop_first / m[&k] / rev)、BTreeSet、VecDeque (両端の push / pop・
// d[i] / iter)、手書きの impl Index / IndexMut
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::ops::{Index, IndexMut};

struct Grid {
    w: usize,
    cells: Vec<u8>,
}

impl Index<(usize, usize)> for Grid {
    type Output = u8;
    fn index(&self, p: (usize, usize)) -> &u8 {
        &self.cells[p.1 * self.w + p.0]
    }
}

impl IndexMut<(usize, usize)> for Grid {
    fn index_mut(&mut self, p: (usize, usize)) -> &mut u8 {
        &mut self.cells[p.1 * self.w + p.0]
    }
}

fn sorted(s: &HashSet<i32>) -> Vec<i32> {
    let mut v: Vec<i32> = s.iter().copied().collect();
    v.sort();
    v
}

fn main() {
    // HashSet
    let mut a = HashSet::new();
    for x in [3, 1, 4, 1, 5, 9, 2, 6] {
        a.insert(x);
    }
    let b: HashSet<i32> = (4..8).collect();
    println!("{} {} {} {}", a.len(), a.contains(&4), a.insert(4), a.remove(&9));
    let mut u: Vec<i32> = a.union(&b).copied().collect();
    let mut i: Vec<i32> = a.intersection(&b).copied().collect();
    let mut d: Vec<&i32> = a.difference(&b).collect();
    u.sort();
    i.sort();
    d.sort();
    println!("{:?} {:?} {:?} {:?}", sorted(&a), u, i, d);
    let words: HashSet<&str> = "a b a c b".split(' ').collect();
    println!("{}", words.len());
    // BTreeMap
    let mut m = BTreeMap::new();
    for w in "the quick brown fox jumps over the lazy dog the end".split_whitespace() {
        *m.entry(w).or_insert(0) += 1;
    }
    println!("{:?}", m);
    for (k, v) in &m {
        if *v > 1 {
            println!("{} {}", k, v);
        }
    }
    println!("{:?} {:?} {} {:?}", m.first_key_value(), m.last_key_value(), m["fox"], m.get("cat"));
    let mut lens: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
    for k in m.keys() {
        lens.entry(k.len()).or_default().push(k);
    }
    println!("{:?}", lens);
    let rev: Vec<usize> = lens.iter().rev().map(|(k, _)| *k).collect();
    println!("{:?} {:?} {}", rev, lens.pop_first(), lens.len());
    let mut scores: BTreeMap<String, u32> = BTreeMap::new();
    scores.insert(String::from("bo"), 3);
    scores.insert(String::from("al"), 5);
    let old = scores.insert(String::from("bo"), 7);
    if let Some(s) = scores.get_mut("al") {
        *s += 1;
    }
    println!("{:?} {:?}", scores, old);
    let gone = scores.remove("al");
    println!("{:?} {}", gone, scores.contains_key("al"));
    // BTreeSet
    let set: BTreeSet<char> = "hello world".chars().filter(|c| c.is_alphabetic()).collect();
    println!("{:?} {:?} {:?}", set, set.first(), set.iter().rev().next());
    // VecDeque
    let mut q = VecDeque::new();
    for k in 0..6 {
        if k % 2 == 0 {
            q.push_back(k);
        } else {
            q.push_front(k);
        }
    }
    println!("{:?} {} {:?} {:?}", q, q[1], q.front(), q.back());
    q[0] = 100;
    let mut order = vec![];
    while let Some(x) = q.pop_front() {
        order.push(x);
        if x == 0 {
            q.push_back(-1);
        }
    }
    println!("{:?} {}", order, q.is_empty());
    let mut bfs: VecDeque<(u32, u32)> = VecDeque::new();
    bfs.push_back((0, 1));
    let mut seen = 0;
    while let Some((d, n)) = bfs.pop_front() {
        seen += n;
        if d < 3 {
            bfs.push_back((d + 1, n * 2));
            bfs.push_back((d + 1, n * 2 + 1));
        }
    }
    let names: VecDeque<String> = vec![String::from("x"), String::from("y")].into_iter().collect();
    println!("{} {:?} {}", seen, names, names.iter().map(|s| s.len()).sum::<usize>());
    // Index / IndexMut
    let mut g = Grid { w: 3, cells: vec![0; 9] };
    g[(1, 2)] = 7;
    g[(0, 0)] += 2;
    println!("{} {} {:?}", g[(1, 2)], g[(0, 0)], g.cells);
}
