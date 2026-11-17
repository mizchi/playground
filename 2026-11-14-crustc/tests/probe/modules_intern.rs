// モジュールごとの名前空間 (同じ名前の型・関数・const を別のモジュールに)、use の別名、
// Mutex / OnceLock / Box::leak による intern、ptr::eq、再帰する型 (参照を通す) の ==、
// &String と &str の比較・受け渡し、Vec<u8> とバイト列の比較、ガードつきの腕の中の match、後で代入する let
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

mod a {
    #[derive(Debug, Clone, PartialEq)]
    pub enum Ty { Int, Name(String) }
    pub fn make(n: &str) -> Ty { if n == "int" { Ty::Int } else { Ty::Name(n.to_string()) } }
    pub const LIMIT: u32 = 3;
}

mod b {
    use crate::a::{Ty as ATy, make};
    #[derive(Debug)]
    pub struct Ty { pub inner: ATy, pub depth: u32 }
    pub fn wrap(n: &str) -> Ty { Ty { inner: make(n), depth: crate::a::LIMIT } }
    pub const LIMIT: u32 = 7;
    pub fn lim() -> u32 { LIMIT + super::a::LIMIT }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
enum T { A, R(&'static T), N(u32) }

fn intern(t: T) -> &'static T {
    static TABLE: OnceLock<Mutex<HashSet<&'static T>>> = OnceLock::new();
    let mut tab = TABLE.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap();
    if let Some(&r) = tab.get(&t) {
        return r;
    }
    let r: &'static T = Box::leak(Box::new(t));
    tab.insert(r);
    r
}

enum E { Lit(String), P(u8) }
fn num(t: &str) -> Option<u64> { t.parse().ok() }
fn classify(e: &E) -> Result<u64, String> {
    match e {
        E::Lit(t) if t.starts_with(|c: char| c.is_ascii_digit()) => match num(t) {
            Some(n) => Ok(n),
            None => Err("bad".to_string()),
        },
        E::Lit(t) => Err(format!("word {t}")),
        E::P(n) => Ok(*n as u64),
    }
}

fn main() {
    let t = b::wrap("x");
    println!("{:?} {:?} {} {}", t, a::make("int"), b::lim(), t.inner == a::Ty::Name("x".into()));
    let x = intern(T::R(intern(T::N(3))));
    let y = intern(T::R(intern(T::N(3))));
    println!("{} {} {:?} {}", std::ptr::eq(x, y), std::ptr::eq(x, intern(T::A)), x, *x == T::R(&T::N(3)));
    for e in [E::Lit("42".into()), E::Lit("4x".into()), E::Lit("hi".into()), E::P(7)] {
        println!("{:?}", classify(&e));
    }
    let quals = vec!["pub".to_string(), "unsafe".to_string()];
    println!("{} {}", quals.iter().any(|q| q == "unsafe"), quals.iter().any(|q| q == "const"));
    let v = b"crate".to_vec();
    let segs = vec![b"self".to_vec(), v.clone()];
    println!("{} {} {}", v == b"crate", segs[0] == b"crate", segs.iter().any(|s| s == b"crate"));
    let kind;
    let n: u32;
    if v.len() > 3 { kind = "long"; n = v.len() as u32; } else { kind = "short"; n = 0; }
    println!("{kind} {n}");
}
