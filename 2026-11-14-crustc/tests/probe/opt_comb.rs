fn half(x: i32) -> Option<i32> { if x % 2 == 0 { Some(x / 2) } else { None } }
fn parse(s: &str) -> Result<i32, String> { s.parse::<i32>().map_err(|e| format!("bad {}: {}", s, e)) }
fn sum(a: &str, b: &str) -> Result<i32, String> { Ok(parse(a)? + parse(b)?) }
fn main() {
    println!("{:?} {:?}", half(8).and_then(half), half(6).and_then(half));
    println!("{} {}", half(3).map_or(-1, |x| x * 10), half(4).unwrap_or_else(|| 0));
    println!("{:?} {:?}", half(4).filter(|&x| x > 5), half(4).ok_or("odd"));
    println!("{:?} {:?}", Some(1).zip(Some("a")), Some(2).xor(None::<i32>));
    let mut o = Some(5);
    println!("{:?} {:?}", o.take(), o);
    println!("{:?} {:?}", sum("1", "2"), sum("1", "x"));
    println!("{}", parse("7").unwrap_or_default() + parse("z").unwrap_or_default());
    println!("{:?}", parse("9").ok());
    println!("{}", Some(3).is_some_and(|x| x > 2));
    let v: Option<Vec<i32>> = vec!["1", "2"].iter().map(|s| s.parse().ok()).collect();
    println!("{:?}", v);
    let r: Result<Vec<i32>, String> = vec!["1", "q"].iter().map(|s| parse(s)).collect();
    println!("{:?}", r);
}
