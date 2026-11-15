use std::collections::BTreeMap;
fn main() {
    let mut m = BTreeMap::new();
    m.insert("b", 2);
    m.insert("a", 1);
    for (k, v) in &m {
        println!("{} {}", k, v);
    }
}
