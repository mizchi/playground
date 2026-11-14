//! サイズ予算テスト: バイナリ・キャッシュ・LSP のメモリ上キャッシュが予算 (bench/budget.txt) を超えたら失敗する。
//! 意図して増やしたときは `UPDATE_BUDGET=1 cargo test --release --test budget` で予算を書き直す
//! (現在値 + 5% の余裕で書かれる)。

use std::collections::BTreeMap;
use std::path::PathBuf;

use exrustc::{Options, Session, cache_path, testgen::program};

const BUDGET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/bench/budget.txt");

fn measure() -> BTreeMap<String, u64> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("budget");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut m = BTreeMap::new();
    let mut cases: Vec<(String, String)> = vec![("gen1000".into(), program(1000, 0))];
    for e in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/examples")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            cases.push((name, std::fs::read_to_string(&p).unwrap()));
        }
    }
    for (name, src) in &cases {
        let out = dir.join(name);
        // リンカで大きさが変わるので bfd ではなく既定 (自動選択) を使う。CI では EXRUSTC_BENCH_LINKER で固定
        let o = Options { linker: std::env::var("EXRUSTC_BENCH_LINKER").ok(), ..Options::default() };
        let mut s = Session::new(o);
        let r = s.build(src, &out).unwrap_or_else(|e| panic!("{name}: {e}"));
        let cache = s.save_cache(&cache_path(&out)).unwrap();
        m.insert(format!("{name}.binary_bytes"), r.binary_bytes);
        m.insert(format!("{name}.object_bytes"), r.object_bytes as u64);
        m.insert(format!("{name}.cache_bytes"), cache as u64);
    }
    // LSP: 1000 関数を開いた後にメモリ上に残るキャッシュの量
    let mut s = Session::new(Options::default());
    s.check(&program(1000, 0));
    m.insert("lsp1000.mem_bytes".into(), s.mem_usage().1 as u64);
    m
}

#[test]
fn sizes_within_budget() {
    let now = measure();
    if std::env::var_os("UPDATE_BUDGET").is_some() {
        let body: String = now.iter().map(|(k, v)| format!("{k} {}\n", v + v / 20)).collect();
        std::fs::write(BUDGET, format!("# 名前 上限バイト (UPDATE_BUDGET=1 で再生成: 現在値 + 5%)\n{body}")).unwrap();
        return;
    }
    let text = std::fs::read_to_string(BUDGET).expect("bench/budget.txt がない: UPDATE_BUDGET=1 で作る");
    let mut over = vec![];
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let (k, v) = line.split_once(' ').unwrap();
        let limit: u64 = v.trim().parse().unwrap();
        match now.get(k) {
            Some(&cur) if cur > limit => over.push(format!("{k}: {cur} > {limit}")),
            Some(_) => {}
            None => over.push(format!("{k}: 計測されていない")),
        }
    }
    for (k, v) in &now {
        eprintln!("{k:<28} {v:>10}");
    }
    assert!(over.is_empty(), "サイズ予算超過:\n{}", over.join("\n"));
}
