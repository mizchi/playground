fn main() {
    let v = vec![1, 2, 3, 4, 5];
    for w in v.windows(2) {
        print!("{:?}", w);
    }
    println!();
    for c in v.chunks(2) {
        print!("{:?}", c);
    }
    println!();
    let names = vec!["a".to_string(), "b".to_string()];
    println!("{} {}", names.join(", "), ["x", "y"].concat());
}
