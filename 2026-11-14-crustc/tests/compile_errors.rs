//! コンパイルエラーの場所 (`行:桁: メッセージ`) が、syn 版のドライバと syn・Cranelift を使わないドライバ (driver2) で同じか。
//! tests/compile_errors/*.rs はどれも 1 つ型のエラーがある
#![cfg(feature = "acg")]

#[test]
fn compile_errors_like_driver() {
    let dir = std::env::temp_dir().join(format!("cerr_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir("tests/compile_errors").unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    let mut bad = vec![];
    for p in &files {
        let src = std::fs::read_to_string(p).unwrap();
        let out = dir.join("x");
        let mut s = crustc::driver::Session::new(crustc::driver::Options { no_cache: true, ..Default::default() });
        let a = s.build(&src, &out).err();
        let b = crustc::driver2::build(&src, &out, false, None).err();
        match (&a, &b) {
            (Some(a), Some(b)) if a == b && a.split(':').next().is_some_and(|l| l.parse::<u32>().is_ok()) => {}
            _ => bad.push(format!("{}: driver {a:?} vs driver2 {b:?}", p.display())),
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    assert!(files.len() >= 10);
}
