fn main() {
    let o: Option<i32> = Some(3);
    println!(
        "{} {} {:?} {:?}",
        o.map_or(0, |x| x * 2),
        o.unwrap_or_default(),
        o.and_then(|x| if x > 2 { Some(x) } else { None }),
        o.zip(Some('a'))
    );
    let s: Option<String> = Some("hi".to_string());
    println!("{:?}", s.as_deref());
}
