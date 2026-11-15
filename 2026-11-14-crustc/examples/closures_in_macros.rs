// マクロの引数の中のクロージャ: println! / format! / assert! / assert_eq! / vec! の中の map / filter など。
// assert_eq! / assert_ne! は == / != として比べる (Option / Vec / String と &str も)
fn main() {
    let v = vec![1, 2, 3, 4];
    let k = 10;
    println!("{:?}", v.iter().map(|x| x * 2).collect::<Vec<_>>());
    println!("{} {}", v.iter().filter(|&&x| x > 2).count(), v.iter().map(|x| x + k).sum::<i32>());
    assert!(v.iter().all(|x| *x > 0));
    assert_eq!(v.iter().position(|&x| x == 3), Some(2));
    let w = vec![v.iter().map(|x| x * x).sum::<i32>(); 2];
    let s = format!("{:?}", v.iter().filter_map(|x| if x % 2 == 0 { Some(x * 10) } else { None }).collect::<Vec<_>>());
    println!("{:?} {}", w, s);
    let total: i32 = v.iter().map(|x| x * k).sum();
    println!("{}", format!("{}-{}", total, v.iter().any(|&x| x == k)));
    let names = vec![String::from("b"), String::from("a")];
    println!("{:?}", names.iter().map(|s| s.len()).collect::<Vec<usize>>());
    assert_eq!(names.iter().map(|s| s.as_str()).collect::<Vec<_>>(), vec!["b", "a"]);
    assert_ne!(Some(1), None);
    assert_eq!(String::from("x"), "x");
}
