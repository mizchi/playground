fn main() {
    let mut s = String::new();
    for c in ['a', 'b'] {
        s.push(c);
    }
    s.insert(0, 'z');
    s.push_str("yy");
    println!(
        "{} {:?} {} {}",
        s,
        s.find('b'),
        &s[1..3],
        s.chars().nth(1).unwrap()
    );
    let r: String = s
        .chars()
        .filter(|c| c.is_alphabetic())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    println!("{}", r);
}
