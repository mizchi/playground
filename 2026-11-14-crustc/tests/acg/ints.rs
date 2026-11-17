// ast_codegen v0: 整数・制御構文・関数呼び出し・extern "C"
#![no_std]
#![no_main]

use core::panic::PanicInfo;

unsafe extern "C" {
    fn putchar(c: i32) -> i32;
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

fn put(c: u8) {
    unsafe {
        putchar(c as i32);
    }
}

fn print_u(mut n: u64) {
    let mut d: u64 = 1;
    while d * 10 <= n {
        d *= 10;
    }
    loop {
        put(b'0' + (n / d) as u8);
        n %= d;
        if d == 1 {
            break;
        }
        d /= 10;
    }
}

fn print_i(n: i64) {
    if n < 0 {
        put(b'-');
        print_u((0 - n) as u64);
    } else {
        print_u(n as u64);
    }
}

fn line(n: i64) {
    print_i(n);
    put(b'\n');
}

fn fib(n: u32) -> u64 {
    if n < 2 { n as u64 } else { fib(n - 1) + fib(n - 2) }
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

fn is_prime(n: u32) -> bool {
    if n < 2 {
        return false;
    }
    let mut i = 2;
    while i * i <= n {
        if n % i == 0 {
            return false;
        }
        i += 1;
    }
    true
}

fn collatz(mut n: u64) -> u32 {
    let mut steps = 0;
    while n != 1 {
        n = if n % 2 == 0 { n / 2 } else { 3 * n + 1 };
        steps += 1;
    }
    steps
}

fn bits(x: u32) -> u32 {
    let mut c = 0;
    let mut v = x;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    line(fib(30) as i64);
    line(gcd(1071, 462));
    let mut primes = 0;
    let mut k: u32 = 0;
    while k < 1000 {
        if is_prime(k) {
            primes += 1;
        }
        k += 1;
    }
    line(primes);
    line(collatz(27) as i64);
    line(bits(0xF0F0) as i64);
    let a: i32 = -17;
    let b: i32 = 5;
    line((a / b) as i64);
    line((a % b) as i64);
    line(((a as u32) >> 28) as i64);
    line((a >> 2) as i64);
    let x: u8 = 250;
    let y = x + 0;
    line((y as i8) as i64);
    let ok = a < 0 && b > 0 || false;
    line(ok as i64);
    let c = 'A';
    line(c as i64 + 1);
    line(-7i64 * 3 + (100 ^ 7) - (1 << 4));
    0
}
