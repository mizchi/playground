"""crustc 自身の src/link.rs (リンカを探して直接起動する) と src/testgen.rs (ベンチ用のプログラム生成器) を
1 ファイルのプログラムにまとめる。
usage: python3 tests/selfhost/build_link.py > out/selfhost/link_main.rs
main は <obj> <out> を受け取り、link::link でリンクして実行し、その出力と testgen の生成結果のハッシュを出す。"""

link = open("src/link.rs").read()
testgen = open("src/testgen.rs").read()
print("""// 自動生成: tests/selfhost/build_link.py (crustc の src/link.rs と src/testgen.rs をそのまま含む)
mod link {
""" + link + """}

mod testgen {
""" + testgen + """}

use std::path::Path;

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    println!("sh found: {}", link::which("sh").is_some());
    println!("abs: {:?}", link::which("/bin/sh").map(|p| p.display().to_string()));
    println!("missing: {:?}", link::which("no-such-linker-xyz"));
    println!("pick bfd: {:?}", link::pick(Some("bfd")).map(|p| p.file_name().unwrap().to_string_lossy().into_owned()));
    println!("pick none: {:?}", link::pick(Some("no-such-linker-xyz")).err());
    for (n, v) in [(3, 0), (50, 0), (50, 7), (400, 1)] {
        let p = testgen::program(n, v);
        println!("testgen {n} {v}: {} bytes {} lines {:016x}", p.len(), p.lines().count(), fnv(&p));
    }
    let (obj, out) = (Path::new(&args[1]), Path::new(&args[2]));
    let _ = std::fs::remove_file(out);
    for round in 0..2 {
        // (2 回目は保存したリンク行 `<out>.link` を使う)
        match link::link(Some("bfd"), obj, out) {
            Ok(l) => println!("link {round}: ok ({l})"),
            Err(e) => println!("link {round}: error {e}"),
        }
        let o = std::process::Command::new(out).output().unwrap();
        println!("run {round}: {:?} {}", String::from_utf8_lossy(&o.stdout), o.status.success());
    }
    println!("saved link line: {}", std::fs::read_to_string(format!("{}.link", out.display())).map(|s| s.lines().count() > 2).unwrap_or(false));
}
""")
