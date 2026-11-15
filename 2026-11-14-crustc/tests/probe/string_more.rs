use std::fmt::Write;
fn main() {
    let s = "Hello, World! 123";
    println!("{}", s.chars().filter(|c| c.is_alphabetic()).collect::<String>());
    println!("{:?}", s.split(", ").collect::<Vec<_>>());
    println!("{}", s.replace("l", "L"));
    println!("{} {}", s.to_uppercase(), s.to_lowercase());
    println!("{:?} {:?}", s.strip_prefix("Hello"), s.strip_suffix("xyz"));
    println!("{}", s.chars().rev().collect::<String>());
    println!("{:?}", s.char_indices().filter(|(_, c)| *c == 'o').map(|(i, _)| i).collect::<Vec<_>>());
    let mut out = String::new();
    write!(out, "{}-{}", 1, 2).unwrap();
    writeln!(out, "!").unwrap();
    print!("{}", out);
    println!("{}", "ab".repeat(3));
    let nums: Vec<i32> = "1,2,,3".split(',').filter_map(|x| x.parse().ok()).collect();
    println!("{:?}", nums);
    println!("{:?}", String::from_utf8(vec![104, 105]));
    println!("{}", s.bytes().filter(|b| b.is_ascii_digit()).count());
    println!("{:?}", s.split_once(' '));
    println!("{}", s.trim_matches(|c: char| !c.is_alphabetic()));
    println!("{:?}", "a b  c".split_whitespace().rev().collect::<Vec<_>>());
    let mut t = String::from("abc"); t.insert(1, 'X'); t.truncate(3); let p = t.pop(); println!("{} {:?}", t, p);
    println!("{}", char::from_digit(7, 10).unwrap());
    println!("{}", (b'a' + 2) as char);
    println!("{:>8}|{:^8}|{:<8}|", "r", "c", "l");
    println!("{:?}", "x=1;y=2".split(';').map(|kv| kv.split('=').collect::<Vec<_>>()).collect::<Vec<_>>());
}
