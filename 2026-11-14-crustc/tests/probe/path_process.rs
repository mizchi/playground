// std::path (Path / PathBuf / OsString) と std::process::Command、env::var_os / split_paths、
// 同じ型への複数の From impl、書式文字列の中の名前をキャプチャするクロージャ
use std::path::{Path, PathBuf};
use std::process::Command;

struct Celsius(f64);
impl From<i32> for Celsius {
    fn from(x: i32) -> Celsius { Celsius(x as f64) }
}
impl From<&str> for Celsius {
    fn from(s: &str) -> Celsius { Celsius(s.trim_end_matches('C').parse().unwrap()) }
}

fn main() {
    let p = PathBuf::from("/usr/bin");
    let q = p.join("ld");
    println!("{} {:?} {} {:?}", q.display(), q.file_name(), q.is_absolute(), q);
    let mut l = q.as_os_str().to_owned();
    l.push(".link");
    println!("{}", PathBuf::from(l).display());
    let a = Path::new("dir/sub/main.rs");
    println!("{:?} {:?} {:?} {}", a.extension(), a.file_stem(), a.parent(), a.with_extension("o").display());
    let mut b = PathBuf::from("x/y");
    b.push("z.txt");
    b.set_extension("md");
    println!("{} {}", b.display(), Path::new("/").join("etc").display());
    println!("{} {}", Path::new("/bin/sh").is_file(), Path::new("/").is_dir());
    let o = Command::new("sh").arg("-c").arg("echo out; echo err 1>&2; exit 3").output().unwrap();
    println!("{:?} {:?} {} {}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr), o.status.success(), o.status);
    let st = Command::new("true").status().unwrap();
    println!("{} {}", st.success(), st);
    match Command::new("no-such-cmd-xyz").output() {
        Ok(_) => println!("ok?"),
        Err(e) => println!("err {}", e),
    }
    let path = std::env::var_os("PATH").unwrap();
    println!("{}", std::env::split_paths(&path).map(|d| d.join("sh")).any(|p| p.is_file()));
    let args: Vec<String> = vec!["a".into(), "b c".into()];
    let o = Command::new("echo").args(&args).args(&["x", "y"]).output().unwrap();
    println!("{:?}", String::from_utf8_lossy(&o.stdout).into_owned());
    let c1: Celsius = 21.into();
    let c2 = Celsius::from("36.5C");
    println!("{} {}", c1.0, c2.0);
    let names = vec!["p", "q"];
    let f = || format!("names={names:?} n={}", names.len());
    println!("{}", f());
}
