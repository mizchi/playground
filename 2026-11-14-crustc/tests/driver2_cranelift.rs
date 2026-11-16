//! driver2 の Cranelift バックエンド (minclif が記録した命令の列を Cranelift でコンパイルする) で tests/probe・examples・
//! tests/acg をビルドし、minclif でビルドしたものと出力 (stdout・stderr・終了コード) が同じか (-O なしと -O)。
//! (-O はオーバーフローの検査をしないので、名前に overflow を含むものは -O では比べない)
#![cfg(feature = "acg")]
use std::path::PathBuf;

fn build(src: &str, out: &PathBuf, opt: bool, cl: bool) -> Result<crustc::driver2::Report2, String> {
    let mut o = crustc::driver2::Options2 { opt, no_cache: true, jobs: 1, ..Default::default() };
    if cl {
        o.backend = Some(crustc::clif_backend::compile);
    }
    crustc::driver2::Session2::new(o).build(src, out)
}

#[test]
fn cranelift_backend_matches_minclif() {
    let dir = std::env::temp_dir().join(format!("drv2cl_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut files: Vec<PathBuf> = vec![];
    for d in ["tests/probe", "examples", "tests/acg"] {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    files.sort();
    let (mut ok, mut bad) = (0, vec![]);
    for p in &files {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(p).unwrap();
        for opt in [false, true] {
            if opt && name.contains("overflow") {
                continue;
            }
            let (a, b) = (dir.join(format!("{name}.{opt}.m")), dir.join(format!("{name}.{opt}.c")));
            if build(&src, &a, opt, false).is_err() {
                continue;
            }
            if let Err(e) = build(&src, &b, opt, true) {
                bad.push(format!("{name} (opt {opt}): cranelift build failed: {}", e.lines().next().unwrap_or("")));
                continue;
            }
            let run = |e: &PathBuf| std::process::Command::new(e).stdin(std::process::Stdio::null()).output().unwrap();
            let (x, y) = (run(&a), run(&b));
            if x.stdout == y.stdout && x.stderr == y.stderr && x.status.code() == y.status.code() {
                ok += 1;
            } else {
                bad.push(format!("{name} (opt {opt}): differs (exit {:?} vs {:?})", x.status.code(), y.status.code()));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    assert!(ok >= 260, "{ok}");
}
