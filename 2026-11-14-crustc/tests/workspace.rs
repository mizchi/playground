//! 複数クレート・複数ファイルのワークスペース (tests/workspace) を `Session2::build_workspace` でビルドし、
//! rustc (rlib + リンク) で作ったものと出力・終了コード・panic のメッセージ (クレートとファイルをまたぐ場所) が同じか。
//! さらに、依存されるクレートの関数の本体だけを変えると、そのクレートだけをコンパイルし直すか
#![cfg(feature = "acg")]

use std::path::{Path, PathBuf};
use std::process::Command;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        let q = to.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &q);
        } else {
            std::fs::copy(&p, &q).unwrap();
        }
    }
}

fn run(exe: &Path, dir: &Path, n: usize) -> (String, String, Option<i32>) {
    let o = Command::new(exe).args(std::iter::repeat("x").take(n)).current_dir(dir).env_remove("RUST_BACKTRACE").output().unwrap();
    // (新しい rustc はスレッドの ID を出す: `thread 'main' (1234) panicked`)
    let mut err = String::from_utf8_lossy(&o.stderr).into_owned();
    if let Some(i) = err.find("thread 'main' (") {
        if let Some(j) = err[i..].find(") ") {
            err.replace_range(i + 13..i + j + 1, "");
        }
    }
    (String::from_utf8_lossy(&o.stdout).into_owned(), err, o.status.code())
}

#[test]
fn workspace_matches_rustc_and_rebuilds_incrementally() {
    let dir = std::env::temp_dir().join(format!("crustc_ws_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(Path::new("tests/workspace"), &dir);
    let rs: PathBuf = dir.join("target_rustc");
    std::fs::create_dir_all(&rs).unwrap();
    let st = Command::new("bash").arg(dir.join("build_rustc.sh")).arg(&rs).status().unwrap();
    assert!(st.success());
    let crates = crustc::driver2::read_workspace(&dir.join("workspace.txt")).unwrap();
    let out = dir.join("target");
    let mut s = crustc::driver2::Session2::new(crustc::driver2::Options2::default());
    let r = s.build_workspace(&crates, &out).unwrap();
    assert_eq!(r.linked, 1);
    // (引数の数で: 0 = 依存するクレートの中の 0 除算, 1 = 普通, 2 = bin のモジュールのファイルの添字, 3 = unwrap)
    for n in [0, 1, 2, 3, 5] {
        let a = run(&out.join("app"), &dir, n);
        let b = run(&rs.join("app"), &dir, n);
        assert_eq!(a, b, "args {n}");
    }
    // 何も変えなければ何もしない
    let r = s.build_workspace(&crates, &out).unwrap();
    assert_eq!((r.objects_written, r.linked), (0, 0));
    assert!(r.crates.iter().all(|(_, c)| c.fns_compiled == 0 && c.fns_hit == 0));
    // util の関数の本体だけを変える: util だけコンパイルし直す (geom・app はインターフェースが同じなので見ない)
    let lib = dir.join("util/src/lib.rs");
    let t = std::fs::read_to_string(&lib).unwrap().replace("x * 2", "x * 3");
    std::fs::write(&lib, t).unwrap();
    let r = s.build_workspace(&crates, &out).unwrap();
    let compiled: Vec<&str> = r.crates.iter().filter(|(_, c)| c.fns_compiled > 0).map(|(n, _)| n.as_str()).collect();
    assert_eq!(compiled, ["util"]);
    assert_eq!(r.linked, 1);
    let (o, _, _) = run(&out.join("app"), &dir, 1);
    assert!(o.starts_with("(9, -12)"), "{o}");
    // シグネチャを変えると、それを使う側だけコンパイルし直す (geom は calls を使わないので見ない)
    let t = std::fs::read_to_string(&lib).unwrap().replace("pub fn calls() -> u32 { unsafe { CALLS } }", "pub fn calls() -> u64 { unsafe { CALLS as u64 } }");
    std::fs::write(&lib, t).unwrap();
    let r = s.build_workspace(&crates, &out).unwrap();
    let rebuilt: Vec<&str> = r.crates.iter().filter(|(_, c)| c.fns_compiled + c.fns_hit > 0).map(|(n, _)| n.as_str()).collect();
    assert_eq!(rebuilt, ["util", "app"]);
    let (o, _, _) = run(&out.join("app"), &dir, 1);
    assert!(o.contains("calls 2"), "{o}");
    // 本体だけを変えれば util だけ
    let t = std::fs::read_to_string(&lib).unwrap().replace("pub fn helper(x: i64) -> i64 { unsafe { CALLS += 1; } x * 3 }", "pub fn helper(x: i64) -> i64 { unsafe { CALLS += 2; } x * 3 }");
    std::fs::write(&lib, t).unwrap();
    let r = s.build_workspace(&crates, &out).unwrap();
    let compiled: Vec<&str> = r.crates.iter().filter(|(_, c)| c.fns_compiled > 0).map(|(n, _)| n.as_str()).collect();
    assert_eq!(compiled, ["util"]);
    let t = std::fs::read_to_string(&lib).unwrap().replace("pub fn helper(x: i64) -> i64 {", "pub fn helper(x: i64) -> i64 { let _ = 0u8;");
    std::fs::write(&lib, t).unwrap();
    let r = s.build_workspace(&crates, &out).unwrap();
    let rebuilt: Vec<&str> = r.crates.iter().filter(|(_, c)| c.fns_compiled + c.fns_hit > 0).map(|(n, _)| n.as_str()).collect();
    assert_eq!(rebuilt, ["util"], "body change only");
    // geom が使う関数のシグネチャを変えると geom も (rustc で作ったものと同じ動き)
    let geom = dir.join("geom/src/lib.rs");
    let t = std::fs::read_to_string(&lib).unwrap().replace("pub fn helper(x: i64) -> i64 { let _ = 0u8; unsafe { CALLS += 2; } x * 3 }", "pub fn helper(x: i32) -> i64 { unsafe { CALLS += 2; } x as i64 * 3 }");
    std::fs::write(&lib, t).unwrap();
    let g = std::fs::read_to_string(&geom).unwrap().replace("P::new(util::helper(self.x), util::helper(self.y))", "P::new(util::helper(self.x as i32), util::helper(self.y as i32))");
    std::fs::write(&geom, g).unwrap();
    let r = s.build_workspace(&crates, &out).unwrap();
    let rebuilt: Vec<&str> = r.crates.iter().filter(|(_, c)| c.fns_compiled + c.fns_hit > 0).map(|(n, _)| n.as_str()).collect();
    assert!(rebuilt.contains(&"util") && rebuilt.contains(&"geom"), "{rebuilt:?}");
    let (a, _, _) = run(&out.join("app"), &dir, 1);
    let rs2 = dir.join("target_rustc2");
    std::fs::create_dir_all(&rs2).unwrap();
    let st = Command::new("bash").arg(dir.join("build_rustc.sh")).arg(&rs2).status().unwrap();
    assert!(st.success());
    let (b, _, _) = run(&rs2.join("app"), &dir, 1);
    assert_eq!(a, b);
    let _ = std::fs::remove_dir_all(&dir);
}
