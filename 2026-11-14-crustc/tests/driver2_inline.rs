//! driver2 の `-O` (小さい関数のインライン展開 + minclif の読み直しの省略) で tests/probe・examples・tests/acg を
//! ビルドし、`-O` なしと出力 (stdout・stderr・終了コード) が同じか。
//! (`-O` はオーバーフローの検査をしないので、オーバーフローで panic するもの (名前に overflow を含む) は除く)
#![cfg(feature = "acg")]
use std::path::PathBuf;

#[test]
fn inline_keeps_behavior() {
    let dir = std::env::temp_dir().join(format!("drv2inl_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut files: Vec<PathBuf> = vec![];
    for d in ["tests/probe", "examples", "tests/acg"] {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "rs") && !p.to_string_lossy().contains("overflow") {
                files.push(p);
            }
        }
    }
    files.sort();
    let (mut ok, mut bad) = (0, vec![]);
    for p in &files {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(p).unwrap();
        let (a, b) = (dir.join(format!("{name}.0")), dir.join(format!("{name}.1")));
        if crustc::driver2::build(&src, &a, false, None).is_err() {
            continue;
        }
        if let Err(e) = crustc::driver2::build(&src, &b, true, None) {
            bad.push(format!("{name}: -O build failed: {}", e.lines().next().unwrap_or("")));
            continue;
        }
        let run = |e: &PathBuf| std::process::Command::new(e).stdin(std::process::Stdio::null()).output().unwrap();
        let (x, y) = (run(&a), run(&b));
        if x.stdout == y.stdout && x.stderr == y.stderr && x.status.code() == y.status.code() {
            ok += 1;
        } else {
            bad.push(format!("{name}: differs (exit {:?} vs {:?})", x.status.code(), y.status.code()));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    assert!(ok >= 130, "{ok}");
}
