fn main() {
    let mut st = vec![1, 2, 3];
    while let Some(t) = st.pop() {
        print!("{} ", t);
    }
    println!();
    let mut v = vec![5, 3, 1];
    v.retain(|x| *x > 2);
    v.dedup();
    v.insert(0, 9);
    v.truncate(2);
    println!("{:?} {:?}", v, v.contains(&9));
}
