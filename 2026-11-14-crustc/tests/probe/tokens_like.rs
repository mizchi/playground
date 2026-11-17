// macros.rs のセルフホストで足したもの: struct と同じ名前のバリアント、&struct の ==、
// PartialEq<str> と PartialEq の両方の impl、Option::flatten、HashMap の clone、
// String::replace_range / into_bytes、大文字の名前のパターンを含むクロージャ
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Ident { name: String }
#[derive(Clone, Debug)]
pub struct Punct { ch: char }
#[derive(Clone, Debug)]
pub enum Tree { Ident(Ident), Punct(Punct), None }

impl PartialEq for Ident {
    fn eq(&self, o: &Ident) -> bool { self.name == o.name }
}
impl PartialEq<str> for Ident {
    fn eq(&self, o: &str) -> bool { self.name == o }
}

#[derive(Clone, Debug, PartialEq)]
struct P { x: i32, y: i32 }

fn main() {
    let ts = vec![Tree::Ident(Ident { name: "macro_rules".into() }), Tree::Punct(Punct { ch: '!' }), Tree::None];
    for t in &ts {
        match t {
            Tree::Ident(i) if i == "macro_rules" => println!("kw {}", i.name),
            Tree::Punct(p) => println!("punct {}", p.ch),
            _ => println!("other"),
        }
    }
    let (a, b) = (Ident { name: "x".into() }, Ident { name: "x".into() });
    let (ra, rb) = (&a, &b);
    let (p1, p2) = (P { x: 1, y: 2 }, P { x: 1, y: 3 });
    println!("{} {} {}", ra == rb, &p1 == &p2, &&p1 == &&p1.clone());
    let o: Option<Option<i32>> = Some(Some(3));
    let n: Option<Option<i32>> = Some(None);
    println!("{:?} {:?}", o.flatten(), n.flatten());
    let mut m: HashMap<String, Vec<i32>> = HashMap::new();
    m.insert("a".into(), vec![1]);
    let mut c = m.clone();
    c.get_mut("a").unwrap().push(2);
    c.insert("b".into(), vec![]);
    println!("{:?} {} {:?}", m.get("a"), m.len(), c.get("a"));
    let mut s = String::from("hello world");
    s.replace_range(0..5, "goodbye");
    println!("{s} {:?}", s.clone().into_bytes().len());
    let names = vec!["x", "y"];
    let f = |v: Option<&str>| match v {
        None => names.len(),
        Some(k) => names.iter().position(|n| *n == k).unwrap_or(99),
    };
    println!("{} {} {}", f(None), f(Some("y")), f(Some("z")));
}
