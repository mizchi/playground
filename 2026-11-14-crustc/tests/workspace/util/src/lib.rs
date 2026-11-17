pub static mut CALLS: u32 = 0;
pub fn helper(x: i64) -> i64 { unsafe { CALLS += 1; } x * 2 }
pub fn pick<T: Copy>(v: &[T], i: usize) -> T { v[i] }
pub fn calls() -> u32 { unsafe { CALLS } }
