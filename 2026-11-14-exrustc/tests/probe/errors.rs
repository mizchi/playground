use std::fmt;
use std::num::ParseIntError;
#[derive(Debug)]
enum AppError { Parse(ParseIntError), Negative(i64), Empty }
impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Parse(e) => write!(f, "parse error: {}", e),
            AppError::Negative(n) => write!(f, "negative: {}", n),
            AppError::Empty => write!(f, "empty input"),
        }
    }
}
impl From<ParseIntError> for AppError { fn from(e: ParseIntError) -> Self { AppError::Parse(e) } }
fn parse_pos(s: &str) -> Result<i64, AppError> {
    if s.is_empty() { return Err(AppError::Empty); }
    let n: i64 = s.trim().parse()?;
    if n < 0 { Err(AppError::Negative(n)) } else { Ok(n) }
}
fn sum_all(xs: &[&str]) -> Result<i64, AppError> {
    let mut t = 0;
    for x in xs { t += parse_pos(x)?; }
    Ok(t)
}
fn main() {
    for s in ["42", " 7 ", "-3", "abc", ""] {
        match parse_pos(s) { Ok(n) => println!("ok {}", n), Err(e) => println!("err {}", e) }
    }
    println!("{:?}", sum_all(&["1", "2", "3"]).unwrap());
    if let Err(e) = sum_all(&["1", "x"]) { println!("{} / {:?}", e, e); }
    let r: Result<i32, String> = Err("bad".into());
    println!("{}", r.clone().unwrap_or(-1));
    let e = r.map_err(|s| s.len()).unwrap_err();
    println!("{}", e);
}
