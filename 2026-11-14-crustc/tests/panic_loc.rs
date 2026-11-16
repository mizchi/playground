//! panic のメッセージの場所 (`panicked at <file>:<行>:<桁>:`) が rustc と同じか。
//! tests/panic_loc/cases.rs を rustc と driver2 (syn・Cranelift を使わない経路) でビルドし、引数 0〜6 の場合を比べる
//! (添字の範囲外 (Vec / 配列)・オーバーフロー・unwrap・assert!・0 除算・書式マクロの引数の中の expect)。
#![cfg(feature = "acg")]

fn location(stderr: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(stderr);
    let line = s.lines().find(|l| l.contains("panicked at "))?;
    // `<file>:<行>:<桁>:` の行と桁だけ
    let rest = line.split("panicked at ").nth(1)?.trim_end_matches(':');
    let mut parts = rest.rsplitn(3, ':');
    let col = parts.next()?;
    let ln = parts.next()?;
    Some(format!("{ln}:{col}"))
}

#[test]
fn panic_locations_like_rustc() {
    let dir = std::env::temp_dir().join(format!("panic_loc_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src_path = "tests/panic_loc/cases.rs";
    let src = std::fs::read_to_string(src_path).unwrap();
    let r = dir.join("r");
    let st = std::process::Command::new("rustc").args(["-Awarnings", "--edition", "2024", src_path, "-o"]).arg(&r).status().unwrap();
    assert!(st.success());
    let d = dir.join("d");
    crustc::driver2::build(&src, &d, false, None).unwrap();
    let mut bad = vec![];
    for k in 0..7 {
        let a = std::process::Command::new(&r).arg(k.to_string()).output().unwrap();
        let b = std::process::Command::new(&d).arg(k.to_string()).output().unwrap();
        let (la, lb) = (location(&a.stderr), location(&b.stderr));
        if la.is_none() || la != lb || a.status.code() != b.status.code() {
            bad.push(format!("case {k}: rustc {la:?} (exit {:?}) vs driver2 {lb:?} (exit {:?})", a.status.code(), b.status.code()));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
