//! 微分可能プログラミングの雛形 (Rust std::autodiff 版)。../../bench/autodiff のコピー。
//! ../python/ttl_jax.py・../julia/diff/ttl_enzyme.jl と同じ問題を、Rust の std::autodiff (Enzyme) で解く。
//!
//! 損失は普通の Rust のループで書き、#[autodiff_reverse] が逆伝搬のコードを生成する。
//! 手で書いた逆伝搬 (../../rust/src/lib.rs の accumulate) も、勾配チェックも要らない。
//!
//!   RUSTFLAGS="-Zautodiff=Enable" cargo +nightly run --release -- bhr   (または ohr)
#![feature(autodiff)]
use std::autodiff::autodiff_reverse;
use std::fs;
use std::time::Instant;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../bench/data");
const D: usize = 11;
const H: usize = 16;
const NP: usize = H * D + H + H + 1; // W1, b1, w2, b2
const TAU: f64 = 0.25;
const MU: f64 = 10.0;
const LOG_INF: f64 = 30.0;

/// TTL の対数。p = [W1 (H×D) | b1 (H) | w2 (H) | b2]
fn log_ttl(p: &[f64], x: &[f32]) -> f64 {
    let mut out = p[NP - 1];
    for j in 0..H {
        let mut z = p[H * D + j];
        for k in 0..D {
            z += p[j * D + k] * x[k] as f64;
        }
        if z > 0.0 {
            out += p[H * D + H + j] * z;
        }
    }
    out
}

/// (soft なヒット, 平均占有量)。jax の terms() と同じ式
fn terms(p: &[f64], x: &[f32], lg: &[f64], g: &[f64], z: &[f64], w: &[f64]) -> (f64, f64) {
    let n = lg.len();
    let (mut hit, mut occ) = (0.0, 0.0);
    for i in 0..n {
        let lt = log_ttl(p, &x[i * D..(i + 1) * D]);
        hit += w[i] / (1.0 + (-(lt - lg[i]) / TAU).exp());
        occ += z[i] * g[i].min(lt.exp());
    }
    (hit, occ / n as f64)
}

/// 最小化する損失。容量を超えた分だけに二乗の罰則をかける
#[autodiff_reverse(d_loss, Duplicated, Const, Const, Const, Const, Const, Const, Active)]
fn loss(p: &[f64], x: &[f32], lg: &[f64], g: &[f64], z: &[f64], w: &[f64], cap: f64) -> f64 {
    let (hit, occ) = terms(p, x, lg, g, z, w);
    let over = (occ / cap - 1.0).max(0.0);
    -hit + MU * over * over
}

fn read_f32(path: &str) -> Vec<f32> {
    fs::read(path).unwrap().chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}
fn read_u32(path: &str) -> Vec<u32> {
    fs::read(path).unwrap().chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}
fn field(meta: &str, key: &str) -> f64 {
    meta.split(&format!("\"{key}\":")).nth(1).unwrap().split([',', '}']).next().unwrap().parse().unwrap()
}

fn main() {
    let target = std::env::args().nth(1).unwrap_or("bhr".into());
    let meta = fs::read_to_string(format!("{DATA}/cache.json")).unwrap();
    let (n, tr, cap, h0) = (field(&meta, "n") as usize, field(&meta, "train") as usize, field(&meta, "capacity"), field(&meta, "H"));
    let x = read_f32(&format!("{DATA}/cache_X.f32"));
    let ids = read_u32(&format!("{DATA}/cache_id.u32"));
    let size: Vec<f64> = read_u32(&format!("{DATA}/cache_size.u32")).iter().map(|&v| v as f64).collect();

    // 次に同じ id が来るまでの間隔。学習区間の外に出るものは ∞
    let mut gap = vec![f64::INFINITY; n];
    let mut last = std::collections::HashMap::new();
    for t in (0..n).rev() {
        if let Some(&nx) = last.get(&ids[t]) {
            if nx < tr {
                gap[t] = (nx - t) as f64;
            }
        }
        last.insert(ids[t], t);
    }
    let g = &gap[..tr];
    let lg: Vec<f64> = g.iter().map(|&v| if v.is_infinite() { LOG_INF } else { v.ln() }).collect();
    let z = &size[..tr];
    let zsum: f64 = z.iter().sum();
    let w: Vec<f64> = if target == "bhr" { z.iter().map(|v| v / zsum).collect() } else { vec![1.0 / tr as f64; tr] };
    let xtr = &x[..tr * D];

    // jax 版と同じ決定的な初期値
    let mut p = vec![0.0; NP];
    for i in 0..H {
        for j in 0..D {
            p[i * D + j] = (1.0 + 0.7 * i as f64 + 0.3 * j as f64).sin() * 0.3;
        }
        p[H * D + i] = 0.1;
    }
    p[NP - 1] = h0.ln();

    let (lr, steps) = (0.01, 500);
    let (mut m, mut v) = (vec![0.0; NP], vec![0.0; NP]);
    let t0 = Instant::now();
    for k in 1..=steps {
        let mut dp = vec![0.0; NP];
        let val = d_loss(&p, &mut dp, xtr, &lg, g, z, &w, cap, 1.0);
        let (c1, c2) = (1.0 - 0.9f64.powi(k), 1.0 - 0.999f64.powi(k));
        for i in 0..NP {
            m[i] = 0.9 * m[i] + 0.1 * dp[i];
            v[i] = 0.999 * v[i] + 0.001 * dp[i] * dp[i];
            p[i] -= lr * (m[i] / c1) / ((v[i] / c2).sqrt() + 1e-8);
        }
        if [1, 100, 250, 500].contains(&k) {
            let (hit, occ) = terms(&p, xtr, &lg, g, z, &w);
            println!("step {k:>3}  loss {val:.4}  soft {target} {hit:.4}  occupancy/capacity {:.3}", occ / cap);
        }
    }
    let train_s = t0.elapsed().as_secs_f64();

    // バイアスをずらして、学習区間の平均占有量をちょうど容量に合わせる
    let (mut lo, mut hi) = (-20.0, 20.0);
    let b2 = p[NP - 1];
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        p[NP - 1] = b2 + mid;
        let occ = terms(&p, xtr, &lg, g, z, &w).1;
        if occ < cap { lo = mid } else { hi = mid }
    }
    p[NP - 1] = b2 + lo;
    let (hit, occ) = terms(&p, xtr, &lg, g, z, &w);
    println!("calibrated: soft {target} {hit:.4}  occupancy/capacity {:.3}", occ / cap);
    println!("train {train_s:.2}s ({steps} steps, full batch {tr} rows, 1 thread)");

    // 重みを共通の JSON 形式で書く (TS / Rust の MLP.fromJSON で読み、exp(predictLogit(x)) が TTL)
    let nums = |v: &[f64]| v.iter().map(|x| format!("{x:e}")).collect::<Vec<_>>().join(",");
    let out = concat!(env!("CARGO_MANIFEST_DIR"), "/out");
    fs::create_dir_all(out).unwrap();
    fs::write(
        format!("{out}/ttl-autodiff-{target}.json"),
        format!(
            "{{\"sizes\":[{D},{H},1],\"W\":[[{}],[{}]],\"b\":[[{}],[{}]]}}",
            nums(&p[..H * D]),
            nums(&p[H * D + H..H * D + 2 * H]),
            nums(&p[H * D..H * D + H]),
            nums(&p[NP - 1..])
        ),
    )
    .unwrap();

    let ttl: Vec<u8> = (0..n).flat_map(|i| (log_ttl(&p, &x[i * D..(i + 1) * D]).exp() as f32).to_le_bytes()).collect();
    fs::create_dir_all(format!("{DATA}/ttl")).unwrap();
    fs::write(format!("{DATA}/ttl/rust-template-autodiff-{target}.f32"), ttl).unwrap();
    println!("wrote ttl/rust-template-autodiff-{target}.f32");
}
