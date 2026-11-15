fn caesar(s: &str, k: u8) -> String {
    s.chars().map(|c| {
        if c.is_ascii_lowercase() { (((c as u8 - b'a' + k) % 26) + b'a') as char }
        else if c.is_ascii_uppercase() { (((c as u8 - b'A' + k) % 26) + b'A') as char }
        else { c }
    }).collect()
}
fn is_palindrome(s: &str) -> bool {
    let cs: Vec<char> = s.chars().filter(|c| c.is_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect();
    cs.iter().eq(cs.iter().rev())
}
fn rle(s: &str) -> String {
    let mut out = String::new();
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let mut j = i;
        while j < cs.len() && cs[j] == cs[i] { j += 1; }
        out.push_str(&format!("{}{}", j - i, cs[i]));
        i = j;
    }
    out
}
fn main() {
    println!("{}", caesar("Hello, World!", 3));
    println!("{} {}", is_palindrome("A man, a plan, a canal: Panama"), is_palindrome("abc"));
    println!("{}", rle("aaabccdddd"));
    let csv = "name,age\nbob,30\namy,25";
    let rows: Vec<Vec<&str>> = csv.lines().skip(1).map(|l| l.split(',').collect()).collect();
    let total: u32 = rows.iter().map(|r| r[1].parse::<u32>().unwrap()).sum();
    println!("{:?} {}", rows, total);
    let words = "one two three two one two";
    let mut counts: Vec<(String, usize)> = Vec::new();
    for w in words.split(' ') {
        match counts.iter_mut().find(|(k, _)| k == w) { Some((_, c)) => *c += 1, None => counts.push((w.to_string(), 1)) }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    println!("{:?}", counts);
    let title: String = "hello world".split(' ').map(|w| { let mut c = w.chars(); match c.next() { Some(f) => f.to_uppercase().collect::<String>() + c.as_str(), None => String::new() } }).collect::<Vec<_>>().join(" ");
    println!("{}", title);
}
