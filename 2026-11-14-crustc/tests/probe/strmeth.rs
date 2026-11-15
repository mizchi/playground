fn main() {
    let s = String::from("Hello World");
    println!(
        "{} {} {}",
        s.to_uppercase(),
        s.to_lowercase(),
        s.replace("o", "0")
    );
    let t = s.clone() + "!" + &String::from("?");
    println!("{} {}", t, "ab".repeat(3));
    let n: i32 = "42".parse().unwrap();
    let m = "x".parse::<i32>().is_err();
    println!("{} {}", n + 1, m);
}
