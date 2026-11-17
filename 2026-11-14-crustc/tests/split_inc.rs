//! 差分の split (driver2::split_inc) が、毎回全体を split したものと同じになるか。
//! 実際のソース (crates/core/src/*.rs と examples/*.rs) に、乱数で編集 (文字の挿入・削除、引用符やコメントの開き、
//! 関数の追加・削除) を続けて加え、各段で比べる

use crustc::driver2::split_inc;
use crustc::lexer::{self, SrcItem};

fn same(a: &[SrcItem], b: &[SrcItem]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.start == y.start && x.end == y.end && x.line == y.line && x.hash == y.hash && x.idents == y.idents && x.kind == y.kind
        })
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn char_floor(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn edit(s: &str, r: &mut Rng) -> String {
    let at = char_floor(s, r.below(s.len() + 1));
    let snippets = [
        "x", " ", "\n", "}", "{", "\"", "'", "/*", "*/", "//", "#", ";", "1 + 2", "fn added() {}\n", "\nfn f2(a: u32) -> u32 { a + 1 }\n",
        "impl S { fn m(&self) {} }\n", "r#\"", "b'x'", "'a", "\n#[derive(Clone)]\nstruct Q;\n",
    ];
    match r.below(3) {
        // 挿入
        0 | 1 => {
            let t = snippets[r.below(snippets.len())];
            format!("{}{}{}", &s[..at], t, &s[at..])
        }
        // 削除
        _ => {
            let end = char_floor(s, (at + 1 + r.below(40)).min(s.len()));
            let end = end.max(at);
            format!("{}{}", &s[..at], &s[end..])
        }
    }
}

#[test]
fn split_inc_matches_full_split() {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut files: Vec<String> = vec![];
    for dir in ["crates/core/src", "examples"] {
        let mut ps: Vec<_> = std::fs::read_dir(format!("{root}/{dir}")).unwrap().map(|e| e.unwrap().path()).collect();
        ps.sort();
        for p in ps {
            if p.extension().is_some_and(|e| e == "rs") {
                files.push(std::fs::read_to_string(&p).unwrap());
            }
        }
    }
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut checked, mut errs) = (0, 0);
    for src in files.iter().take(40) {
        let mut memo = None;
        let mut cur = src.clone();
        let mut prev = src.clone();
        for step in 0..60 {
            let full = lexer::split(&cur);
            let inc = split_inc(&cur, &mut memo);
            match (full, inc) {
                (Ok(f), Ok(i)) => {
                    assert!(same(&f, &i), "differs after {step} edits");
                    checked += 1;
                }
                (Err(_), Err(_)) => {
                    // (split できない編集は戻す: 正しいソースへの編集を多く試す)
                    errs += 1;
                    cur = prev.clone();
                }
                (f, i) => panic!("ok / err differ after {step} edits: full {:?} inc {:?}", f.map(|v| v.len()), i.map(|v| v.len())),
            }
            // (失敗したら memo は前の成功のまま: 次の編集はそこからの差分)
            prev = cur.clone();
            cur = edit(&cur, &mut r);
        }
    }
    assert!(checked > 1000, "checked {checked}, errors {errs}");
}
