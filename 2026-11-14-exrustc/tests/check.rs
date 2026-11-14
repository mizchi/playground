//! check (LSP の診断) はエラーがあっても止まらず、全関数の診断を返す
use exrustc::{Options, Session};

#[test]
fn reports_all_errors_and_recovers() {
    let src = "enum E { A(i64), B }\nfn f(e: E) -> i64 {\n    match e {\n        E::A(x, y) => x,\n        E::B => 0,\n    }\n}\nfn g() -> bool { 1 }\nfn h() -> u8 { 2 }\nfn main() {}\n";
    let mut s = Session::new(Options::default());
    let d = s.check(src);
    assert_eq!(d.len(), 2, "{d:?}");
    assert_eq!((d[0].line, d[1].line), (4, 8));
    // 直した後は診断が消え、変わっていない関数は検査済みの印で飛ばされる
    let fixed = src.replace("E::A(x, y)", "E::A(x)").replace("-> bool { 1 }", "-> bool { true }");
    assert!(s.check(&fixed).is_empty());
    assert!(s.check(&fixed).is_empty());
}

/// 呼び出し元が「検査済み」でキャッシュに当たっても、ジェネリック関数の本体の型エラーは報告される
/// (検査済みの印に、その関数が要求したインスタンスを保存して要求し直すため)
#[test]
fn generic_body_errors_with_cached_callers() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/traits.rs")).unwrap();
    let mut s = Session::new(Options::default());
    assert!(s.check(&src).is_empty());
    let bad = src.replace("    a.area() + b.area()\n", "    a.area() + b.area() + true\n");
    assert_ne!(bad, src);
    let d = s.check(&bad);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].msg.contains("expected `i64`, found `bool`"), "{d:?}");
    assert!(s.check(&src).is_empty());
}
