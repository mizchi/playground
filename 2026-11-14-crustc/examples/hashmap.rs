// HashMap のサンプル: insert / get / 添字 / entry().or_insert() で単語を数える / remove / contains_key /
// derive(Hash) した struct をキーに / for で回す / values_mut / collect で作る。&str の split も使う
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct P(i32, i32);

fn main() {
    let mut m: HashMap<String, i32> = HashMap::new();
    m.insert(String::from("a"), 1);
    m.insert("b".to_string(), 2);
    println!("{} {:?} {:?}", m.len(), m.get(&String::from("a")), m.get(&"zz".to_string()));
    let old = m.insert(String::from("a"), 10);
    println!("{:?} {}", old, m[&String::from("a")]);
    let text = "the cat and the dog and the bird";
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for w in text.split(' ') {
        *counts.entry(w).or_insert(0) += 1;
    }
    let mut ks: Vec<&str> = counts.keys().copied().collect();
    ks.sort();
    for k in &ks {
        println!("{k}: {}", counts[k]);
    }
    let mut grid: HashMap<P, char> = HashMap::new();
    for i in 0..50 {
        grid.insert(P(i, i * 2), if i % 2 == 0 { 'e' } else { 'o' });
    }
    println!("{} {:?} {}", grid.len(), grid.get(&P(3, 6)), grid.contains_key(&P(3, 7)));
    println!("{:?}", grid.remove(&P(4, 8)));
    println!("{} {}", grid.len(), grid.contains_key(&P(4, 8)));
    let mut total = 0;
    for (p, c) in &grid {
        if *c == 'e' {
            total += p.0;
        }
    }
    println!("{total}");
    let mut sq: HashMap<u64, u64> = (1..=10).map(|x| (x, x * x)).collect();
    for v in sq.values_mut() {
        *v += 1;
    }
    let mut s: u64 = sq.values().sum();
    println!("{s}");
    sq.clear();
    s = sq.len() as u64;
    println!("{s}");
}
