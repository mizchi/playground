fn main() {
    let c = 'a';
    println!(
        "{} {} {:?} {} {}",
        c.is_alphabetic(),
        '7'.is_numeric(),
        '7'.to_digit(10),
        c.to_ascii_uppercase(),
        'Z'.is_uppercase()
    );
    let s: String = "abc".chars().rev().collect();
    println!("{}", s);
}
