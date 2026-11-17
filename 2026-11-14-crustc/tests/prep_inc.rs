//! 差分の前処理 (prep::prep) が、毎回全体を前処理したもの (prep::prep_full) と同じになるか。
//! 実際のソース (examples/*.rs と、crates/core/src のファイルを `mod x { .. }` で束ねたもの) に乱数で編集を続けて加え、
//! 各段で本文・桁のずれ・成否を比べる。編集は関数の本体の中 (骨組みが変わらない: 差分の経路) と、どこでも (骨組みが
//! 変わる: 全体の経路) の両方

use crustc::prep::{PrepMemo, prep, prep_full};

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
    // (半分は `let` の行の直後 (関数の本体の中) に、半分はどこでも)
    let at = if r.below(2) == 0 {
        let lets: Vec<usize> = s.match_indices("let ").map(|(i, _)| i).collect();
        if lets.is_empty() { char_floor(s, r.below(s.len() + 1)) } else { lets[r.below(lets.len())] }
    } else {
        char_floor(s, r.below(s.len() + 1))
    };
    let snippets = [
        "x", " ", "\n", "let q = 1;\n", "let v = Some(1).map(Some);\n", "{ fn inner() {} }\n", "\"", "'", "/*", "*/", "//", ";", "1 + 2",
        "fn added() {}\n", "\nmod mm { pub fn f() {} }\n", "struct T(u8);\n", "use std::path::Path;\n", "m!();\n", "thread_local! { static TT: u8 = 1; }\n",
        "\nmacro_rules! m { () => { 1 }; }\n",
    ];
    match r.below(3) {
        0 | 1 => {
            let t = snippets[r.below(snippets.len())];
            format!("{}{}{}", &s[..at], t, &s[at..])
        }
        _ => {
            let end = char_floor(s, (at + 1 + r.below(30)).min(s.len())).max(at);
            format!("{}{}", &s[..at], &s[end..])
        }
    }
}

fn sources() -> Vec<String> {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut v = vec![];
    let mut ps: Vec<_> = std::fs::read_dir(format!("{root}/examples")).unwrap().map(|e| e.unwrap().path()).collect();
    ps.sort();
    for p in ps.iter().filter(|p| p.extension().is_some_and(|e| e == "rs")).take(25) {
        v.push(std::fs::read_to_string(p).unwrap());
    }
    // (モジュールに束ねたもの: 名前のぶつかりの書き換え・use・入れ子のアイテム・マクロ)
    let mut bundle = String::from("fn main() {}\n");
    for m in ["ctors", "cfg", "stub", "macros"] {
        let t = std::fs::read_to_string(format!("{root}/crates/core/src/{m}.rs")).unwrap();
        let t: String = t.lines().filter(|l| !l.starts_with("//!") && !l.starts_with("#![")).map(|l| format!("{l}\n")).collect();
        bundle.push_str(&format!("mod {m} {{\n{t}}}\n"));
    }
    v.push(bundle);
    v
}

#[test]
fn prep_inc_matches_full() {
    let mut r = Rng(0x2545_f491_4f6c_dd1d);
    let (mut same, mut errs) = (0, 0);
    for src in sources() {
        let mut memo = PrepMemo::default();
        let mut cur = src.clone();
        let mut prev = src.clone();
        for step in 0..40 {
            let mut f1 = vec![];
            let mut f2 = vec![];
            let full = prep_full(&cur, &[], &mut f1, None).map(|p| p.text);
            let inc = prep(&cur, &[], &mut f2, None, &mut memo);
            match (full, inc) {
                (Ok(a), Ok(b)) => {
                    if a != b || f1 != f2 {
                        let d = env!("CARGO_TARGET_TMPDIR");
                        let _ = std::fs::write(format!("{d}/prep_inc_src.rs"), &cur);
                        let _ = std::fs::write(format!("{d}/prep_inc_full.rs"), &a);
                        let _ = std::fs::write(format!("{d}/prep_inc_inc.rs"), &b);
                        panic!("differs after {step} edits (fix same: {})", f1 == f2);
                    }
                    same += 1;
                }
                (Err(_), Err(_)) => {
                    errs += 1;
                    cur = prev.clone();
                }
                (a, b) => panic!("ok / err differ after {step} edits: full {:?} inc {:?}", a.err(), b.err()),
            }
            prev = cur.clone();
            cur = edit(&cur, &mut r);
        }
    }
    assert!(same > 500, "same {same}, errors {errs}");
}
