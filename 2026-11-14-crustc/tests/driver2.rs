//! tests/probe・examples・tests/acg を driver2 (minclif) でビルドして動かし、出力 (stdout・stderr・終了コード) が
//! tests/expected/<ディレクトリ>.<名前>.{stdout,stderr,code} と同じか。期待値は最初の syn + Cranelift のドライバで作ったもの
//! (そのドライバは消した)。Cranelift のバックエンドは tests/driver2_cranelift.rs が minclif と比べる。
//! DRIVER2_FILE=path で 1 ファイルだけ。
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;

#[test]
fn driver2_matches_expected() {
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
        let d = p.parent().unwrap().file_name().unwrap().to_string_lossy().to_string();
        let exp = |ext: &str| PathBuf::from("tests/expected").join(format!("{d}.{name}.{ext}"));
        let (Ok(want_out), Ok(want_err), Ok(want_code)) = (std::fs::read(exp("stdout")), std::fs::read(exp("stderr")), std::fs::read_to_string(exp("code"))) else {
            bad.push(format!("{d}/{name}: no expected output in tests/expected"));
            continue;
        };
        let src = std::fs::read_to_string(p).unwrap();
        let exe = dir.join(&name);
        if let Err(e) = crustc::driver2::build(&src, &exe, false, None) {
            bad.push(format!("{name}: {}", e.lines().next().unwrap_or("")));
            continue;
        }
        let a = std::process::Command::new(&exe).current_dir("tests/expected").stdin(std::process::Stdio::null()).output().unwrap();
        let code = a.status.code().unwrap_or_else(|| 128 + a.status.signal().unwrap_or(0));
        if a.stdout == want_out && a.stderr == want_err && code.to_string() == want_code.trim() {
            ok += 1;
        } else {
            let why = if a.stdout != want_out { "stdout" } else if a.stderr != want_err { "stderr" } else { "exit code" };
            bad.push(format!("{d}/{name}: {why} differs (exit {code} vs {})", want_code.trim()));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{ok} same\n{}", bad.join("\n"));
}
