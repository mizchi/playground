fn main() {
    let v = vec![1, 2, 3, 4, 5];
    let r: Vec<i32> = v.iter().rev().cloned().collect();
    let s: Vec<i32> = (0..10).step_by(3).collect();
    println!("{:?} {:?}", r, s);
    let mut p = v.iter().peekable();
    while let Some(x) = p.next() {
        if let Some(&&y) = p.peek() {
            print!("{}<{} ", x, y);
        }
    }
    println!();
    let f: Vec<i32> = v.iter().flat_map(|x| vec![*x, *x * 10]).collect();
    println!("{:?}", f);
}
