//! 時間の計測 (CRUSTC_PROF=1 の段ごとの時間)。std::time を使わない (crustc 自身でもコンパイルできるように)

unsafe extern "C" {
    #[link_name = "clock_gettime"]
    fn c_clock_gettime(clk: i32, ts: *mut i64) -> i32;
}

/// 単調な時計 (ns)
pub fn now_ns() -> u64 {
    let mut ts = [0i64; 2];
    // SAFETY: CLOCK_MONOTONIC (1)。ts は timespec と同じ並び
    unsafe { c_clock_gettime(1, ts.as_mut_ptr()) };
    ts[0] as u64 * 1_000_000_000 + ts[1] as u64
}

/// 段ごとの時間を出す (on のときだけ)
pub struct Timer {
    pub on: bool,
    pub last: u64,
    pub start: u64,
}

impl Timer {
    pub fn lap(&mut self, what: &str, extra: String) {
        if !self.on {
            return;
        }
        let t = now_ns();
        eprintln!("  {:<10} {:>8.2} ms  {}", what, (t - self.last) as f64 / 1e6, extra);
        self.last = t;
    }
}
