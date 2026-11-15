// オーバーフロー検査: 途中まで出力して、足し算のあふれで panic (終了コード 101)
fn fact(n: u32) -> u32 { if n == 0 { 1 } else { n * fact(n - 1) } }
fn main() {
    for n in [5, 10, 12] { println!("{}! = {}", n, fact(n)); }
    let v: Vec<u8> = vec![100, 100];
    println!("{}", v.iter().map(|&x| x as u32).sum::<u32>());
    println!("{:?} {}", 250u8.checked_add(10), 250u8.wrapping_add(10));
    println!("{}", fact(13));
}
