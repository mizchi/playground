//! 自前の構文木からのアイテムの要約 (src/ast_summary.rs) を syn 版 (front::parse_and_summarize) と比べる:
//! driver と同じ前処理 (マクロ展開・モジュールを平らに・入れ子のアイテムを外へ) の後の全アイテムで、要約が同じか。
use crustc::front::{self, Summary};
use crustc::lexer::{self, ItemKind};

fn prep(src: &str) -> String {
    let s = crustc::macros::expand(src).ok().flatten().unwrap_or_else(|| src.to_string());
    let s = lexer::flatten_modules(&s).unwrap_or(s);
    lexer::hoist_nested_items(&s).unwrap_or(s)
}

#[test]
fn summaries_like_syn() {
    let mut files: Vec<std::path::PathBuf> = vec![];
    for d in ["tests/probe", "examples", "src"] {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    files.sort();
    let mut bad = vec![];
    let mut total = 0;
    for p in &files {
        let src = prep(&std::fs::read_to_string(p).unwrap());
        let Ok(items) = lexer::split(&src) else { continue };
        let is_header = |k: ItemKind| matches!(k, ItemKind::ImplHeader | ItemKind::TraitHeader);
        let mut a: Vec<Summary> = vec![Summary::default(); items.len()];
        let mut b: Vec<Summary> = vec![Summary::default(); items.len()];
        let mut ra: Vec<Option<Result<String, String>>> = vec![None; items.len()];
        let mut rb: Vec<Option<Result<String, String>>> = vec![None; items.len()];
        for pass in 0..2 {
            for (i, it) in items.iter().enumerate() {
                if is_header(it.kind) != (pass == 0) {
                    continue;
                }
                let oa = if pass == 1 { crustc::ast_summary::owner_of(it, &a) } else { None };
                let ob = if pass == 1 { crustc::ast_summary::owner_of(it, &b) } else { None };
                let x = front::parse_and_summarize(&src, it, oa.as_ref()).map(|r| r.0);
                let y = crustc::ast_summary::parse_and_summarize(&src, it, ob.as_ref());
                ra[i] = Some(x.as_ref().map(|s| format!("{s:?}")).map_err(|e| e.clone()));
                rb[i] = Some(y.as_ref().map(|s| format!("{s:?}")).map_err(|e| e.clone()));
                if let Ok(s) = x {
                    a[i] = s;
                }
                if let Ok(s) = y {
                    b[i] = s;
                }
            }
        }
        for i in 0..items.len() {
            total += 1;
            let same = match (&ra[i], &rb[i]) {
                (Some(Ok(x)), Some(Ok(y))) => x == y,
                (Some(Err(_)), Some(Err(_))) => true,
                _ => false,
            };
            if !same {
                let t = &src[items[i].start..items[i].end];
                bad.push(format!("{} item {i} `{}`:\n    syn  {:?}\n    ours {:?}", p.display(), &t[..t.len().min(60)].replace('\n', " "), ra[i], rb[i]));
            }
        }
    }
    let n = bad.len();
    bad.truncate(12);
    assert!(n == 0, "{n} differences over {total} items:\n{}", bad.join("\n"));
    eprintln!("{total} items checked");
}
