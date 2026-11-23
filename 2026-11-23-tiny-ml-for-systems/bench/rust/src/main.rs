//! Rust 側のベンチ。Python 側 (../python/bench.py) と同じデータ・同じ指標で測る。
//!
//!   cargo run --release -- dump-bloom   # ../data に URL の特徴量を書き出す
//!   cargo run --release -- run          # 学習時間・テスト AUC・推論の速さを測る
//!
//! 指標:
//!   train   学習の壁時計時間
//!   AUC     テストデータの ROC AUC
//!   1-row   1 行ずつ推論したときの 1 行あたりの時間 (システムの判断部品はこの使い方になる)
//!   batch   全テスト行をまとめて推論したときの 1 行あたりの時間
use perpetual::objective::Objective;
use perpetual::{Matrix, PerpetualBooster};
use std::collections::HashSet;
use std::fs;
use std::time::Instant;
use tiny_ml::{hashed_ngrams, urls, Mlp, Rng};

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../data");

fn write_f32(path: &str, xs: &[f32]) {
    fs::write(path, xs.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
}
fn read_f32(path: &str) -> Vec<f32> {
    fs::read(path).unwrap().chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

fn dump_bloom() {
    const D: usize = 256;
    let mut rng = Rng::new(42);
    let none = HashSet::new();
    let keys = urls::unique(&mut rng, 30_000, urls::malicious, &none);
    let key_set: HashSet<String> = keys.iter().cloned().collect();
    let train_neg = urls::unique(&mut rng, 30_000, urls::benign, &key_set);
    let mut seen = key_set.clone();
    seen.extend(train_neg.iter().cloned());
    let test_pos = urls::unique(&mut rng, 20_000, urls::malicious, &seen);
    let test_neg = urls::unique(&mut rng, 20_000, urls::benign, &seen);
    for (name, pos, neg) in [("train", &keys, &train_neg), ("test", &test_pos, &test_neg)] {
        let rows: Vec<&String> = pos.iter().chain(neg.iter()).collect();
        let mut x = vec![0f32; rows.len() * D];
        for (i, s) in rows.iter().enumerate() {
            hashed_ngrams(s, &mut x[i * D..(i + 1) * D]);
        }
        let y: Vec<f32> = (0..rows.len()).map(|i| (i < pos.len()) as u8 as f32).collect();
        write_f32(&format!("{DATA}/bloom_{name}_X.f32"), &x);
        write_f32(&format!("{DATA}/bloom_{name}_y.f32"), &y);
    }
    fs::write(format!("{DATA}/bloom.json"), format!("{{\"d\":{D},\"train\":60000,\"test\":40000}}")).unwrap();
    println!("dumped bloom: train 60000, test 40000, d={D}");
}

struct Split {
    d: usize,
    xtr: Vec<f32>,
    ytr: Vec<f32>,
    xte: Vec<f32>,
    yte: Vec<f32>,
}

fn load_bloom() -> Split {
    Split {
        d: 256,
        xtr: read_f32(&format!("{DATA}/bloom_train_X.f32")),
        ytr: read_f32(&format!("{DATA}/bloom_train_y.f32")),
        xte: read_f32(&format!("{DATA}/bloom_test_X.f32")),
        yte: read_f32(&format!("{DATA}/bloom_test_y.f32")),
    }
}

/// 時系列で分ける: 先頭 30% で学習、残りで評価 (TS の実験と同じ)
fn load_cache() -> Split {
    let (d, n) = (11, 600_000);
    let train = n * 3 / 10;
    let x = read_f32(&format!("{DATA}/cache_X.f32"));
    let y = read_f32(&format!("{DATA}/cache_y.f32"));
    Split { d, xtr: x[..train * d].to_vec(), ytr: y[..train].to_vec(), xte: x[train * d..].to_vec(), yte: y[train..].to_vec() }
}

fn auc(scores: &[f64], labels: &[f32]) -> f64 {
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    idx.sort_by(|&a, &b| scores[a].partial_cmp(&scores[b]).unwrap());
    let (mut pos, mut neg, mut rank_sum) = (0f64, 0f64, 0f64);
    for (r, &i) in idx.iter().enumerate() {
        if labels[i] > 0.5 {
            pos += 1.0;
            rank_sum += (r + 1) as f64;
        } else {
            neg += 1.0;
        }
    }
    (rank_sum - pos * (pos + 1.0) / 2.0) / (pos * neg)
}

fn report(task: &str, name: &str, train_s: f64, auc: f64, one_row_ns: f64, batch_ns: f64) {
    println!("{task:<6} {name:<28} train {train_s:>7.2}s  AUC {auc:.4}  1-row {:>9}  batch {:>9}", fmt_ns(one_row_ns), fmt_ns(batch_ns));
}
fn fmt_ns(ns: f64) -> String {
    if ns >= 1000.0 { format!("{:.1}µs", ns / 1000.0) } else { format!("{ns:.0}ns") }
}

fn bench_mlp(task: &str, s: &Split, sizes: &[usize], epochs: usize, batch: usize) {
    let d = s.d;
    let mut m = Mlp::new(sizes, &mut Rng::new(1));
    let t0 = Instant::now();
    m.fit(&s.xtr, &s.ytr, epochs, batch, 3e-3, &mut Rng::new(2));
    let train_s = t0.elapsed().as_secs_f64();
    let n = s.yte.len();
    let t0 = Instant::now();
    let scores: Vec<f64> = (0..n).map(|i| m.predict(&s.xte[i * d..(i + 1) * d]) as f64).collect();
    let per_row = t0.elapsed().as_nanos() as f64 / n as f64;
    let name = if sizes.len() == 2 { "own logreg (1 thread)".to_string() } else { format!("own MLP {:?} (1 thread)", &sizes[1..sizes.len() - 1]) };
    // 自前の実装は 1 行ずつしか推論しないので、1-row と batch は同じ値になる
    report(task, &name, train_s, auc(&scores, &s.yte), per_row, per_row);
}

fn col_major(x: &[f32], rows: usize, d: usize) -> Vec<f64> {
    let mut out = vec![0f64; rows * d];
    for r in 0..rows {
        for c in 0..d {
            out[c * rows + r] = x[r * d + c] as f64;
        }
    }
    out
}

fn bench_perpetual(task: &str, s: &Split, budget: f32) {
    let d = s.d;
    let (ntr, nte) = (s.ytr.len(), s.yte.len());
    let xtr = col_major(&s.xtr, ntr, d);
    let ytr: Vec<f64> = s.ytr.iter().map(|&v| v as f64).collect();
    let mut model = PerpetualBooster::default().set_objective(Objective::LogLoss).set_budget(budget);
    let t0 = Instant::now();
    model.fit(&Matrix::new(&xtr, ntr, d), &ytr, None, None).unwrap();
    let train_s = t0.elapsed().as_secs_f64();
    let xte = col_major(&s.xte, nte, d);
    let t0 = Instant::now();
    let scores = model.predict_proba(&Matrix::new(&xte, nte, d), true, false);
    let batch_ns = t0.elapsed().as_nanos() as f64 / nte as f64;
    let k = nte.min(20_000);
    let t0 = Instant::now();
    let mut sink = 0.0;
    for i in 0..k {
        let row: Vec<f64> = s.xte[i * d..(i + 1) * d].iter().map(|&v| v as f64).collect();
        sink += model.predict_proba(&Matrix::new(&row, 1, d), false, false)[0];
    }
    let one_row_ns = t0.elapsed().as_nanos() as f64 / k as f64;
    std::hint::black_box(sink);
    let trees = model.get_prediction_trees().len();
    report(task, &format!("perpetual budget={budget} ({trees} trees)"), train_s, auc(&scores, &s.yte), one_row_ns, batch_ns);
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("dump-bloom") => dump_bloom(),
        Some("run-own") => {
            bench_mlp("url", &load_bloom(), &[256, 1], 8, 64);
            bench_mlp("url", &load_bloom(), &[256, 8, 1], 8, 64);
            bench_mlp("cache", &load_cache(), &[11, 1], 4, 128);
            bench_mlp("cache", &load_cache(), &[11, 16, 1], 4, 128);
        }
        Some("run") => {
            println!("threads: own = 1, perpetual = rayon default ({} cores)", std::thread::available_parallelism().unwrap());
            let b = load_bloom();
            bench_mlp("url", &b, &[256, 1], 8, 64);
            bench_mlp("url", &b, &[256, 8, 1], 8, 64);
            bench_perpetual("url", &b, 0.5);
            let c = load_cache();
            bench_mlp("cache", &c, &[11, 16, 1], 4, 128);
            bench_perpetual("cache", &c, 0.5);
            bench_perpetual("cache", &c, 1.0);
        }
        _ => eprintln!("usage: tiny-ml-bench dump-bloom | run"),
    }
}
