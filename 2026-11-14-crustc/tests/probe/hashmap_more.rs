use std::collections::{HashMap, HashSet, BTreeMap};
fn main() {
    let text = "the cat the dog the end";
    let mut freq: HashMap<&str, usize> = HashMap::new();
    for w in text.split_whitespace() { *freq.entry(w).or_insert(0) += 1; }
    if let Some(x) = freq.get_mut("cat") { *x += 10; }
    let mut items: Vec<_> = freq.iter().collect();
    items.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    println!("{:?}", items);
    println!("{} {:?} {}", freq.contains_key("dog"), freq.get("zzz"), freq.len());
    freq.remove("end");
    let keys: HashSet<&str> = freq.keys().copied().collect();
    let mut kv: Vec<_> = keys.into_iter().collect(); kv.sort();
    println!("{:?}", kv);
    let bt: BTreeMap<i32, &str> = vec![(3, "c"), (1, "a"), (2, "b")].into_iter().collect();
    println!("{:?} {:?}", bt, bt.range(2..).collect::<Vec<_>>());
    let mut groups: HashMap<usize, Vec<&str>> = HashMap::new();
    for w in ["aa", "b", "cc", "ddd"] { groups.entry(w.len()).or_default().push(w); }
    let mut g: Vec<_> = groups.into_iter().collect(); g.sort();
    println!("{:?}", g);
    let total: usize = freq.values().sum();
    println!("{}", total);
    let mut m2 = HashMap::new();
    m2.insert(String::from("k"), vec![1]);
    m2.entry("k".to_string()).and_modify(|v| v.push(2)).or_insert_with(Vec::new);
    println!("{:?}", m2.get("k"));
}
