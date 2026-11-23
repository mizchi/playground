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
use std::collections::{BTreeMap, HashMap, HashSet};
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

// ------------------------------------------------------------------ キャッシュのヒット率
// 各手段は、全 60 万リクエストについて「入れるべきか」のスコアを data/scores/<名前>.f32 に書く。
// 特徴量はキャッシュの中身に依存しない (参照回数・前回からの間隔・種別・サイズ) ので、
// スコアを先に作っておき、1 つのシミュレータで全手段を同じ条件で評価できる。

fn read_u32(path: &str) -> Vec<u32> {
    fs::read(path).unwrap().chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

/// キャッシュ実験 (先頭 30% で学習) のモデルで、全行のスコアを書く
fn write_scores() {
    let d = 11;
    let n = 600_000;
    let train = n * 3 / 10;
    let x = read_f32(&format!("{DATA}/cache_X.f32"));
    let y = read_f32(&format!("{DATA}/cache_y.f32"));
    let (xtr, ytr) = (&x[..train * d], &y[..train]);
    for (name, sizes) in [("rust-own-logreg", vec![11, 1]), ("rust-own-mlp16", vec![11, 16, 1])] {
        let mut m = Mlp::new(&sizes, &mut Rng::new(1));
        m.fit(xtr, ytr, 4, 128, 3e-3, &mut Rng::new(2));
        let s: Vec<f32> = (0..n).map(|i| m.predict(&x[i * d..(i + 1) * d])).collect();
        write_f32(&format!("{DATA}/scores/{name}.f32"), &s);
        println!("wrote scores/{name}.f32");
    }
    let xcol = col_major(xtr, train, d);
    let y64: Vec<f64> = ytr.iter().map(|&v| v as f64).collect();
    let xall = col_major(&x, n, d);
    for budget in [0.5f32, 1.0] {
        let mut m = PerpetualBooster::default().set_objective(Objective::LogLoss).set_budget(budget);
        m.fit(&Matrix::new(&xcol, train, d), &y64, None, None).unwrap();
        let s: Vec<f32> = m.predict_proba(&Matrix::new(&xall, n, d), true, false).iter().map(|&v| v as f32).collect();
        let name = format!("rust-perpetual-b{budget}");
        write_f32(&format!("{DATA}/scores/{name}.f32"), &s);
        println!("wrote scores/{name}.f32");
    }
}

/// LRU + admission。TS の LRUCache (Map の挿入順) と同じ動き
struct Lru {
    cap: u64,
    used: u64,
    map: HashMap<u32, (u32, u64)>, // id -> (size, 最後に使った時刻)
    order: BTreeMap<u64, u32>,     // 時刻 -> id (古い順)
    clock: u64,
}
impl Lru {
    fn new(cap: u64) -> Self {
        Lru { cap, used: 0, map: HashMap::new(), order: BTreeMap::new(), clock: 0 }
    }
    fn get(&mut self, id: u32) -> bool {
        self.clock += 1;
        let Some(e) = self.map.get_mut(&id) else { return false };
        self.order.remove(&e.1);
        e.1 = self.clock;
        self.order.insert(self.clock, id);
        true
    }
    fn put(&mut self, id: u32, size: u32) {
        if size as u64 > self.cap {
            return;
        }
        while self.used + size as u64 > self.cap {
            let (&t, &vid) = self.order.iter().next().unwrap();
            self.order.remove(&t);
            self.used -= self.map.remove(&vid).unwrap().0 as u64;
        }
        self.clock += 1;
        self.map.insert(id, (size, self.clock));
        self.order.insert(self.clock, id);
        self.used += size as u64;
    }
}

struct Hit {
    ohr: f64,
    bhr: f64,
}

/// 先頭 30% は暖機。評価は残り 70% (TS の実験と同じ)
fn measure(ids: &[u32], sizes: &[u32], step: impl FnMut(usize) -> bool) -> Hit {
    measure_range(ids, sizes, ids.len() * 3 / 10, ids.len(), step)
}

/// [0, end) を流し、[start, end) のヒット率を測る
fn measure_range(_ids: &[u32], sizes: &[u32], start: usize, end: usize, mut step: impl FnMut(usize) -> bool) -> Hit {
    let (mut hits, mut reqs, mut hb, mut b) = (0u64, 0u64, 0u64, 0u64);
    for t in 0..end {
        let hit = step(t);
        if t >= start {
            reqs += 1;
            b += sizes[t] as u64;
            if hit {
                hits += 1;
                hb += sizes[t] as u64;
            }
        }
    }
    Hit { ohr: hits as f64 / reqs as f64, bhr: hb as f64 / b as f64 }
}

fn lru_with(ids: &[u32], sizes: &[u32], cap: u64, admit: impl Fn(usize) -> bool) -> Hit {
    lru_range(ids, sizes, cap, ids.len() * 3 / 10, ids.len(), admit)
}

fn lru_range(ids: &[u32], sizes: &[u32], cap: u64, start: usize, end: usize, admit: impl Fn(usize) -> bool) -> Hit {
    let mut c = Lru::new(cap);
    measure_range(ids, sizes, start, end, |t| {
        let hit = c.get(ids[t]);
        if !hit && admit(t) {
            c.put(ids[t], sizes[t]);
        }
        hit
    })
}

#[derive(Clone)]
struct SizeWeighter;
impl quick_cache::Weighter<u32, u32> for SizeWeighter {
    fn weight(&self, _k: &u32, v: &u32) -> u64 {
        *v as u64
    }
}

fn simulate() {
    let ids = read_u32(&format!("{DATA}/cache_id.u32"));
    let sizes = read_u32(&format!("{DATA}/cache_size.u32"));
    let meta = fs::read_to_string(format!("{DATA}/cache.json")).unwrap();
    let cap: u64 = meta.split("\"capacity\":").nth(1).unwrap().trim_end_matches('}').parse().unwrap();
    let x = read_f32(&format!("{DATA}/cache_X.f32"));
    let y = read_f32(&format!("{DATA}/cache_y.f32"));
    let mut rows: Vec<(String, Hit)> = vec![];

    rows.push(("LRU (admit all)".into(), lru_with(&ids, &sizes, cap, |_| true)));
    // 特徴量 9 列目 = 過去に見たことがあるか
    rows.push(("LRU + doorkeeper (2nd hit)".into(), lru_with(&ids, &sizes, cap, |t| x[t * 11 + 9] > 0.5)));

    let moka: moka::sync::Cache<u32, u32> =
        moka::sync::Cache::builder().max_capacity(cap).weigher(|_k, v: &u32| *v).build();
    rows.push(("moka 0.12 (TinyLFU)".into(), measure(&ids, &sizes, |t| {
        let hit = moka.get(&ids[t]).is_some();
        if !hit {
            moka.insert(ids[t], sizes[t]);
        }
        moka.run_pending_tasks();
        hit
    })));

    let mut qc = quick_cache::unsync::Cache::with_weighter(100_000, cap, SizeWeighter);
    rows.push(("quick_cache 0.7 (S3-FIFO)".into(), measure(&ids, &sizes, |t| {
        let hit = qc.get(&ids[t]).is_some();
        if !hit {
            qc.insert(ids[t], sizes[t]);
        }
        hit
    })));

    let mut files: Vec<_> = fs::read_dir(format!("{DATA}/scores")).unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    for f in files {
        let s = read_f32(f.to_str().unwrap());
        let name = f.file_stem().unwrap().to_string_lossy().to_string();
        rows.push((format!("LRU + {name} p>0.5"), lru_with(&ids, &sizes, cap, |t| s[t] > 0.5)));
        // 閾値を学習区間 (先頭 30%) の後半で選び直す。評価区間は見ない
        let train = ids.len() * 3 / 10;
        let best = (1..=9)
            .map(|k| k as f32 / 10.0)
            .max_by(|&a, &b| {
                let ha = lru_range(&ids, &sizes, cap, train / 3, train, |t| s[t] > a).ohr;
                let hb = lru_range(&ids, &sizes, cap, train / 3, train, |t| s[t] > b).ohr;
                ha.partial_cmp(&hb).unwrap()
            })
            .unwrap();
        rows.push((format!("LRU + {name} p>{best} (tuned)"), lru_with(&ids, &sizes, cap, |t| s[t] > best)));
    }
    rows.push(("LRU + oracle (future)".into(), lru_with(&ids, &sizes, cap, |t| y[t] > 0.5)));

    println!("cache {} MB, eval on last 70% of {} requests", cap / 1_000_000, ids.len());
    println!("{:<40} {:>7} {:>7}", "policy", "OHR", "BHR");
    for (name, h) in rows {
        println!("{name:<40} {:>6.2}% {:>6.2}%", h.ohr * 100.0, h.bhr * 100.0);
    }
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("dump-bloom") => dump_bloom(),
        Some("scores") => write_scores(),
        Some("sim") => simulate(),
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
