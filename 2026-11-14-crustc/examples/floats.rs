// 浮動小数点のサンプル: f64 / f32 の四則演算・剰余・比較・キャスト (飽和)、Display / Debug (いちばん短い表現・
// 指数・-0・NaN・inf)、{:.N} / 幅 / 0 埋め / +、メソッド (sqrt / powi / powf / 三角関数 / floor など)、定数、parse、
// sort_by(partial_cmp) / total_cmp、構造体のフィールド・Vec の合計
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
struct P {
    x: f64,
    y: f64,
}

fn dist(a: P, b: P) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

fn main() {
    let vals = [0.0, -0.0, 1.0, 0.1, 0.1 + 0.2, 1.5, -2.25, 100.0, 1e15, 1e16, 1.5e17, 1e21, 1e-4, 1e-5, 3.14159, 2.0 / 3.0, 123456789.125, f64::MAX, f64::MIN_POSITIVE];
    for v in vals {
        println!("{} | {:?}", v, v);
    }
    let fs = [0.1f32, 1.0 / 3.0, 16777216.0, 1e-7, 2.5];
    for v in fs {
        println!("{} | {:?}", v, v);
    }
    let nan = f64::NAN;
    let inf = f64::INFINITY;
    println!("{} {:?} {} {} {}", nan, nan, inf, -inf, nan == nan);
    println!("[{:.2}] [{:.0}] [{:.3}] [{:8.2}] [{:<8.1}] [{:08.3}] [{:+.1}] [{:.1}]", PI, 2.5, -0.0005, PI, 1.25, -3.14159, 2.0, 0.05);
    // 演算・比較・キャスト
    let a = 7.5f64;
    let b = 2.0;
    println!("{} {} {} {} {} {}", a + b, a - b, a * b, a / b, a % b, -a % b);
    println!("{} {} {} {}", a > b, a <= b, (0.1 + 0.2) == 0.3, (0.1f64 + 0.2 - 0.3).abs() < f64::EPSILON);
    println!("{} {} {} {} {} {}", 3.99 as i32, -3.99 as i32, 300.7 as u8, -5.0 as u8, f64::NAN as i32, 1e20 as i64);
    println!("{} {} {} {}", 7 as f64 / 2.0, (2u64.pow(53) + 1) as f64, 1.0f32 as f64, 0.1f64 as f32);
    // メソッド
    let x = 2.0f64;
    println!("{} {} {} {} {}", x.sqrt(), x.powi(10), x.powf(0.5), x.exp(), x.ln());
    println!("{:.6} {:.6} {:.6} {:.6}", (PI / 6.0).sin(), (PI / 3.0).cos(), 1.0f64.atan2(1.0), 100f64.log10());
    println!("{} {} {} {} {} {}", 2.5f64.floor(), 2.5f64.ceil(), 2.5f64.round(), (-2.5f64).round(), 2.7f64.trunc(), 2.75f64.fract());
    println!("{} {} {} {} {}", 3.0f64.max(f64::NAN), f64::NAN.min(1.0), (-4.5f64).abs(), (-0.0f64).signum(), 5.5f64.clamp(0.0, 3.0));
    println!("{} {} {} {}", nan.is_nan(), inf.is_infinite(), 1.0f64.is_finite(), (-1.0f64).is_sign_negative());
    println!("{} {} {:.4}", 1.0f64.to_bits(), 0.5f32.to_bits(), 90.0f64.to_radians());
    // parse
    let ps = ["3.25", "-1e3", "inf", "NaN", "  1.0", "1.", ".5", "abc", ""];
    for p in ps {
        match p.parse::<f64>() {
            Ok(v) => print!("{:?} ", v),
            Err(e) => print!("[{}] ", e),
        }
    }
    println!();
    // sort / 合計 / 構造体
    let mut v = vec![3.5, -1.0, 2.25, 0.0, 10.0];
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let total: f64 = v.iter().sum();
    let mean = total / v.len() as f64;
    let mx = v.iter().cloned().fold(f64::MIN, f64::max);
    println!("{:?} {} {} {}", v, total, mean, mx);
    let mut w = vec![2.5f64, -0.0, 0.0, -7.0];
    w.sort_by(|a, b| a.total_cmp(b));
    println!("{:?}", w);
    let p = P { x: 0.0, y: 0.0 };
    let q = P { x: 3.0, y: 4.0 };
    println!("{:?} {} {}", q, dist(p, q), p == P { x: -0.0, y: 0.0 });
    let avg: f32 = [1.0f32, 2.0, 4.0].iter().sum::<f32>() / 3.0;
    println!("{} {:.3}", avg, avg);
}
