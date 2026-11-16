//! driver2 のキャッシュと並列化: 2 回目のビルドは全部キャッシュから読み同じ .o になること、
//! 関数 1 つの変更ではその関数だけコンパイルし直すこと、fork で並列にコンパイルしても 1 プロセスと同じ .o になること。
#![cfg(feature = "acg")]
use crustc::driver2::{Options2, Session2};

fn program(k: u32) -> String {
    let mut s = String::new();
    for i in 0..120 {
        let m = if i == 7 { k } else { 31 };
        s.push_str(&format!("fn f{i}(x: u64) -> u64 {{ let mut s = x; for j in 0..{} {{ s = s.wrapping_mul({m}).wrapping_add(j * {i}); }} s % 1000 }}\n", i % 5 + 2));
    }
    s.push_str("fn main() { let mut t = 1u64; let v: Vec<u64> = (0..10).collect();\n");
    for i in 0..120 {
        s.push_str(&format!("    t = t.wrapping_add(f{i}(t + v[{}]));\n", i % 10));
    }
    s.push_str("    println!(\"{t}\");\n}\n");
    s
}

#[test]
fn cache_and_parallel() {
    let dir = std::env::temp_dir().join(format!("d2cache_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("p");
    let cp = dir.join("p.crcache2");
    let run = |src: &str, jobs: usize, cache: bool| {
        let mut s = Session2::new(Options2 { jobs, no_cache: !cache, ..Default::default() });
        if cache {
            s.load_cache(&cp);
        }
        let rep = s.build(src, &out).unwrap();
        if cache {
            s.save_cache(&cp).unwrap();
        }
        let obj = std::fs::read(out.with_extension("o")).unwrap();
        let o = std::process::Command::new(&out).output().unwrap();
        (rep, obj, o.stdout)
    };
    let src = program(31);
    let (r1, o1, s1) = run(&src, 1, true);
    assert!(r1.fns_compiled > 120 && r1.fns_hit == 0, "{r1:?}");
    let (r2, o2, s2) = run(&src, 1, true);
    assert_eq!(r2.fns_compiled, 0, "{r2:?}");
    assert_eq!(r2.items_parsed, 0, "{r2:?}");
    assert!(o1 == o2 && s1 == s2);
    // 関数 1 つだけ変える
    let src2 = program(37);
    let (r3, o3, s3) = run(&src2, 1, true);
    assert_eq!(r3.fns_compiled, 1, "{r3:?}");
    // 並列 (fork) とキャッシュなしの 1 プロセスで同じ .o と出力
    let (_, o4, s4) = run(&src2, 4, false);
    let (_, o5, s5) = run(&src2, 1, false);
    assert!(o3 == o4 && o4 == o5, "objects differ");
    assert!(s3 == s4 && s4 == s5);
}
