//! syn・Cranelift を使わない経路 (driver2 + acg + minclif) で tests/acg/*.rs をビルドし、rustc でビルドしたものと出力を比べる。
#![cfg(feature = "acg")]
#[test]
fn acg_like_rustc() {
    let dir = std::env::temp_dir().join(format!("acg_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir("tests/acg").unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    // (ACG_FILE=path で 1 ファイルだけ試す)
    if let Ok(f) = std::env::var("ACG_FILE") {
        files = vec![std::path::PathBuf::from(f)];
    }
    let mut bad = vec![];
    for p in &files {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(p).unwrap();
        let obj = match crustc::driver2::build_object(&src, false) {
            Ok(o) => o,
            Err(e) => {
                bad.push(format!("{name}: {e}"));
                continue;
            }
        };
        let o = dir.join(format!("{name}.o"));
        std::fs::write(&o, &obj).unwrap();
        let exe = dir.join(&name);
        let st = std::process::Command::new("cc").arg(&o).arg("-o").arg(&exe).status().unwrap();
        if !st.success() {
            bad.push(format!("{name}: link failed"));
            continue;
        }
        let ours = std::process::Command::new(&exe).output().unwrap();
        let rexe = dir.join(format!("{name}.rustc"));
        let st = std::process::Command::new("rustc")
            .args(["-Awarnings", "--edition", "2024", "-C", "panic=abort", "-C", "link-arg=-lc", "-C", "link-arg=-Wl,--defsym=rust_eh_personality=0"])
            .arg(p)
            .arg("-o")
            .arg(&rexe)
            .status()
            .unwrap();
        assert!(st.success());
        let theirs = std::process::Command::new(&rexe).output().unwrap();
        if ours.stdout != theirs.stdout || ours.status.code() != theirs.status.code() {
            bad.push(format!(
                "{name}: output differs:\n{}\nvs rustc:\n{}",
                String::from_utf8_lossy(&ours.stdout),
                String::from_utf8_lossy(&theirs.stdout)
            ));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
