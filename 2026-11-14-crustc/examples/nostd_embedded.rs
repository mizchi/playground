// 組み込みっぽい no_std プログラム: リングバッファ・状態機械・trait とジェネリクス・volatile
#![no_std]
#![no_main]

use core::cmp::{max, min};
use core::panic::PanicInfo;

unsafe extern "C" {
    fn putchar(c: i32) -> i32;
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

fn put(c: u8) {
    unsafe {
        putchar(c as i32);
    }
}

fn print_u32(n: u32) {
    if n >= 10 {
        print_u32(n / 10);
    }
    put(b'0' + (n % 10) as u8);
}

fn line(n: u32) {
    print_u32(n);
    put(b'\n');
}

const CAP: usize = 4;

struct Ring {
    buf: [u16; CAP],
    head: usize,
    len: usize,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum RingError {
    Full,
    Empty,
}

impl Ring {
    const fn new() -> Ring {
        Ring { buf: [0; CAP], head: 0, len: 0 }
    }
    fn push(&mut self, v: u16) -> Result<(), RingError> {
        if self.len == CAP {
            return Err(RingError::Full);
        }
        let i = (self.head + self.len) % CAP;
        self.buf[i] = v;
        self.len += 1;
        Ok(())
    }
    fn pop(&mut self) -> Result<u16, RingError> {
        if self.len == 0 {
            return Err(RingError::Empty);
        }
        let v = self.buf[self.head];
        self.head = (self.head + 1) % CAP;
        self.len -= 1;
        Ok(v)
    }
}

trait Checksum {
    fn fold(&self) -> u32;
    fn tagged(&self) -> u32 {
        self.fold() ^ 0xA5
    }
}

impl Checksum for u16 {
    fn fold(&self) -> u32 {
        (*self as u32 >> 8) + (*self as u32 & 0xff)
    }
}

impl Checksum for Ring {
    fn fold(&self) -> u32 {
        let mut s = 0;
        let mut i = 0;
        while i < self.len {
            s += self.buf[(self.head + i) % CAP].fold();
            i += 1;
        }
        s
    }
}

fn sum_tags<T: Checksum>(xs: &[T]) -> u32 {
    let mut s: u32 = 0;
    for x in xs {
        s = s.wrapping_add(x.tagged());
    }
    s
}

fn clamp<T: Ord + Copy>(v: T, lo: T, hi: T) -> T {
    max(lo, min(v, hi))
}

#[derive(Clone, Copy)]
enum State {
    Idle,
    Running { ticks: u32 },
    Done(u32),
}

fn step(s: State, input: u8) -> State {
    match (s, input) {
        (State::Idle, b's') => State::Running { ticks: 0 },
        (State::Running { ticks }, b't') => State::Running { ticks: ticks + 1 },
        (State::Running { ticks }, b'x') => State::Done(ticks),
        (s, _) => s,
    }
}

fn mmio_like() -> u32 {
    let mut reg: u32 = 0;
    let p = &mut reg as *mut u32;
    unsafe {
        core::ptr::write_volatile(p, 0x1234);
        let v = core::ptr::read_volatile(p as *const u32);
        core::ptr::write_volatile(p, v + 1);
    }
    reg
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    let mut r = Ring::new();
    let mut fulls = 0;
    for v in [10u16, 20, 300, 40, 50] {
        if r.push(v) == Err(RingError::Full) {
            fulls += 1;
        }
    }
    line(fulls); // 1
    line(r.fold()); // 10 + 20 + (1 + 44) + 40 = 115
    line(r.pop().unwrap() as u32); // 10
    r.push(7).unwrap();
    let mut drained = 0;
    while let Ok(v) = r.pop() {
        drained = drained * 3 + v as u32;
    }
    line(drained);
    line(if r.pop().is_err() { 1 } else { 0 }); // 1
    line(sum_tags(&[1u16, 2, 0x0102]));
    line(clamp(15u32, 3, 9) + clamp(1u32, 3, 9) * 10); // 9 + 30
    let mut st = State::Idle;
    for &c in b"qsttttx" {
        st = step(st, c);
    }
    line(match st {
        State::Done(n) => n,
        State::Running { ticks } => 100 + ticks,
        State::Idle => 999,
    }); // 4
    line(mmio_like()); // 0x1235 = 4661
    let mut a = 3u32;
    let mut b = 8u32;
    core::mem::swap(&mut a, &mut b);
    line(a * 10 + b); // 83
    line(core::mem::size_of::<Ring>() as u32 + core::mem::size_of::<State>() as u32);
    let np: *const u8 = core::ptr::null();
    line(if np.is_null() { 1 } else { 0 }); // 1
    line(5u16.tagged()); // 5 ^ 0xA5
    0
}
