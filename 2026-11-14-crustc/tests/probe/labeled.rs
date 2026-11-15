fn main() {
    let mut n = 0;
    'outer: for i in 0..10 {
        for j in 0..10 {
            if i * j > 20 {
                break 'outer;
            }
            if j > i {
                continue 'outer;
            }
            n += 1;
        }
    }
    let x = loop {
        n += 1;
        if n > 30 {
            break n * 2;
        }
    };
    println!("{} {}", n, x);
}
