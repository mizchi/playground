// セルフホスト第 3 段階で足した std: rsplit / rsplit_once / match_indices / クロージャの starts_with、
// &str の要素の contains、参照の列の flatten、OnceLock、const のパターン、matches! の中のメソッドの連鎖、
// `||` の右辺の一時値
use std::collections::HashMap;
use std::sync::OnceLock;

const KW: &str = "fn";
const NAMES: &[&str] = &["next", "map", "filter"];

fn table() -> &'static HashMap<&'static str, Vec<&'static str>> {
    static T: OnceLock<HashMap<&'static str, Vec<&'static str>>> = OnceLock::new();
    T.get_or_init(|| {
        println!("init");
        let mut m = HashMap::new();
        m.insert("a", vec!["x", "y"]);
        m.insert("b", vec!["z"]);
        m
    })
}

fn kind(w: &str) -> &'static str {
    match w {
        KW => "keyword",
        _ if w.starts_with(|c: char| c.is_ascii_digit() || c == '-') => "number",
        _ if w.ends_with(|c: char| c == '!') => "macro",
        _ => "ident",
    }
}

fn main() {
    let p = "crate::front::Program";
    println!("{:?} {:?}", p.rsplit("::").next(), p.rsplit("::").collect::<Vec<_>>());
    println!("{:?} {:?}", p.rsplit_once("::"), "k:v:w".rsplit(':').collect::<Vec<_>>());
    let src = "a.split(x).split('y').b.split(";
    let n = src.match_indices(".split(").filter(|(i, m)| src.as_bytes().get(i + m.len()) != Some(&b'\'')).count();
    println!("{n} {:?}", "aaaa".match_indices("aa").collect::<Vec<_>>());
    for w in ["fn", "12", "-3", "vec!", "x", ""] {
        println!("{w:?} {}", kind(w));
    }
    let w = "map";
    println!("{} {}", NAMES.contains(&w), NAMES.contains(&"zip"));
    let v = vec![String::from("a"), String::from("b")];
    println!("{}", v.contains(&"b".to_string()));
    let keys = ["a", "c", "b"];
    let mut found: Vec<&str> = keys.iter().filter_map(|k| table().get(k)).flatten().copied().collect();
    found.sort();
    println!("{:?} {}", found, table().len());
    let vv = vec![vec![1, 2], vec![], vec![3]];
    println!("{:?}", vv.iter().flatten().collect::<Vec<_>>());
    let name = "__exr_powf";
    println!("{}", matches!(name.trim_start_matches("__exr_").trim_end_matches('f'), "pow" | "exp"));
    for s in ["ab", "xyz"] {
        let hit = s.len() == 2 || src.contains(&format!(".{s}("));
        println!("{s} {hit}");
    }
}
