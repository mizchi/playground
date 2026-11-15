fn make_counter() -> impl FnMut() -> u32 { let mut c = 0; move || { c += 1; c } }
fn compose<A, B, C>(f: impl Fn(A) -> B, g: impl Fn(B) -> C) -> impl Fn(A) -> C { move |x| g(f(x)) }
fn apply_n(f: &dyn Fn(i32) -> i32, n: usize, x: i32) -> i32 { (0..n).fold(x, |acc, _| f(acc)) }
fn main() {
    let mut c = make_counter();
    println!("{} {} {}", c(), c(), c());
    let h = compose(|x: i32| x + 1, |y: i32| y * 10);
    println!("{}", h(4));
    println!("{}", apply_n(&|x| x * 2, 5, 1));
    let mut log = Vec::new();
    let mut push = |s: &str| log.push(s.to_uppercase());
    push("a"); push("b");
    println!("{:?}", log);
    let mut ops: Vec<Box<dyn Fn(i32) -> i32>> = Vec::new();
    for k in 1..=3 { ops.push(Box::new(move |x| x * k)); }
    println!("{:?}", ops.iter().map(|f| f(10)).collect::<Vec<_>>());
    let mut v = vec![5, 3, 8];
    let mut swaps = 0;
    v.sort_by(|a, b| { swaps += 1; a.cmp(b) });
    println!("{:?} {}", v, swaps > 0);
    let add = |a: i32, b: i32| -> i32 { a + b };
    println!("{}", add(3, 4));
}
