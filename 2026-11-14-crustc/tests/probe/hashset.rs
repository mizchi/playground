use std::collections::HashSet;
fn main() {
    let mut s = HashSet::new();
    s.insert(3);
    s.insert(5);
    s.insert(3);
    println!("{} {} {}", s.len(), s.contains(&5), s.contains(&4));
}
