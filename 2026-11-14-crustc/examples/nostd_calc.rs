// no_std: バイト列の字句解析と、固定長スタックで後置記法の式を評価する電卓
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

fn print_i64(n: i64) {
    if n < 0 {
        put(b'-');
    }
    let mut v = if n < 0 { (0 - n) as u64 } else { n as u64 };
    let mut buf = [0u8; 20];
    let mut i = 20;
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for &b in &buf[i..] {
        put(b);
    }
}

fn puts(s: &str) {
    for &b in s.as_bytes() {
        put(b);
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tok {
    Num(i64),
    Op(u8),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum CalcError {
    BadChar(u8),
    Underflow,
    Overflow,
    DivZero,
    Leftover(usize),
}

const CAP: usize = 8;

struct Stack<T: Copy> {
    items: [Option<T>; CAP],
    len: usize,
}

impl<T: Copy> Stack<T> {
    fn new() -> Self {
        Stack { items: [None; CAP], len: 0 }
    }
    fn push(&mut self, v: T) -> Result<(), CalcError> {
        if self.len == CAP {
            return Err(CalcError::Overflow);
        }
        self.items[self.len] = Some(v);
        self.len += 1;
        Ok(())
    }
    fn pop(&mut self) -> Result<T, CalcError> {
        if self.len == 0 {
            return Err(CalcError::Underflow);
        }
        self.len -= 1;
        match self.items[self.len].take() {
            Some(v) => Ok(v),
            None => Err(CalcError::Underflow),
        }
    }
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn next_tok(&mut self) -> Option<Result<Tok, CalcError>> {
        while self.pos < self.src.len() && self.src[self.pos] == b' ' {
            self.pos += 1;
        }
        if self.pos >= self.src.len() {
            return None;
        }
        let c = self.src[self.pos];
        if c.is_ascii_digit() {
            let mut v: i64 = 0;
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() {
                v = v * 10 + (self.src[self.pos] - b'0') as i64;
                self.pos += 1;
            }
            return Some(Ok(Tok::Num(v)));
        }
        self.pos += 1;
        match c {
            b'+' | b'-' | b'*' | b'/' => Some(Ok(Tok::Op(c))),
            _ => Some(Err(CalcError::BadChar(c))),
        }
    }
}

fn apply(op: u8, a: i64, b: i64) -> Result<i64, CalcError> {
    Ok(match op {
        b'+' => a + b,
        b'-' => a - b,
        b'*' => a * b,
        _ => {
            if b == 0 {
                return Err(CalcError::DivZero);
            }
            a / b
        }
    })
}

fn eval(src: &str) -> Result<i64, CalcError> {
    let mut lx = Lexer { src: src.as_bytes(), pos: 0 };
    let mut st: Stack<i64> = Stack::new();
    while let Some(t) = lx.next_tok() {
        match t? {
            Tok::Num(n) => st.push(n)?,
            Tok::Op(op) => {
                let b = st.pop()?;
                let a = st.pop()?;
                st.push(apply(op, a, b)?)?;
            }
        }
    }
    let r = st.pop()?;
    if st.len != 0 {
        return Err(CalcError::Leftover(st.len));
    }
    Ok(r)
}

const CASES: [&str; 7] = ["3 4 +", "2 3 4 * +", "10 2 8 * + 3 -", "1 +", "4 0 /", "7 x", "1 2 3"];

fn code(e: CalcError) -> i64 {
    match e {
        CalcError::BadChar(c) => 1000 + c as i64,
        CalcError::Underflow => -1,
        CalcError::Overflow => -2,
        CalcError::DivZero => -3,
        CalcError::Leftover(n) => -10 - n as i64,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    let mut ok = 0;
    for (i, src) in CASES.iter().enumerate() {
        print_i64(i as i64);
        puts(": ");
        match eval(src) {
            Ok(v) => {
                ok += 1;
                print_i64(v);
            }
            Err(e) => {
                puts("err ");
                print_i64(code(e));
            }
        }
        put(b'\n');
    }
    ok
}
