// rustc でもそのままコンパイルできる exrustc サブセットのサンプル
unsafe extern "C" {
    fn putchar(c: i32) -> i32;
}

fn put(c: u8) {
    unsafe {
        putchar(c as i32);
    }
}

fn print_u64(n: u64) {
    if n >= 10 {
        print_u64(n / 10);
    }
    put(b'0' + (n % 10) as u8);
}

fn print_i64(n: i64) {
    if n < 0 {
        put(b'-');
        print_u64((0 - n) as u64);
    } else {
        print_u64(n as u64);
    }
}

fn newline() {
    put(b'\n');
}

fn fib(n: u32) -> u64 {
    let mut a: u64 = 0;
    let mut b: u64 = 1;
    let mut i = 0;
    while i < n {
        let t = a + b;
        a = b;
        b = t;
        i += 1;
    }
    a
}

fn is_prime(n: u64) -> bool {
    if n < 2 {
        return false;
    }
    let mut d = 2;
    while d * d <= n {
        if n % d == 0 {
            return false;
        }
        d += 1;
    }
    true
}

fn collatz(mut n: u64) -> u32 {
    let mut steps = 0;
    loop {
        if n == 1 {
            break;
        }
        n = if n % 2 == 0 { n / 2 } else { 3 * n + 1 };
        steps += 1;
    }
    steps
}

// main から呼ばれないので、型検査もコード生成もされない
fn unused_helper(x: i64) -> i64 {
    x * 2
}

fn main() {
    print_u64(fib(50));
    newline();
    let mut count = 0;
    let mut n = 0;
    while n < 1000 {
        if is_prime(n) && n % 10 == 7 {
            count += 1;
        }
        n += 1;
    }
    print_i64(count);
    newline();
    print_i64(-12345);
    newline();
    print_u64(collatz(27) as u64);
    newline();
    let flag = !(1 > 2) || false;
    print_i64(if flag { 1 } else { 0 });
    newline();
    print_i64((-7) / 2 + (-7) % 2 + (1 << 4) + (0xff & 0x0f) - (100 >> 2));
    newline();
}
