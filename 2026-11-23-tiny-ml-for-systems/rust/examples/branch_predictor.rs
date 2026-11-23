//! パーセプトロン分岐予測 (Jiménez & Lin, HPCA 2001) の再現。
//! 実際に AMD Zen 系などの CPU に載っている「最も成功した組み込み ML」の一つ。
//!
//! 誤差逆伝搬すら不要な、オンライン学習する 1 層パーセプトロン:
//!   y = w0 + Σ w_i * x_i   (x_i = 過去 i 番目の分岐結果を ±1 にしたもの)
//!   予測 = y >= 0, 外れたか |y| <= θ なら w_i += t * x_i (t = ±1 の正解)
//! 重みは i8 の飽和カウンタ。浮動小数点もフレームワークも要らない。
//!
//!   cargo run --release --example branch_predictor
use tiny_ml::Rng;

// ------------------------------------------------------------------ 予測器
trait Predictor {
    fn name(&self) -> String;
    fn predict(&mut self, pc: u32) -> bool;
    fn update(&mut self, pc: u32, taken: bool);
    fn bytes(&self) -> usize;
}

fn counter_update(c: &mut u8, taken: bool) {
    *c = if taken { (*c + 1).min(3) } else { c.saturating_sub(1) };
}

/// PC ごとの 2bit 飽和カウンタ
struct Bimodal { t: Vec<u8> }
impl Predictor for Bimodal {
    fn name(&self) -> String { "bimodal (2bit counters)".into() }
    fn predict(&mut self, pc: u32) -> bool { self.t[pc as usize % self.t.len()] >= 2 }
    fn update(&mut self, pc: u32, taken: bool) { let n = self.t.len(); counter_update(&mut self.t[pc as usize % n], taken) }
    fn bytes(&self) -> usize { self.t.len() / 4 }
}

/// gshare: (PC xor グローバル履歴) で 2bit カウンタ表を引く。古典的な定番。
struct Gshare { t: Vec<u8>, hist: u64, bits: u32 }
impl Gshare {
    fn idx(&self, pc: u32) -> usize { ((pc as u64 ^ self.hist) & ((1 << self.bits) - 1)) as usize }
}
impl Predictor for Gshare {
    fn name(&self) -> String { format!("gshare ({}bit history)", self.bits) }
    fn predict(&mut self, pc: u32) -> bool { self.t[self.idx(pc)] >= 2 }
    fn update(&mut self, pc: u32, taken: bool) {
        let i = self.idx(pc);
        counter_update(&mut self.t[i], taken);
        self.hist = (self.hist << 1) | taken as u64;
    }
    fn bytes(&self) -> usize { self.t.len() / 4 }
}

/// パーセプトロン予測器
struct Perceptron { w: Vec<i8>, n: usize, h: usize, hist: Vec<i8>, theta: i32, last_y: i32 }
impl Perceptron {
    fn new(n: usize, h: usize) -> Self {
        let theta = (1.93 * h as f64 + 14.0) as i32; // 論文の経験式
        Perceptron { w: vec![0; n * (h + 1)], n, h, hist: vec![1; h], theta, last_y: 0 }
    }
    fn row(&self, pc: u32) -> usize { (pc as usize % self.n) * (self.h + 1) }
}
impl Predictor for Perceptron {
    fn name(&self) -> String { format!("perceptron ({}x{} hist)", self.n, self.h) }
    fn predict(&mut self, pc: u32) -> bool {
        let r = self.row(pc);
        let w = &self.w[r..r + self.h + 1];
        self.last_y = w[0] as i32 + w[1..].iter().zip(&self.hist).map(|(&w, &x)| w as i32 * x as i32).sum::<i32>();
        self.last_y >= 0
    }
    fn update(&mut self, pc: u32, taken: bool) {
        let t: i32 = if taken { 1 } else { -1 };
        if (self.last_y >= 0) != taken || self.last_y.abs() <= self.theta {
            let r = self.row(pc);
            let sat = |w: &mut i8, d: i32| *w = (*w as i32 + d).clamp(-127, 127) as i8;
            sat(&mut self.w[r], t);
            for i in 0..self.h {
                sat(&mut self.w[r + 1 + i], t * self.hist[i] as i32);
            }
        }
        self.hist.rotate_right(1);
        self.hist[0] = t as i8;
    }
    fn bytes(&self) -> usize { self.w.len() }
}

// ------------------------------------------------------------------ 合成プログラム
// 1 イテレーションで実行される静的分岐の列。それぞれ違う「予測しやすさ」を持つ。
#[derive(Clone, Copy, PartialEq)]
enum Kind { InnerLoop, Biased, Random, Random2, CopyNear, XorNear, Filler, CopyFar }
const KINDS: [(Kind, &str); 8] = [
    (Kind::InnerLoop, "inner loop (TTTTTTTN)"),
    (Kind::Biased, "biased 90% taken"),
    (Kind::Random, "random 50% (=R)"),
    (Kind::Random2, "random 50% (=S)"),
    (Kind::CopyNear, "= R (next)"),
    (Kind::XorNear, "= R xor S (next)"),
    (Kind::Filler, "filler (95% taken, x16)"),
    (Kind::CopyFar, "= R (~20 branches later)"),
];

fn run(p: &mut dyn Predictor, iters: usize) -> Vec<(usize, usize)> {
    let mut rng = Rng::new(9);
    let mut stats = vec![(0usize, 0usize); KINDS.len()]; // (miss, total)
    let mut exec = |p: &mut dyn Predictor, kind: Kind, pc: u32, taken: bool, warm: bool| {
        let guess = p.predict(pc);
        p.update(pc, taken);
        if warm {
            let s = &mut stats[KINDS.iter().position(|k| k.0 == kind).unwrap()];
            s.0 += (guess != taken) as usize;
            s.1 += 1;
        }
    };
    for it in 0..iters {
        let warm = it > iters / 10;
        for j in 0..8 {
            exec(p, Kind::InnerLoop, 0x10, j < 7, warm);
        }
        let b = rng.f64() < 0.9;
        exec(p, Kind::Biased, 0x20, b, warm);
        let r = rng.f64() < 0.5;
        exec(p, Kind::Random, 0x30, r, warm);
        let s2 = rng.f64() < 0.5;
        exec(p, Kind::Random2, 0x38, s2, warm);
        exec(p, Kind::CopyNear, 0x40, r, warm);
        exec(p, Kind::XorNear, 0x50, r ^ s2, warm);
        for f in 0..16 {
            exec(p, Kind::Filler, 0x100 + f, rng.f64() < 0.95, warm);
        }
        exec(p, Kind::CopyFar, 0x60, r, warm);
    }
    stats
}

fn main() {
    let iters = 200_000;
    let mut preds: Vec<Box<dyn Predictor>> = vec![
        Box::new(Bimodal { t: vec![1; 4096] }),
        Box::new(Gshare { t: vec![1; 1 << 12], hist: 0, bits: 12 }),
        Box::new(Gshare { t: vec![1; 1 << 16], hist: 0, bits: 16 }),
        Box::new(Perceptron::new(256, 32)),
    ];
    let results: Vec<_> = preds.iter_mut().map(|p| run(p.as_mut(), iters)).collect();

    print!("{:<26}", "misprediction rate");
    for p in &preds {
        print!(" | {:>26}", p.name());
    }
    println!();
    print!("{:<26}", "  (storage)");
    for p in &preds {
        print!(" | {:>26}", format!("{} B", p.bytes()));
    }
    println!();
    for (k, (_, label)) in KINDS.iter().enumerate() {
        print!("{label:<26}");
        for r in &results {
            print!(" | {:>25.1}%", 100.0 * r[k].0 as f64 / r[k].1 as f64);
        }
        println!();
    }
    print!("{:<26}", "TOTAL");
    for r in &results {
        let (m, t) = r.iter().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        print!(" | {:>25.2}%", 100.0 * m as f64 / t as f64);
    }
    println!();
}
