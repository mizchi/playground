fn check(n: i32) -> Result<i32, String> { if n > 5 { Err(format!("{} too big", n)) } else { Ok(n) } }
fn main() {
    let v = check(3).unwrap();
    println!("{}", v);
    match check(9) {
        Ok(_) => {}
        Err(e) => { println!("error: {}", e); std::process::exit(3); }
    }
    println!("unreachable");
}
