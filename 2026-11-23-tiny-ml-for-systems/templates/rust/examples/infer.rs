//! 製品の中での使い方: どの言語の雛形が書いた重み (JSON) でも読み込み、1 行ずつ推論する。
//!   cargo run --release --example infer -- <weights.json> [学習側が書いたスコア.f32]
//! スコアを渡すと、学習した言語での予測と、ここ (Rust) での予測の最大の差を出す。
use std::time::Instant;
use tinyml_template::{read_f32, Mlp};

fn main() {
    let mut args = std::env::args().skip(1);
    let weights = args.next().expect("usage: infer <weights.json> [scores.f32]");
    let mut model = Mlp::from_json(&std::fs::read_to_string(&weights).unwrap()).unwrap();
    let d = model.sizes[0];
    let x = read_f32(concat!(env!("CARGO_MANIFEST_DIR"), "/../../bench/data/cache_X.f32"));
    let rows = 100_000;
    let t0 = Instant::now();
    // 製品のコードでは、この predict(...) > 0.5 が「判断」の 1 行になる
    let ours: Vec<f32> = (0..rows).map(|i| model.predict(&x[i * d..(i + 1) * d])).collect();
    let ns = t0.elapsed().as_nanos() as f64 / rows as f64;
    let admitted = ours.iter().filter(|&&p| p > 0.5).count();
    let name: Vec<&str> = weights.rsplit('/').take(3).collect();
    let mut line = format!(
        "rust infer {}: {rows} rows, {ns:.0} ns/row, admit {:.1}%",
        name.iter().rev().cloned().collect::<Vec<_>>().join("/"),
        100.0 * admitted as f64 / rows as f64
    );
    if let Some(path) = args.next() {
        let theirs = read_f32(&path);
        let max = ours.iter().zip(&theirs).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        line += &format!(", max |diff| vs trainer = {max:.1e}");
    }
    println!("{line}");
}
