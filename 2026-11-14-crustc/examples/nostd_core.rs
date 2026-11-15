// no_std / no_main のプログラム (rustc では libcore だけでビルドする)
#![no_std]
#![no_main]

use core::panic::PanicInfo;

unsafe extern "C" {
    fn putchar(c: i32) -> i32;
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

const N: usize = 8;
const SCALE: i64 = 3 * 7 - 1;
static GREETING: &[u8] = b"no_std!\n";
static TITLE: &str = "core only\n";
static TABLE: [u16; 4] = [1, 10, 100, 1000];
static mut COUNTER: u32 = 0;

fn put(c: u8) {
    unsafe {
        putchar(c as i32);
    }
}

fn puts(s: &[u8]) {
    for &b in s {
        put(b);
    }
}

fn print_u64(mut n: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    if n == 0 {
        put(b'0');
        return;
    }
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    puts(&buf[i..]);
}

fn println_i64(n: i64) {
    if n < 0 {
        put(b'-');
        print_u64(abs_u64(n));
    } else {
        print_u64(n as u64);
    }
    put(b'\n');
}

fn abs_u64(n: i64) -> u64 {
    if n < 0 { (0 - n) as u64 } else { n as u64 }
}

fn bump() -> u32 {
    unsafe {
        COUNTER += 1;
        COUNTER
    }
}

fn parse_u32(s: &[u8]) -> Option<u32> {
    let mut v: u32 = 0;
    if s.is_empty() {
        return None;
    }
    for &c in s {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((c - b'0') as u32)?;
    }
    Some(v)
}

fn checksum(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for &b in data {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    h
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    puts(GREETING);
    puts(TITLE.as_bytes());
    println_i64(TITLE.len() as i64); // 10
    let mut arr = [0i64; N];
    for (i, x) in arr.iter_mut().enumerate() {
        *x = (i as i64) * SCALE;
    }
    println_i64(arr[N - 1]); // 140
    let mut s = 0;
    for t in TABLE.iter() {
        s += *t as i64;
    }
    println_i64(s); // 1111
    bump();
    bump();
    println_i64(bump() as i64); // 3
    println_i64(parse_u32(b"4096").unwrap_or(0) as i64); // 4096
    println_i64(match parse_u32(b"12x") {
        Some(v) => v as i64,
        None => -1,
    }); // -1
    println_i64(match parse_u32(b"99999999999") {
        Some(v) => v as i64,
        None => -2,
    }); // -2 (オーバーフロー)
    println_i64(checksum(b"hello") as i64);
    println_i64(u8::MAX as i64 + i16::MIN as i64); // 255 - 32768
    println_i64((37u32.pow(3) % 1000) as i64); // 653
    println_i64(0b1011_0000u8.count_ones() as i64 * 10 + 0x0Fu16.leading_zeros() as i64); // 3*10 + 12
    println_i64(7i64.min(3).max(-2)); // 3
    println_i64(abs_u64(-42) as i64); // 42
    let c = 'Z';
    println_i64(c as i64 + ('a' as u8).to_ascii_uppercase() as i64); // 90 + 65
    let x = 1234u32;
    let p = &x as *const u32;
    println_i64(unsafe { *p } as i64); // 1234
    let mut ys = [1u8, 2, 3, 4];
    let q = ys.as_mut_ptr();
    unsafe {
        q.add(2).write(30);
    }
    println_i64(ys[2] as i64); // 30
    (s % 256) as i32
}
