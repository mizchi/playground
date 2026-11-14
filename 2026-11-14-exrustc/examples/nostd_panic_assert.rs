// no_std: panic すると #[panic_handler] が呼ばれる (assert)
#![no_std]
#![no_main]

use core::panic::PanicInfo;

unsafe extern "C" {
    fn putchar(c: i32) -> i32;
    fn exit(code: i32) -> !;
}

fn puts(s: &[u8]) {
    for &b in s {
        unsafe {
            putchar(b as i32);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    puts(b"panicked\n");
    unsafe { exit(42) }
}

fn pick(xs: &[u32], i: usize) -> u32 {
    xs[i]
}

fn find(xs: &[u32], want: u32) -> Option<usize> {
    for (i, &x) in xs.iter().enumerate() {
        if x == want {
            return Some(i);
        }
    }
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    let xs = [3u32, 1, 4, 1, 5];
    let mut n = 0;
    for &x in &xs {
        n += x as usize;
    }
    puts(b"start\n");
    let _ = pick(&xs, 2);
    let _ = find(&xs, 4).unwrap();
    puts(b"ok so far\n");
    assert!(n > 10);
    assert_eq!(n, 15);
    puts(b"unreachable\n");
    0
}
