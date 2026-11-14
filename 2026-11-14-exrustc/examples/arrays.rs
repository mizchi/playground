// タプル・配列・スライス・for のサンプル (rustc でもそのままコンパイルできる)
unsafe extern "C" {
    fn putchar(c: i32) -> i32;
}

fn put(c: u8) {
    unsafe {
        putchar(c as i32);
    }
}

fn print_i64(n: i64) {
    if n < 0 {
        put(b'-');
        print_i64(0 - n);
        return;
    }
    if n >= 10 {
        print_i64(n / 10);
    }
    put(b'0' + (n % 10) as u8);
}

fn println_i64(n: i64) {
    print_i64(n);
    put(b'\n');
}

fn sum(xs: &[i64]) -> i64 {
    let mut s = 0;
    for x in xs {
        s += *x;
    }
    s
}

fn max_index(xs: &[i64]) -> Option<usize> {
    let mut best: Option<(usize, i64)> = None;
    for (i, &x) in xs.iter().enumerate() {
        match best {
            Some((_, b)) if b >= x => {}
            _ => best = Some((i, x)),
        }
    }
    match best {
        Some((i, _)) => Some(i),
        None => None,
    }
}

fn bubble_sort(xs: &mut [i64]) {
    let n = xs.len();
    for i in 0..n {
        for j in 0..n - 1 - i {
            if xs[j] > xs[j + 1] {
                xs.swap(j, j + 1);
            }
        }
    }
}

fn divmod(a: i64, b: i64) -> (i64, i64) {
    (a / b, a % b)
}

fn fill(buf: &mut [u8; 8], v: u8) {
    for b in buf.iter_mut() {
        *b = v;
    }
}

#[derive(Clone, Copy)]
struct Point {
    x: i64,
    y: i64,
}

fn main() {
    let a = [5, 3, 9, 1, 7];
    println_i64(sum(&a)); // 25
    println_i64(sum(&a[1..4])); // 13
    println_i64(sum(&a[..2]) + sum(&a[3..])); // 8 + 8 = 16
    println_i64(a.len() as i64); // 5
    println_i64(max_index(&a).unwrap() as i64); // 2
    println_i64(if max_index(&[]).is_none() { 1 } else { 0 }); // 1

    let mut b = [4, 8, 1, 9, 2, 6];
    bubble_sort(&mut b);
    for x in b {
        print_i64(x);
    }
    put(b'\n'); // 124689

    let (q, r) = divmod(17, 5);
    println_i64(q * 10 + r); // 32
    let t = (1u8, true, -4i64);
    println_i64(t.0 as i64 + if t.1 { 10 } else { 0 } + t.2); // 7

    let mut buf = [0u8; 8];
    fill(&mut buf, 3);
    buf[2] = 9;
    let mut acc = 0u32;
    for v in buf.iter() {
        acc = acc * 2 + *v as u32;
    }
    println_i64(acc as i64); // 3*(255) with buf[2]=9 → compute

    let mut total = 0;
    for i in 1..=10 {
        total += i;
    }
    println_i64(total); // 55
    let mut rev = 0;
    for i in (0..5).rev() {
        rev = rev * 10 + i;
    }
    println_i64(rev); // 43210
    let mut cnt = 0;
    for i in (1..=3).rev() {
        cnt = cnt * 10 + i;
    }
    println_i64(cnt); // 321

    let pts = [Point { x: 1, y: 2 }, Point { x: 3, y: 4 }, Point { x: 5, y: 6 }];
    let mut sx = 0;
    for p in &pts {
        sx += p.x * p.y;
    }
    println_i64(sx); // 2 + 12 + 30 = 44
    let grid = [[1, 2, 3], [4, 5, 6]];
    let mut g = 0;
    for row in &grid {
        for c in row.iter() {
            g = g * 2 + *c;
        }
    }
    println_i64(g);
    println_i64(*a.first().unwrap() + *a.last().unwrap()); // 12
    println_i64(match a.get(10) {
        Some(v) => *v,
        None => -1,
    }); // -1
    println_i64(if a.contains(&9) { 1 } else { 0 }); // 1
    let mut skip = 0;
    for x in a {
        if x == 3 {
            continue;
        }
        if x == 1 {
            break;
        }
        skip += x;
    }
    println_i64(skip); // 5 + 9 = 14
    let words: [(u8, i64); 3] = [(1, 10), (2, 20), (3, 30)];
    let mut ws = 0;
    for &(k, v) in &words {
        ws += k as i64 * v;
    }
    println_i64(ws); // 140
}
