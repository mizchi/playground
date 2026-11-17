// panic の場所 (行:桁) を rustc と比べる (tests/panic_loc.rs)。引数の番号の場合で panic する
fn pick(k: usize) -> i32 {
    let v = vec![1, 2, 3];
    let a = [10u8, 20];
    let mut x: u8 = 250;
    let o: Option<i32> = None;
    match k {
        0 => v[k + 5],
        1 => a[k + 4] as i32,
        2 => { x += 10; x as i32 }
        3 => o.unwrap(),
        4 => { assert!(k == 0, "k was {}", k); 0 }
        5 => { let d = k as i32 - 5; 10 / d }
        _ => { let s = format!("{}", v.iter().map(|q| q * 2).sum::<i32>()); s.parse::<i32>().expect("num") + o.expect("none") }
    }
}
fn main() {
    let k: usize = std::env::args().nth(1).unwrap().parse().unwrap();
    println!("{}", pick(k));
}
