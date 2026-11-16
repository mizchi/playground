//! syn・Cranelift を使わないドライバ (driver2 + acg + minclif) で tests/probe と examples をビルドし、
//! 今のドライバ (syn + Cranelift) でビルドしたものと出力 (stdout と終了コード) を比べる。
//! DRIVER2_FILE=path で 1 ファイルだけ、DRIVER2_MIN=n で一致しなければならない数を指定する。
#![cfg(feature = "acg")]
use std::path::PathBuf;

#[test]
fn driver2_like_driver() {
    let dir = std::env::temp_dir().join(format!("drv2_{}", std::process::id()));
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
    if let Ok(f) = std::env::var("DRIVER2_FILE") {
        files = vec![PathBuf::from(f)];
    }
    let (mut ok, mut bad) = (0, vec![]);
    for p in &files {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(p).unwrap();
        // (no_std の例は C の main と libc でリンクする前提: 今のドライバも同じリンカを使う)
        let ref_exe = dir.join(format!("{name}.ref"));
        let mut s = crustc::driver::Session::new(crustc::driver::Options { no_cache: true, ..Default::default() });
        if s.build(&src, &ref_exe).is_err() {
            continue;
        }
        let exe = dir.join(&name);
        if let Err(e) = crustc::driver2::build(&src, &exe, false, None) {
            bad.push(format!("{name}: {}", e.lines().next().unwrap_or("")));
            continue;
        }
        let run = |e: &PathBuf| std::process::Command::new(e).stdin(std::process::Stdio::null()).output();
        let (Ok(a), Ok(b)) = (run(&exe), run(&ref_exe)) else {
            bad.push(format!("{name}: failed to run"));
            continue;
        };
        if a.stdout == b.stdout && a.status.code() == b.status.code() {
            ok += 1;
        } else {
            bad.push(format!("{name}: output differs (exit {:?} vs {:?})", a.status.code(), b.status.code()));
            if std::env::var("DRIVER2_FILE").is_ok() {
                let (x, y) = (String::from_utf8_lossy(&a.stdout), String::from_utf8_lossy(&b.stdout));
                for (i, (l1, l2)) in x.lines().zip(y.lines()).enumerate() {
                    if l1 != l2 {
                        eprintln!("first diff at line {i}:\n  ours: {l1}\n  ref:  {l2}");
                        break;
                    }
                }
                eprintln!("ours {} lines, ref {} lines; ours stderr: {}", x.lines().count(), y.lines().count(), String::from_utf8_lossy(&a.stderr));
            }
        }
    }
    eprintln!("{ok} same, {} differ", bad.len());
    for b in &bad {
        eprintln!("  {b}");
    }
    let min: usize = std::env::var("DRIVER2_MIN").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    assert!(ok >= min, "only {ok} programs match (need {min})");
}
