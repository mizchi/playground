//! macros::expand (速い版: 全体は lexer::tokenize で見る) が、前の版 (expand_reference: 全体を proc_macro2 で読む) と
//! 同じ結果になるか。macro_rules! を含むテスト・例・crustc 自身のソースと、セルフホスト用にまとめたコンパイラで比べる

fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn fast_expand_matches_reference() {
    let mut files = vec![];
    for d in ["tests", "examples", "crates", "out/selfhost"] {
        rs_files(std::path::Path::new(d), &mut files);
    }
    let mut n = 0;
    let mut bad = vec![];
    for p in &files {
        let Ok(src) = std::fs::read_to_string(p) else { continue };
        if !src.contains("macro_rules!") {
            continue;
        }
        n += 1;
        let a = crustc::macros::expand(&src);
        let b = crustc::macros::expand_reference(&src);
        // (どちらも失敗するもの (読めないソース) は数えない。メッセージが違っても場所が同じならよい)
        match (&a, &b) {
            (Ok(x), Ok(y)) if x == y => {}
            (Err(x), Err(y)) if x.split(':').take(2).eq(y.split(':').take(2)) => {}
            _ => bad.push(format!("{}: {:?}", p.display(), (a.as_ref().map(|o| o.as_ref().map(|s| s.len())), b.as_ref().map(|o| o.as_ref().map(|s| s.len()))))),
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    assert!(n >= 5, "{n}");
}
