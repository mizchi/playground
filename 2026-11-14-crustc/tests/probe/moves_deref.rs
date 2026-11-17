// let-else の同名の束縛・一時値のフィールドのムーブ・Deref 越しのフィールド・Option::iter・クロージャで split・`&&T`
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

fn table() -> (HashMap<usize, u32>, Option<String>, Vec<Option<u32>>) {
    let mut m = HashMap::new();
    m.insert(1, 2);
    (m, Some("t".to_string()), vec![None, Some(4)])
}

struct Inner {
    a: u32,
    names: Vec<String>,
}

struct Outer {
    inner: Inner,
    extra: u8,
}

impl Deref for Outer {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.inner
    }
}

impl DerefMut for Outer {
    fn deref_mut(&mut self) -> &mut Inner {
        &mut self.inner
    }
}

impl Outer {
    fn bump(&mut self, n: &str) -> usize {
        self.a += 1;
        self.names.push(n.to_string());
        self.names.len() + self.extra as usize
    }
}

fn main() {
    // let-else で外の変数を同じ名前で隠す
    let v: Vec<Option<String>> = vec![Some("self".into()), None, Some("abcd".into())];
    let mut out: Vec<(String, u32)> = vec![];
    for (name, k) in v.into_iter().zip([1u32, 2, 3]) {
        let Some(name) = name else { continue };
        out.push((name, k));
    }
    println!("{out:?}");
    // 一時値のタプル / struct からフィールドをムーブ
    let m = table().0;
    let s = table().1;
    let w = table().2;
    println!("{:?} {s:?} {w:?}", m.get(&1));
    // Deref / DerefMut 越しのフィールド
    let mut o = Outer { inner: Inner { a: 1, names: vec![] }, extra: 10 };
    println!("{} {} {}", o.bump("x"), o.bump("y"), o.a);
    // Option::iter / into_iter
    let a = Some(3u32);
    let b: Option<u32> = None;
    let xs: Vec<u32> = a.iter().chain(b.iter()).copied().chain(Some(9u32)).collect();
    println!("{xs:?} {}", b.iter().count());
    // クロージャで split
    let line = String::from("a1b22c,d");
    let parts: Vec<&str> = line.split(|c: char| c.is_ascii_digit() || c == ',').collect();
    println!("{parts:?}");
    // `&&str` の注釈
    let words = vec!["apple", "bob", "kayak"];
    let pal: Vec<&&str> = words.iter().filter(|w| w.chars().eq(w.chars().rev())).collect();
    println!("{pal:?}");
}
