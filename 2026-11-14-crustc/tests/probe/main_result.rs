use std::fmt;
#[derive(Debug)]
struct MyErr(String);
impl fmt::Display for MyErr { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "MyErr: {}", self.0) } }
impl std::error::Error for MyErr {}
fn step(n: i32) -> Result<i32, Box<dyn std::error::Error>> {
    if n > 2 { return Err(Box::new(MyErr(format!("too big {}", n)))); }
    let v: i32 = "40".parse()?;
    Ok(v + n)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", step(1)?);
    match step(5) { Ok(v) => println!("{}", v), Err(e) => println!("caught: {}", e) }
    let r: Result<i32, String> = Err("x".into());
    println!("{}", r.unwrap_or_else(|e| e.len() as i32));
    Ok(())
}
