fn main() {
    let s = "a,b,,c";
    let parts: Vec<&str> = s.split(',').collect();
    let words: Vec<String> = "x y z".split_whitespace().map(String::from).collect();
    println!("{:?} {:?} {}", parts, words, s.len());
    for (i, c) in "héllo".char_indices() {
        print!("{}{} ", i, c);
    }
    println!();
    let b = s.bytes().filter(|b| *b == b',').count();
    println!("{}", b);
}
