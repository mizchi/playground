fn main() {
    let v = vec![3, 4];
    let Some(x) = v.first() else {
        return;
    };
    if let [a, b] = v[..] {
        println!("{} {} {}", x, a, b);
    }
    let t = (1, (2, 3));
    let (p, (q, r)) = t;
    println!("{}", p + q + r);
}
