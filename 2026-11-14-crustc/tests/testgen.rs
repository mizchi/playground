//! Rust 版生成器が bench/gen.py と同じ出力であること
#[test]
fn matches_python_generator() {
    let py = std::process::Command::new("python3")
        .args(["bench/gen.py", "50", "3"])
        .output();
    let Ok(py) = py else { return }; // python が無い環境では省略
    assert_eq!(
        String::from_utf8(py.stdout).unwrap(),
        exrustc::testgen::program(50, 3)
    );
}
