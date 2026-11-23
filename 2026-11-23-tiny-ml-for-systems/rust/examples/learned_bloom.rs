//! Learned Bloom filter の Rust 版。f32 モデルと int8 量子化モデルを比較する。
//!   cargo run --release --example learned_bloom
use std::collections::HashSet;
use std::time::Instant;
use tiny_ml::{hashed_ngrams, urls, Bloom, Mlp, QuantMlp, Rng};

const D: usize = 256;
const N_KEYS: usize = 30_000;

fn kb(bits: usize) -> String {
    format!("{:.1} KB", bits as f64 / 8.0 / 1024.0)
}

fn bloom_bits(n: usize, fpr: f64) -> usize {
    if n == 0 { 0 } else { ((-(n as f64) * fpr.ln()) / (std::f64::consts::LN_2.powi(2))).ceil() as usize }
}

/// モデル (score 関数) と鍵集合から、目標 FPR を満たし総ビット数が最小の learned Bloom filter を組む
fn build(
    score: &mut dyn FnMut(&str) -> f32,
    model_bits: usize,
    keys: &[String],
    val_neg: &[String],
    target: f64,
) -> (f32, Bloom) {
    let ks: Vec<f32> = keys.iter().map(|k| score(k)).collect();
    let mut vs: Vec<f32> = val_neg.iter().map(|k| score(k)).collect();
    vs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut best = (usize::MAX, 1.0f32, 0usize, 0.0f64);
    for q in 0..200 {
        let fm_target = target * q as f64 / 200.0;
        let idx = ((vs.len() as f64 * (1.0 - fm_target)) as usize).min(vs.len() - 1);
        let tau = vs[idx] + 1e-7;
        let fm = vs.iter().filter(|&&v| v >= tau).count() as f64 / vs.len() as f64;
        if fm >= target {
            continue;
        }
        let nb = ks.iter().filter(|&&v| v < tau).count();
        let fb = (target - fm) / (1.0 - fm);
        let bits = model_bits + bloom_bits(nb, fb);
        if bits < best.0 {
            best = (bits, tau, nb, fb);
        }
    }
    let (_, tau, nb, fb) = best;
    let mut backup = Bloom::for_capacity(nb, fb);
    for (k, &s) in keys.iter().zip(&ks) {
        if s < tau {
            backup.add(k.as_bytes());
        }
    }
    (tau, backup)
}

fn eval(name: &str, model_bits: usize, score: &mut dyn FnMut(&str) -> f32, keys: &[String], val: &[String], test: &[String], shifted: &[String], target: f64) {
    let (tau, backup) = build(score, model_bits, keys, val, target);
    let mut q = |s: &str| score(s) >= tau || backup.has(s.as_bytes());
    let fn_ = keys.iter().filter(|k| !q(k)).count();
    let t0 = Instant::now();
    let fp = test.iter().filter(|k| q(k)).count();
    let ns = t0.elapsed().as_nanos() as f64 / test.len() as f64;
    let sfp = shifted.iter().filter(|k| q(k)).count();
    println!(
        "{name:<26} total {:>8} (model {} + backup {})  test FPR={:.3}%  shifted FPR={:.1}%  FN={fn_}  {ns:.0} ns/q",
        kb(model_bits + backup.size_bits()), kb(model_bits), kb(backup.size_bits()),
        100.0 * fp as f64 / test.len() as f64, 100.0 * sfp as f64 / shifted.len() as f64,
    );
}

fn main() {
    let mut rng = Rng::new(42);
    let none = HashSet::new();
    let keys = urls::unique(&mut rng, N_KEYS, urls::malicious, &none);
    let key_set: HashSet<String> = keys.iter().cloned().collect();
    let train_neg = urls::unique(&mut rng, 30_000, urls::benign, &key_set);
    let val_neg = urls::unique(&mut rng, 30_000, urls::benign, &key_set);
    let test_neg = urls::unique(&mut rng, 100_000, urls::benign, &key_set);
    let shifted = urls::unique(&mut rng, 20_000, urls::malicious, &key_set);

    let n = keys.len() + train_neg.len();
    let mut x = vec![0f32; n * D];
    for (i, s) in keys.iter().chain(&train_neg).enumerate() {
        hashed_ngrams(s, &mut x[i * D..(i + 1) * D]);
    }
    let y: Vec<f32> = (0..n).map(|i| if i < keys.len() { 1.0 } else { 0.0 }).collect();

    let target = 0.01;
    let mut plain = Bloom::for_capacity(keys.len(), target);
    keys.iter().for_each(|k| plain.add(k.as_bytes()));
    let t0 = Instant::now();
    let fp = test_neg.iter().filter(|k| plain.has(k.as_bytes())).count();
    let ns = t0.elapsed().as_nanos() as f64 / test_neg.len() as f64;
    let sfp = shifted.iter().filter(|k| plain.has(k.as_bytes())).count();
    println!("target FPR 1%, {} keys", keys.len());
    println!(
        "{:<26} total {:>8}  test FPR={:.3}%  shifted FPR={:.1}%  {ns:.0} ns/q",
        "plain Bloom", kb(plain.size_bits()),
        100.0 * fp as f64 / test_neg.len() as f64, 100.0 * sfp as f64 / shifted.len() as f64
    );

    for sizes in [vec![D, 1], vec![D, 8, 1]] {
        let mut model = Mlp::new(&sizes, &mut Rng::new(1));
        let t0 = Instant::now();
        model.fit(&x, &y, 8, 64, 3e-3, &mut Rng::new(2));
        let label = if sizes.len() == 2 { "logreg".to_string() } else { format!("mlp {:?}", sizes) };
        println!("--- {label}: {} params, train {} ms", model.param_count(), t0.elapsed().as_millis());

        let mut feat = vec![0f32; D];
        let bits32 = model.param_count() * 32;
        eval(&format!("{label} f32"), bits32, &mut |s| { hashed_ngrams(s, &mut feat); model.predict(&feat) }, &keys, &val_neg, &test_neg, &shifted, target);

        let mut qm = QuantMlp::from(&model);
        let bits8 = qm.size_bytes() * 8;
        eval(&format!("{label} int8"), bits8, &mut |s| { hashed_ngrams(s, &mut feat); qm.predict(&feat) }, &keys, &val_neg, &test_neg, &shifted, target);
    }
}
