use std::fmt::Display;
fn show(x: impl Display) -> String { format!("<{}>", x) }
fn total<I: Iterator<Item = u32>>(it: I) -> u32 { it.sum() }
fn main() {
    println!("{} {}", show(5), show("hi"));
    println!("{:?}", (1..).map(|x: u64| x * x).take(5).collect::<Vec<_>>());
    println!("{}", total((0..7u32).filter(|x| x % 2 == 0)));
    let v: Vec<String> = vec!["a", "b"].into_iter().map(String::from).collect();
    println!("{:?}", v);
    let lens: Vec<usize> = v.iter().map(String::len).collect();
    println!("{:?}", lens);
    let s = "a1b2c3";
    println!("{}", s.chars().filter(char::is_ascii_digit).count());
    println!("{:?}", (0..).zip("xyz".chars()).collect::<Vec<_>>());
}
