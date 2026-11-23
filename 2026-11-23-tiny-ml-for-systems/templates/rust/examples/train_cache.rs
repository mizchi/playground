//! 雛形の使い方の例: キャッシュ admission のモデルを学習し、評価し、重みとスコアを書き出す。
//!   cargo run --release --example train_cache [-- ../../bench/data]
//! データ・分け方・書き出し先は ../python/train_cache.py と同じ。
use std::time::Instant;
use tinyml_template::{auc, read_f32, write_f32, Mlp, Rng};

fn main() {
    let data = std::env::args().nth(1).unwrap_or(concat!(env!("CARGO_MANIFEST_DIR"), "/../../bench/data").into());
    let (n, d) = (600_000, 11);
    let train = n * 3 / 10;
    let x = read_f32(&format!("{data}/cache_X.f32"));
    let y = read_f32(&format!("{data}/cache_y.f32"));
    let out = concat!(env!("CARGO_MANIFEST_DIR"), "/out");
    std::fs::create_dir_all(out).unwrap();
    std::fs::create_dir_all(format!("{data}/scores")).unwrap();
    for (name, sizes) in [("logreg", vec![d, 1]), ("mlp16", vec![d, 16, 1])] {
        let mut m = Mlp::new(&sizes, &mut Rng::new(1));
        let t0 = Instant::now();
        m.fit(&x[..train * d], &y[..train], 4, 128, 3e-3, &mut Rng::new(2));
        let train_s = t0.elapsed().as_secs_f64();
        let scores: Vec<f32> = (0..n).map(|i| m.predict(&x[i * d..(i + 1) * d])).collect();
        println!(
            "{name:<7} {:>4} params  train {train_s:.2}s  test AUC {:.4}",
            m.param_count(),
            auc(&scores[train..], &y[train..])
        );
        std::fs::write(format!("{out}/{name}.json"), m.to_json()).unwrap();
        write_f32(&format!("{data}/scores/rust-template-{name}.f32"), &scores);
    }
    println!("wrote out/*.json and {data}/scores/rust-template-*.f32");
}
