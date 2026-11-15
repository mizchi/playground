use std::collections::VecDeque;
fn main() {
    let mut d = VecDeque::new();
    d.push_back(1);
    d.push_front(0);
    d.push_back(2);
    println!("{:?} {:?} {}", d.pop_front(), d.pop_back(), d.len());
}
