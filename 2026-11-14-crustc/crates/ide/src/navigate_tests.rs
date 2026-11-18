//! navigate の小さな部品の単体テスト (変異テストで見つからなかった変異を潰すためのもの)

use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// src の中の pat の nth 番目の位置
fn at(src: &str, pat: &str, nth: usize) -> usize {
    src.match_indices(pat).nth(nth).unwrap_or_else(|| panic!("no {pat:?} #{nth}")).0
}

/// 一つのファイルで、off の定義の名前の始め
fn def1(f: &FileIndex, off: usize) -> Option<usize> {
    definition(&[f], 0, off).first().map(|l| l.start)
}

fn no(_: usize) -> Option<Rc<FileIndex>> {
    None
}

#[test]
fn captures_in_literals() {
    let lit = "\"{a} {{b}} {c:>4} {0} {} {_y} {1x} {z";
    assert_eq!(captures(lit), vec![(2, "a"), (12, "c"), (26, "_y")]);
    assert_eq!(captures("\"{"), vec![]);
    assert_eq!(captures("\"x{{"), vec![]);
    assert_eq!(captures("{q}"), vec![(1, "q")]);
    assert_eq!(captures("{{q}}{r}"), vec![(6, "r")]);
}

#[test]
fn generic_names_of_items() {
    let t = "struct Foo<'a, T: Fn(u8) -> U, const N: usize, U = Vec<u8>> { x: T }";
    assert_eq!(generic_names(t, 10), s(&["T", "N", "U"]));
    assert_eq!(generic_names("struct A { x: u8 }", 8), Vec::<String>::new());
    assert_eq!(generic_names("enum E <K>", 6), s(&["K"]));
    // (`>` の後は見ない)
    assert_eq!(generic_names("struct B<X> where Y: Z<W> {}", 8), s(&["X"]));
    assert_eq!(generic_names("struct C<X: Iterator<Item = u8>, Y>;", 8), s(&["X", "Y"]));
    assert_eq!(generic_names("x", 5), Vec::<String>::new());
}

#[test]
fn type_strings() {
    assert_eq!(targs("HashMap<K, Vec<(A, B)>>"), s(&["K", "Vec<(A, B)>"]));
    assert_eq!(targs("&'a mut [T; 4]"), s(&["T"]));
    assert_eq!(targs("[u8]"), s(&["u8"]));
    assert_eq!(targs("Fn(u8) -> Vec<T>"), Vec::<String>::new());
    assert_eq!(targs("Box<dyn Fn(u8) -> u8>"), s(&["dyn Fn(u8) -> u8"]));
    assert_eq!(targs("Foo<'a, T>"), s(&["T"]));
    assert_eq!(targs("R<[u8; 2], (u8, u8)>"), s(&["[u8; 2]", "(u8, u8)"]));
    assert_eq!(targs("P<F, G> where"), s(&["F", "G"]));
    assert_eq!(targs("P<A<B>, C>, D>"), s(&["A<B>", "C"]));
    assert_eq!(targs("Q<impl Fn() -> u8, V>"), s(&["impl Fn() -> u8", "V"]));
    assert_eq!(targs("u8"), Vec::<String>::new());

    assert_eq!(subst("Vec<T>, a::T, TT, T", &s(&["T"]), &s(&["u8"])), "Vec<u8>, a::T, TT, u8");
    assert_eq!(subst("(K, V) é", &s(&["K", "V"]), &s(&["u8"])), "(u8, V) é");
    assert_eq!(subst("T", &[], &[]), "T");

    assert_eq!(alias_parts("<K, V: Eq = X> = HashMap<K, V>"), (s(&["K", "V"]), "HashMap<K, V>".to_string()));
    assert_eq!(alias_parts(" Vec<u8> "), (vec![], "Vec<u8>".to_string()));
    assert_eq!(alias_parts("<F: Fn() -> u8> = Box<F>"), (s(&["F"]), "Box<F>".to_string()));
    assert_eq!(alias_parts("<A<B>, C> = D"), (s(&["A<B>", "C"]), "D".to_string()));

    assert_eq!(ret_type("fn f(a: impl Fn(u8) -> u8) -> Vec<u8> where T: X"), "Vec<u8>");
    assert_eq!(ret_type("fn g()"), "");
    assert_eq!(ret_type("fn h<T: Fn() -> u8>(x: [u8; 2]) -> T"), "T");
    assert_eq!(ret_type("fn k(x: [fn() -> u8; 2]) -> u16"), "u16");
    assert_eq!(ret_type("fn m()->u8"), "u8");
    assert_eq!(ret_type("->"), "");

    assert_eq!(elem_type("Chars<'a>").as_deref(), Some("char"));
    assert_eq!(elem_type("Bytes").as_deref(), Some("u8"));
    assert_eq!(elem_type("CharIndices").as_deref(), Some("(usize, char)"));
    assert_eq!(elem_type("&Vec<u16>").as_deref(), Some("u16"));
    assert_eq!(elem_type("Lines<'a>").as_deref(), Some("&str"));
    assert_eq!(elem_type("Split<'a, char>").as_deref(), Some("&str"));
    assert_eq!(elem_type("SplitWhitespace").as_deref(), Some("&str"));
    assert_eq!(elem_type("Option<P>").as_deref(), Some("P"));
    assert_eq!(elem_type("HashMap<K, V>"), None);

    assert_eq!(index_type("String", false).as_deref(), Some("str"));
    assert_eq!(index_type("&str", true).as_deref(), Some("str"));
    assert_eq!(index_type("HashMap<K, V>", false).as_deref(), Some("V"));
    assert_eq!(index_type("Vec<u8>", true).as_deref(), Some("[u8]"));
    assert_eq!(index_type("Vec<u8>", false).as_deref(), Some("u8"));
    assert_eq!(index_type("[P]", false).as_deref(), Some("P"));
    assert_eq!(index_type("VecDeque<Q>", false).as_deref(), Some("Q"));
    assert_eq!(index_type("SmallVec<R>", false).as_deref(), Some("R"));
    assert_eq!(index_type("Vec", false), None);
    assert_eq!(index_type("Foo<u8>", false), None);

    assert_eq!(type_head("&'a mut a::Vec<T>"), "Vec");
    assert_eq!(type_head("impl Iterator<Item = u8>"), "Iterator");
    assert_eq!(type_head("[T; 3]"), "[T]");
    assert_eq!(type_head("(u8, u8)"), "");
    assert_eq!(strip_ref("&'a dyn const X"), "X");
    assert_eq!(impl_self("impl<T> Tr for &mut a::S<T>"), "S");
    assert_eq!(impl_self("impl S"), "S");
    assert_eq!(impl_self_full("impl Tr for S<T>"), "S<T>");
    assert_eq!(impl_self_full("impl S<T> "), "S<T>");
}

#[test]
fn hashes_and_modules() {
    // (FNV-1a 64)
    assert_eq!(name_hash(""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(name_hash("a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(name_hash("foobar"), 0x85944171f73967e8);

    assert_eq!(mod_score(&s(&["a", "b"]), &s(&["a", "b"]), true), Some(0));
    assert_eq!(mod_score(&s(&["a", "b"]), &s(&["b"]), true), None);
    assert_eq!(mod_score(&s(&["x", "a", "b", "c"]), &s(&["b", "c"]), false), Some(3));
    assert_eq!(mod_score(&s(&["a", "x", "b", "y"]), &s(&["a", "b"]), false), Some(14));
    assert_eq!(mod_score(&s(&["b", "a"]), &s(&["a", "b"]), false), None);
    assert_eq!(mod_score(&s(&["a"]), &s(&["a", "b"]), false), None);

    let here = s(&["m", "n"]);
    assert_eq!(use_module(&here, &s(&["crate", "a"])), (s(&["a"]), true));
    assert_eq!(use_module(&here, &s(&["self", "a"])), (s(&["m", "n", "a"]), true));
    assert_eq!(use_module(&here, &s(&["super", "a"])), (s(&["m", "a"]), true));
    assert_eq!(use_module(&here, &s(&["super", "super", "a"])), (s(&["a"]), true));
    assert_eq!(use_module(&here, &s(&["std", "sync"])), (s(&["sync"]), false));
    assert_eq!(use_module(&here, &[]), (vec![], true));
}

#[test]
fn lib_rank_order() {
    let src = "pub mod sys { pub fn f() {} }\npub mod a { pub mod b { pub fn f() {} } }\npub mod os { pub fn f() {} }\npub mod sys_x { pub fn f() {} }\npub mod n { pub mod nonpoison { pub fn f() {} } pub mod unix { pub fn f() {} } pub mod windows { pub fn f() {} } pub mod wasi { pub fn f() {} } pub mod ok { pub fn f() {} } }\npub fn f() {}\n";
    let f = FileIndex::new(src.to_string(), vec![]);
    let r: Vec<(String, usize)> = f.defs.iter().filter(|d| d.name == "f").map(|d| (d.module.join("::"), lib_rank(d))).collect();
    assert_eq!(
        r,
        vec![
            ("sys".to_string(), 101),
            ("a::b".to_string(), 2),
            ("os".to_string(), 101),
            ("sys_x".to_string(), 101),
            ("n::nonpoison".to_string(), 102),
            ("n::unix".to_string(), 102),
            ("n::windows".to_string(), 102),
            ("n::wasi".to_string(), 102),
            ("n::ok".to_string(), 2),
            ("".to_string(), 0),
        ]
    );
}

#[test]
fn cfg_predicates() {
    let src = "#[cfg(any(feature = \"a\", feature = \"b\"))]\nfn any_ab() {}\n\
               #[cfg(any(feature = \"b\", feature = \"c\"))]\nfn any_bc() {}\n\
               #[cfg(all(feature = \"a\", not(feature = \"b\")))]\nfn all_a_not_b() {}\n\
               #[cfg(all(feature = \"a\", feature = \"b\"))]\nfn all_ab() {}\n\
               #[cfg(not(feature = \"a\"))]\nfn not_a() {}\n\
               #[cfg(not())]\nfn not_empty() {}\n\
               #[cfg(target_os = \"no_such_os\")]\nfn os_no() {}\n\
               #[cfg(target_os = \"linux\")]\nfn os_linux() {}\n\
               #[cfg(target_family = \"no_such\")]\nfn fam_no() {}\n\
               #[cfg(target_family = \"unix\")]\nfn fam_unix() {}\n\
               #[cfg(unix)]\nfn unix_() {}\n\
               #[cfg(windows)]\nfn windows_() {}\n\
               #[cfg(miri)]\nfn miri_() {}\n\
               #[cfg(whatever)]\nfn other() {}\n\
               #[cfg(foo = \"x\")]\nfn other_kv() {}\n\
               #[cfg(weird(feature = \"z\"))]\nfn other_fn() {}\n\
               #[derive(Debug)]\n#[cfg(feature = \"z\")]\nstruct Gone { x: u8 }\n\
               #[cfg(feature = \"a\")]\nmod m { fn inner() {} }\n";
    let mut f = FileIndex::new(src.to_string(), vec![]);
    f.apply_cfg(s(&["a"]));
    let names: Vec<&str> = f.defs.iter().filter(|d| d.kind != SymKind::Field).map(|d| d.name.as_str()).collect();
    let mut want = vec!["any_ab", "all_a_not_b"];
    if cfg!(target_os = "linux") {
        want.push("os_linux");
    }
    if cfg!(unix) {
        want.extend(["fam_unix", "unix_"]);
    }
    if cfg!(windows) {
        want.push("windows_");
    }
    // (設定されていない cfg `whatever` / `foo = "x"` は成り立たない: rustc と同じ)
    want.extend(["other_fn", "m", "inner"]);
    assert_eq!(names, want);
    assert!(f.derives.is_empty());
    assert!(f.in_inactive(at(src, "any_bc", 0)));
    assert!(!f.in_inactive(at(src, "any_ab", 0)));
    // (feature なし)
    let mut g = FileIndex::new(src.to_string(), vec![]);
    g.apply_cfg(vec![]);
    assert!(g.defs.iter().any(|d| d.name == "not_a"));
    assert!(!g.defs.iter().any(|d| d.name == "inner"));
}

#[test]
fn macro_bodies_and_names() {
    let src = "macro_rules! m { ($x:expr) => {{ $x }}; }\nmacro_rules! n ( () => [] );\nfn f() { macro_rules! k { () => () } }\nmacro_rules! bad x\nmacro_rules! o { (";
    let f = FileIndex::new(src.to_string(), vec![]);
    let m0 = at(src, "{ ($x", 0);
    let m1 = at(src, "( () =>", 0);
    let m2 = at(src, "{ () =>", 0);
    let m3 = at(src, "{ (", 2);
    assert_eq!(f.macros, vec![(m0, at(src, "}\nmacro_rules! n", 0) + 1), (m1, at(src, ");\nfn", 0) + 1), (m2, at(src, ") } }", 0) + 3), (m3, src.len())]);
    assert!(f.in_macro_body(m0 + 1));
    assert!(!f.in_macro_body(m0 - 2));
    assert!(f.may_have("expr"));
    assert!(!f.may_have("nope"));
    let mut g = FileIndex::new(src.to_string(), vec![]);
    g.make_slim();
    assert!(g.may_have("expr"));
    assert!(!g.may_have("nope"));
}

#[test]
fn use_trees() {
    let src = "use a::b::{C, D as E, f::*, g::{h, i as j}};\npub use self::k;\nuse super::super::l as _m;\nfn x() {}\nuse ::q::R;\nuse s::{self, T};\n";
    let f = FileIndex::new(src.to_string(), vec![]);
    let u: Vec<(String, String)> = f.uses.iter().map(|(a, b)| (a.clone(), b.join("::"))).collect();
    let want: Vec<(String, String)> = [
        ("C", "a::b::C"),
        ("E", "a::b::D"),
        ("*", "a::b::f"),
        ("h", "a::b::g::h"),
        ("j", "a::b::g::i"),
        ("k", "self::k"),
        ("_m", "super::super::l"),
        ("R", "q::R"),
        ("self", "s::self"),
        ("T", "s::T"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(u, want);
    assert_eq!(f.use_at[0], at(src, "C,", 0));
    assert_eq!(f.use_at[1], at(src, "D as", 0));
    assert_eq!(f.use_at[2], usize::MAX);
    assert_eq!(f.use_at[4], at(src, "i as", 0));
    assert_eq!(f.use_at[6], at(src, "l as", 0));
    let p: Vec<String> = f.collect_uses(true).into_iter().map(|x| x.0).collect();
    assert_eq!(p, s(&["k"]));
    assert_eq!(f.use_alias("E"), Some("D"));
    assert_eq!(f.use_alias("C"), None);
    assert_eq!(f.use_alias("j"), Some("i"));
}

#[test]
fn locals_and_patterns() {
    let src = "struct S { f: u8 }
fn f<T: Clone>(a: u8, mut b: (u8, u8), c: &[u8], (d1, d2): (u8, u8)) -> u8 {
    let (x, y) = b;
    let z: u8 = a;
    let w = x + 1;
    let s = S { f: a };
    let cl = |p: u8, q| p + q;
    for (i, e) in c.iter().enumerate() { let _ = i + e; }
    if let Some(v) = Some(a) { v } else { w };
    while let Some(k) = None { k; }
    let r = match s { S { f: g } if g > 0 => g, S { f } => f };
    let t = match b { (m, 0) | (0, m) => m, n => n.0 };
    { let inner = 1; }
    let inner = 2;
    let mv = move |o| o + d1 + d2;
    cl(z, y) + inner + r + t + mv(1) + w
}
";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let f = &fi;
    let chk = |use_pat: &str, un: usize, def_pat: &str, dn: usize| {
        assert_eq!(def1(f, at(src, use_pat, un)), Some(at(src, def_pat, dn)), "{use_pat} #{un}");
    };
    chk("b;", 0, "b: (u8", 0);
    chk("a;", 0, "a: u8", 0);
    chk("x + 1", 0, "x, y", 0);
    chk("a }", 0, "a: u8", 0);
    chk("p + q", 0, "p: u8", 0);
    chk("q;", 0, "q|", 0);
    chk("i + e", 0, "i, e", 0);
    chk("e; }", 0, "e) in", 0);
    chk("c.iter", 0, "c: &", 0);
    chk("v }", 0, "v) =", 0);
    chk("w };", 0, "w = x", 0);
    chk("k; }", 0, "k) =", 0);
    chk("s {", 0, "s = S", 0);
    chk("g > 0", 0, "g }", 0);
    chk("g, S", 0, "g }", 0);
    chk("f };", 0, "f } =>", 0);
    chk("m, n", 0, "m, 0", 0);
    chk("n.0", 0, "n =>", 0);
    chk("b {", 0, "b: (u8", 0);
    chk("o + d1", 0, "o|", 0);
    chk("d1 + d2", 0, "d1, d2", 0);
    chk("d2;", 0, "d2)", 0);
    chk("z, y", 0, "z: u8", 0);
    chk("y)", 0, "y) =", 0);
    chk("inner + r", 0, "inner = 2", 0);
    chk("r + t", 0, "r = match", 0);
    chk("t + mv", 0, "t = match", 0);
    chk("mv(1)", 0, "mv =", 0);
    chk("w\n}", 0, "w = x", 0);
    chk("cl(z", 0, "cl =", 0);
    // (束縛の名前そのもの: 自分に)
    chk("g }", 0, "g }", 0);
    chk("m, 0", 0, "m, 0", 0);
    chk("x, y", 0, "x, y", 0);
    // フィールドの名前 `S { f: g }` の f はフィールド
    assert_eq!(def1(f, at(src, "f: g", 0)), Some(at(src, "f: u8", 0)));
    // T は型引数
    assert_eq!(def1(f, at(src, "T: Clone", 0)), Some(at(src, "T: Clone", 0)));
    // 参照の数 (定義を含む)
    let refs = |p: &str, n: usize| references(&[f], 0, at(src, p, n), true, &no).len();
    assert_eq!(refs("w = x", 0), 3);
    assert_eq!(refs("inner = 1", 0), 1);
    assert_eq!(refs("inner = 2", 0), 2);
    assert_eq!(refs("m, 0", 0), 2);
    assert_eq!(refs("g }", 0), 3);
    assert_eq!(refs("a: u8", 0), 4);
}

#[test]
fn binding_kinds() {
    let src = "fn f(a: u8, mut b: u8) {
    let (x, ref y, mut z) = (1, 2, 3);
    let q = |p: u8, r| p;
    for (i, j) in it {}
    if let Some(v) = o {}
    let w = foo(x);
    let e = a.b;
    let k = A::c;
    let _u = 1;
    match o { Some(m) => m, P { f, g: h } => h, Q(n) if n > 0 => n, _ => 0 };
    match o { s => s.x, _ => t };
    let g = |a| |b| a;
    let st = S { x: 1 };
}
";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let tk = |p: &str, n: usize| fi.tok_at(at(src, p, n)).unwrap();
    for (p, n) in [("x, ref", 0), ("y, mut", 0), ("z)", 0), ("i, j", 0), ("j)", 0), ("v)", 0), ("w =", 0), ("_u", 0), ("r|", 0), ("p: u8", 0)] {
        assert!(fi.pat_binding(tk(p, n)), "pat {p}");
    }
    for (p, n) in [("foo(", 0), ("a.b", 0), ("b;", 0), ("c;", 0), ("o {}", 0), ("u8, r", 0), ("it", 0), ("p;", 0), ("x);", 0), ("mut b", 0), ("ref y", 0), ("x: 1", 0)] {
        assert!(!fi.pat_binding(tk(p, n)), "not pat {p}");
    }
    for (p, n) in [("a: u8", 0), ("b: u8", 0)] {
        assert!(fi.fn_param(tk(p, n)), "param {p}");
    }
    for (p, n) in [("p: u8", 0), ("x: 1", 0), ("g: h", 0)] {
        assert!(!fi.fn_param(tk(p, n)), "not param {p}");
    }
    for (p, n) in [("m)", 0), ("f, g", 0), ("h }", 0), ("n) if", 0), ("s =>", 0)] {
        assert!(fi.arm_binding(tk(p, n)), "arm {p}");
    }
    for (p, n) in [("m,", 0), ("g: h", 0), ("n > 0", 0), ("n,", 0), ("s.x", 0), ("x, _", 0), ("t }", 0), ("o {", 1), ("x, ref", 0), ("w =", 0), ("foo", 0), ("st", 0)] {
        assert!(!fi.arm_binding(tk(p, n)), "not arm {p}");
    }
}

#[test]
fn local_types_and_methods() {
    let src = "mod a { pub mod b { pub struct P; impl P { pub fn get(&self) -> u8 { 0 } } } }
use a::b::P;
struct T(u8);
impl T { fn tm(&self) {} }
struct U { x: u8 }
impl U { fn um(&self) {} fn new() -> U { U { x: 0 } } }
fn f(p: &P, mut q: &mut a::b::P) {
    p.get();
    q.get();
    let r: &'static P = &P;
    r.get();
    let t = T(1);
    t.tm();
    let u = U { x: 1 };
    u.um();
    let v = a::b::P;
    v.get();
    let w = U::new();
    w.um();
    let y = mut_x;
    let z = &mut T(2);
    z.tm();
}
";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let f = &fi;
    let get = at(src, "get(&self)", 0);
    for n in 0..4 {
        assert_eq!(def1(f, at(src, "get();", n)), Some(get), "get #{n}");
    }
    assert_eq!(def1(f, at(src, "tm();", 0)), Some(at(src, "tm(&self)", 0)));
    assert_eq!(def1(f, at(src, "tm();", 1)), Some(at(src, "tm(&self)", 0)));
    assert_eq!(def1(f, at(src, "um();", 0)), Some(at(src, "um(&self)", 0)));
    assert_eq!(def1(f, at(src, "um();", 1)), Some(at(src, "um(&self)", 0)));
    let tk = |p: &str| fi.tok_at(at(src, p, 0)).unwrap();
    assert_eq!(fi.local_type(tk("p: &P")).as_deref(), Some("P"));
    assert_eq!(fi.local_type(tk("q: &mut")).as_deref(), Some("P"));
    assert_eq!(fi.local_type(tk("r: &")).as_deref(), Some("P"));
    assert_eq!(fi.local_type(tk("t = T")).as_deref(), Some("T"));
    assert_eq!(fi.local_type(tk("u = U")).as_deref(), Some("U"));
    assert_eq!(fi.local_type(tk("v = a")).as_deref(), Some("P"));
    assert_eq!(fi.local_type(tk("w = U")).as_deref(), Some("U"));
    assert_eq!(fi.local_type(tk("y = ")), None);
    assert_eq!(fi.local_type(tk("z = ")).as_deref(), Some("T"));
    assert_eq!(fi.local_type(tk("get();")), None);
}

#[test]
fn derived_methods() {
    let src = "#[derive(Clone, Default, PartialEq, Debug, PartialOrd, Ord, Hash, Eq, serde::Serialize)]
pub struct D;
#[derive(Copy)]
#[allow(x)]
pub enum K { A }
fn t(d: D, h: &mut H) {
    d.clone(); d.clone_from(&d); D::default(); d.eq(&d); d.ne(&d); d.fmt(h);
    d.partial_cmp(&d); d.lt(&d); d.le(&d); d.gt(&d); d.ge(&d);
    d.cmp(&d); d.max(d); d.min(d); d.clamp(d, d); d.hash(h); d.other();
}
";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let f = &fi;
    let dv = |tr: &str| (0..src.len()).find(|&i| src[i..].starts_with(tr) && matches!(&src[i - 1..i], "(" | " ") && matches!(&src[i + tr.len()..i + tr.len() + 1], "," | ")")).unwrap();
    for (m, tr) in [
        (".clone()", "Clone"),
        (".clone_from", "Clone"),
        (":default()", "Default"),
        (".eq(", "PartialEq"),
        (".ne(", "PartialEq"),
        (".fmt(", "Debug"),
        (".partial_cmp(", "PartialOrd"),
        (".lt(", "PartialOrd"),
        (".le(", "PartialOrd"),
        (".gt(", "PartialOrd"),
        (".ge(", "PartialOrd"),
        (".cmp(", "Ord"),
        (".max(", "Ord"),
        (".min(", "Ord"),
        (".clamp(", "Ord"),
        (".hash(", "Hash"),
    ] {
        let l = definition(&[f], 0, at(src, m, 0) + 1);
        assert_eq!(l, vec![Loc { file: 0, start: dv(tr), end: dv(tr) + tr.len() }], "{m}");
    }
    assert!(definition(&[f], 0, at(src, "other", 0)).is_empty());
    let dn: Vec<(&str, &str)> = fi.derives.iter().map(|(a, b, _)| (a.as_str(), b.as_str())).collect();
    assert_eq!(dn, vec![("D", "Clone"), ("D", "Default"), ("D", "PartialEq"), ("D", "Debug"), ("D", "PartialOrd"), ("D", "Ord"), ("D", "Hash"), ("D", "Eq"), ("D", "Serialize"), ("K", "Copy")]);
    assert!(fi.in_derive(fi.tok_at(at(src, "Hash", 0)).unwrap()));
    assert!(fi.in_derive(fi.tok_at(at(src, "Serialize", 0)).unwrap()));
    assert!(!fi.in_derive(fi.tok_at(at(src, "allow", 0)).unwrap()));
    assert!(!fi.in_derive(fi.tok_at(at(src, "x)", 0)).unwrap()));
    // (ライブラリのファイルの derive は見ない)
    let mut lib = FileIndex::new(src.to_string(), vec![]);
    lib.lib = true;
    assert_eq!(derive_method(&[&lib], 0, &s(&["D"]), "clone"), None);
    assert_eq!(derive_method(&[f], 0, &s(&["X"]), "clone"), None);
}

#[test]
fn format_captures() {
    let src = "fn f(x: u8) {\n    let y = 2;\n    let s = format!(\"{x} {y:?} {{x}} {z}\");\n    println!(\"{x}\", );\n}\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let f = &fi;
    assert_eq!(def1(f, at(src, "x} {y", 0)), Some(at(src, "x: u8", 0)));
    assert_eq!(def1(f, at(src, "y:?", 0)), Some(at(src, "y = 2", 0)));
    assert_eq!(def1(f, at(src, "x}\",", 0)), Some(at(src, "x: u8", 0)));
    assert_eq!(def1(f, at(src, "z}", 0)), None);
    assert_eq!(def1(f, at(src, "x}}", 0)), None);
    assert_eq!(fi.capture_at(at(src, "format", 0)), None);
    let r = references(&[f], 0, at(src, "x: u8", 0), true, &no);
    assert_eq!(r.iter().map(|l| l.start).collect::<Vec<_>>(), vec![at(src, "x: u8", 0), at(src, "x} {y", 0), at(src, "x}\",", 0)]);
    assert_eq!(references(&[f], 0, at(src, "y = 2", 0), true, &no).len(), 2);
}

#[test]
fn generic_params() {
    let src = "fn f<'a, T: Clone, const N: usize>(t: T) -> [T; N] { todo!() }
struct W<A, B>(A, B);
impl<X: Fn() -> u8, Y> W<X, Y> { fn m(&self) -> X { todo!() } fn n<Z>(&self, z: Z) -> Y { todo!() } }
trait Tr<U> { fn tm(u: U); }
enum E<V> { A(V) }
type Al<Q> = Vec<Q>;
union Un<R> { r: R }
";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let f = &fi;
    let chk = |p: &str, n: usize, d: &str, dn: usize| assert_eq!(def1(f, at(src, p, n)), Some(at(src, d, dn)), "{p} #{n}");
    chk("T) ->", 0, "T: Clone", 0);
    chk("T; N", 0, "T: Clone", 0);
    chk("N]", 0, "N: usize", 0);
    chk("A, B)", 0, "A, B>", 0);
    chk("B)", 0, "B>", 0);
    chk("X, Y>", 0, "X: Fn", 0);
    chk("Y> {", 0, "Y> W", 0);
    chk("X {", 0, "X: Fn", 0);
    chk("Z) ->", 0, "Z>", 0);
    chk("Y { todo", 0, "Y> W", 0);
    chk("U);", 0, "U> {", 0);
    chk("V) }", 0, "V> {", 0);
    chk("Q>;", 0, "Q> =", 0);
    chk("R }", 0, "R> {", 0);
    assert_eq!(fi.generic_param(at(src, "todo", 0), "M"), None);
}

#[test]
fn module_scopes() {
    let src = "mod a { mod b { fn f() {} } fn g() {} }\nfn h() {}\n";
    let fi = FileIndex::new(src.to_string(), s(&["top"]));
    assert_eq!(fi.module_at(at(src, "f()", 0)), s(&["top", "a", "b"]));
    assert_eq!(fi.module_at(at(src, "g()", 0)), s(&["top", "a"]));
    assert_eq!(fi.module_at(at(src, "h()", 0)), s(&["top"]));
    assert_eq!(fi.enclosing_fn(at(src, "{} }", 0) + 1).map(|d| d.name.as_str()), Some("f"));
    assert_eq!(fi.enclosing_fn(at(src, "{}\n", 0) + 1).map(|d| d.name.as_str()), Some("h"));
    assert!(fi.enclosing_fn(0).is_none());
    // tok_at: 識別子の両端と、その外
    assert_eq!(fi.tok_at(at(src, "h()", 0)).map(|k| fi.tok(k)), Some("h"));
    assert_eq!(fi.tok_at(at(src, "h()", 0) + 1).map(|k| fi.tok(k)), Some("h"));
    assert_eq!(fi.tok_at(at(src, "()", 2) + 1), None);
}


#[test]
fn guards_and_arm_edges() {
    let src = "fn f(o: Option<u8>) -> u8 {
    match o { Some(x) if g(x, 1) => x, _ => 0 };
    match o { Some(y) if h([y, 2]) => y, _ => 0 };
    match o { m::V(z) => z, k!(w) => w, _ => 0 };
    if o.is_some() { 1 } else { 2 }
}
";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let f = &fi;
    assert_eq!(def1(f, at(src, "x, 1", 0)), Some(at(src, "x) if", 0)));
    assert_eq!(def1(f, at(src, "y, 2", 0)), Some(at(src, "y) if", 0)));
    assert_eq!(def1(f, at(src, "x, _", 0)), Some(at(src, "x) if", 0)));
    assert_eq!(def1(f, at(src, "z, k", 0)), Some(at(src, "z) =>", 0)));
    let tk = |p: &str| fi.tok_at(at(src, p, 0)).unwrap();
    assert!(!fi.arm_binding(tk("m::V")));
    assert!(!fi.arm_binding(tk("k!")));
    assert!(fi.arm_binding(tk("w) =>")));
    assert_eq!(fi.guard_arrow(tk("if o.")), None);
    assert!(fi.guard_arrow(tk("if g")).is_some());
    let p = |s: &str| fi.toks.iter().position(|t| t.1 as usize == at(src, s, 0)).unwrap();
    assert_eq!(fi.guard_arrow(tk("if g")), Some(p("=> x,")));
    assert_eq!(fi.guard_arrow(tk("if h")), Some(p("=> y,")));
}

#[test]
fn derive_across_files() {
    let a = "#[derive(Clone)]\npub struct D;\n";
    let b = "fn t(d: D) { d.clone(); }\n";
    let fa = FileIndex::new(a.to_string(), vec![]);
    let mut fb = FileIndex::new(b.to_string(), vec![]);
    let at_clone = at(a, "Clone", 0);
    let want = Some(Loc { file: 0, start: at_clone, end: at_clone + 5 });
    // (src のファイル・同じクレートのファイルの derive は見える。ほかのクレートの tests の中のものは見えない)
    assert_eq!(derive_method(&[&fa, &fb], 1, &s(&["D"]), "clone"), want);
    let mut fa2 = FileIndex::new(a.to_string(), vec![]);
    fa2.in_src = false;
    fa2.krate = 3;
    fb.krate = 3;
    assert_eq!(derive_method(&[&fa2, &fb], 1, &s(&["D"]), "clone"), want);
    fb.krate = 4;
    assert_eq!(derive_method(&[&fa2, &fb], 1, &s(&["D"]), "clone"), None);
    assert_eq!(derive_method(&[&fa2, &fb], 0, &s(&["D"]), "clone"), want);
}

#[test]
fn boundaries() {
    let src = "macro_rules! m { () => {} }\nfn f(x: u8) { let s = format!(\"{x}\"); }\nmod a { fn g() {} }\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let (a, b) = fi.macros[0];
    assert!(fi.in_macro_body(a));
    assert!(fi.in_macro_body(b - 1));
    assert!(!fi.in_macro_body(b));
    assert!(!fi.in_macro_body(at(src, "m {", 0)));
    assert!(fi.in_macro_body(a - 1));
    // 取り込み `{x}` の両端
    let xa = at(src, "x}", 0);
    assert!(fi.capture_at(xa).is_some());
    assert!(fi.capture_at(xa + 1).is_some());
    assert!(fi.capture_at(xa - 1).is_none());
    assert!(fi.capture_at(xa + 2).is_none());
    assert!(fi.capture_at(at(src, "\"{x", 0)).is_none());
    // mod の範囲の端
    let ma = fi.defs.iter().find(|d| d.name == "a").unwrap();
    assert_eq!(fi.module_at(ma.item_start), s(&["a"]));
    assert_eq!(fi.module_at(ma.item_end - 1), s(&["a"]));
    assert_eq!(fi.module_at(ma.item_end), Vec::<String>::new());
    let ff = fi.defs.iter().find(|d| d.name == "f").unwrap();
    assert_eq!(fi.enclosing_fn(ff.item_start).map(|d| d.name.as_str()), Some("f"));
    assert!(fi.enclosing_fn(ff.item_end).is_none());
    assert_eq!(fi.enclosing_fn(ff.item_end - 1).map(|d| d.name.as_str()), Some("f"));
    // tok_at: 識別子の終わりの位置もその識別子
    let g = at(src, "g()", 0);
    assert_eq!(fi.tok_at(g + 1).map(|k| fi.tok(k)), Some("g"));
    assert_eq!(fi.tok_at(g - 1).map(|k| fi.tok(k)), Some("fn"));
}

#[test]
fn nested_uses() {
    let src = "use a::{b::{c, d::{e}}, f};\npub(crate) use g::h;\n#[cfg(x)]\nuse i::j;\nfn k() { use l::n; }\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let u: Vec<String> = fi.uses.iter().map(|(a, b)| format!("{a}={}", b.join("::"))).collect();
    assert_eq!(u, s(&["c=a::b::c", "e=a::b::d::e", "f=a::f", "h=g::h", "j=i::j", "n=l::n"]));
}

/// 書き換えを当てた本文
fn apply(src: &str, mut e: Vec<(Loc, String)>) -> String {
    e.sort_by_key(|x| std::cmp::Reverse(x.0.start));
    let mut s = src.to_string();
    for (l, t) in e {
        s.replace_range(l.start..l.end, &t);
    }
    s
}

#[test]
fn rename_items_locals_and_shorthand() {
    let src = "#[derive(Clone)]\nstruct P { x: u8 }\nimpl P { fn get(&self) -> u8 { self.x } }\n\
               fn f(x: u8) -> P { let p = P { x }; let P { x } = p.clone(); println!(\"{x}\"); P { x: x + p.get() } }\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&fi];
    // 型
    let r = apply(src, rename(&ws, 0, at(src, "P {", 0), "Q", &no).unwrap());
    assert_eq!(r.matches("Q").count(), 6);
    assert!(!r.contains(" P"));
    // フィールド: 省略形は `y: x` に
    let r = apply(src, rename(&ws, 0, at(src, "x: u8 }", 0), "y", &no).unwrap());
    assert!(r.contains("struct P { y: u8 }"), "{r}");
    assert!(r.contains("self.y"), "{r}");
    assert!(r.contains("P { y: x }; let P { y: x } = "), "{r}");
    assert!(r.contains("P { y: x + p.get() }"), "{r}");
    // 引数 x: 省略形は `x: z` に、書式の中も
    let r = apply(src, rename(&ws, 0, at(src, "x: u8) ->", 0), "z", &no).unwrap());
    assert!(r.contains("fn f(z: u8)"), "{r}");
    assert!(r.contains("P { x: z };"), "{r}");
    // (let P { x } の x は別の束縛)
    assert!(r.contains("let P { x } = "), "{r}");
    // メソッド
    let r = apply(src, rename(&ws, 0, at(src, "get()", 0), "fetch", &no).unwrap());
    assert!(r.contains("fn fetch(&self)") && r.contains("p.fetch()"), "{r}");
    // 名前にできないもの・変えられないもの
    assert!(rename(&ws, 0, at(src, "x: u8)", 0), "fn", &no).is_err());
    assert!(rename(&ws, 0, at(src, "x: u8)", 0), "1a", &no).is_err());
    assert!(rename(&ws, 0, at(src, "x: u8)", 0), "r#fn", &no).is_ok());
    assert!(prepare_rename(&ws, 0, at(src, "self.x", 0)).is_err());
    assert!(prepare_rename(&ws, 0, at(src, "u8 }", 0)).is_err());
    assert!(prepare_rename(&ws, 0, at(src, "clone()", 0)).is_err());
    assert!(prepare_rename(&ws, 0, at(src, "println", 0)).is_err());
    assert_eq!(prepare_rename(&ws, 0, at(src, "get()", 0) + 1), Ok(Loc { file: 0, start: at(src, "get()", 0), end: at(src, "get()", 0) + 3 }));
}

#[test]
fn rename_refuses_library_and_file_modules() {
    let lib = FileIndex::library("pub struct Vec;\npub struct Mutex;\n".to_string(), s(&["sync"]));
    let m = FileIndex::new("pub fn g() {}\n".to_string(), s(&["m"]));
    let src = "mod m;\nuse std::sync::Mutex;\nfn f(v: Vec, x: Mutex) -> Mutex { m::g(); x }\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&lib, &m, &fi];
    assert!(prepare_rename(&ws, 2, at(src, "Vec", 0)).is_err());
    // (use したライブラリのもの: 別名を付ける)
    let r = apply(src, rename(&ws, 2, at(src, "Mutex;", 0), "M", &no).unwrap());
    assert_eq!(r, "mod m;\nuse std::sync::Mutex as M;\nfn f(v: Vec, x: M) -> M { m::g(); x }\n");
    assert!(prepare_rename(&ws, 2, at(src, "Mutex)", 0)).is_err());
    // (ワークスペースのものも、use の名前の上なら別名に)
    let src2 = "use crate::m::g;\nfn h() { g(); crate::m::g(); }\n";
    let f2 = FileIndex::new(src2.to_string(), vec![]);
    let ws2 = [&lib, &m, &f2];
    let r = apply(src2, rename(&ws2, 2, at(src2, "g;", 0), "k", &no).unwrap());
    assert_eq!(r, "use crate::m::g as k;\nfn h() { k(); crate::m::g(); }\n");
    assert!(prepare_rename(&ws, 2, at(src, "m;", 0)).is_err());
    let e = rename(&ws, 2, at(src, "g()", 0), "h", &no).unwrap();
    assert_eq!(e.iter().map(|x| x.0.file).collect::<Vec<_>>(), vec![1, 2]);
}

#[test]
fn signature_help_calls() {
    let src = "struct P;\nimpl P { fn m(&self, a: u8, b: impl Fn(u8) -> u8) -> u8 { 0 } fn new(x: u8) -> P { P } }\n\
               fn g<T: Into<u8>>(t: T, v: Vec<(u8, u8)>) {}\nstruct T2(u8, u16);\n\
               fn f(p: P) { p.m(1, |x| x); P::new(g::<u8>(1, vec![(1, 2)]), ); T2(1, 2); let y = [1, 2]; }\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&fi];
    let sh = |p: &str, d: usize| signature_help(&ws, 0, at(src, p, 0) + d, &no).map(|(s, ps, a, _)| (s.clone(), ps.iter().map(|&(x, y)| s[x..y].to_string()).collect::<Vec<_>>(), a));
    let (sig, ps, a) = sh("1, |x|", 0).unwrap();
    assert_eq!(sig, "fn m(&self, a: u8, b: impl Fn(u8) -> u8) -> u8");
    assert_eq!(ps, s(&["a: u8", "b: impl Fn(u8) -> u8"]));
    assert_eq!(a, 0);
    assert_eq!(sh("|x| x)", 0).unwrap().2, 1);
    // (入れ子の呼び出しの中は内側、閉じた括弧の後は外側)
    let (sig, ps, a) = sh("(1, 2)]", 0).unwrap();
    assert!(sig.starts_with("fn g<T: Into<u8>>"), "{sig}");
    assert_eq!(ps, s(&["t: T", "v: Vec<(u8, u8)>"]));
    assert_eq!(a, 1);
    assert_eq!(sh("1, vec", 0).unwrap().2, 0);
    let (sig, _, a) = sh(", );", 1).unwrap();
    assert_eq!((sig.as_str(), a), ("fn new(x: u8) -> P", 1));
    let (sig, ps, a) = sh("2); let", 0).unwrap();
    assert_eq!((sig.as_str(), ps, a), ("struct T2(u8, u16)", s(&["u8", "u16"]), 1));
    assert!(sh("2]; }", 0).is_none());
    assert!(sh("p: P) {", 0).is_none());
}

/// cfg で外れたアイテムの中: rust-analyzer と同じく、外側のスコープから見えるアイテムへのパスだけ解決する
#[test]
fn inactive_code_paths_only() {
    let src = "pub struct P { pub x: u8 }
pub fn used() {}
impl P { pub fn get(&self) -> u8 { self.x } }
pub mod sub { pub fn deep() {} }
use sub::deep;
#[cfg(feature = \"b\")]
fn inact(p: P) -> u8 {
    let loc = p.get();
    used(); deep(); sub::deep(); P::get(&p);
    let s = P { x: loc };
    loc + s.x
}
#[cfg(feature = \"b\")]
mod gone { fn used() {} fn k(_: &crate::P) { crate::used(); } }
#[cfg(feature = \"b\")]
fn caller() {}
pub fn caller() { used(); }
";
    let mut fi = FileIndex::new(src.to_string(), vec![]);
    fi.apply_cfg(vec![]);
    let f = &fi;
    let d = |p: &str, n: usize| def1(f, at(src, p, n));
    // パス: 外側のスコープで
    assert_eq!(d("used(); deep", 0), Some(at(src, "used()", 0)));
    assert_eq!(d("deep(); sub", 0), Some(at(src, "deep()", 0)));
    assert_eq!(d("deep(); P", 0), Some(at(src, "deep()", 0)));
    assert_eq!(d("sub::deep(); P", 0), Some(at(src, "sub {", 0)));
    assert_eq!(d("P) -> u8", 0), Some(at(src, "P {", 0)));
    assert_eq!(d("P { x: loc", 0), Some(at(src, "P {", 0)));
    assert_eq!(d("P::get", 0), Some(at(src, "P {", 0)));
    assert_eq!(d("used(); } }", 0), Some(at(src, "used()", 0)));
    // ローカル・メソッド・フィールド・関連アイテム・外れたアイテムの名前は答えない
    for (p, n) in [("loc =", 0), ("loc + ", 0), ("get();", 0), ("get(&p)", 0), ("x: loc", 0), ("s.x", 0), ("p.get", 0), ("inact", 0), ("caller() {}", 0), ("used() {} fn k", 0)] {
        assert_eq!(d(p, n), None, "{p}");
    }
    // 参照: used は定義・外れた inact の中・外れた gone の中・caller の中 (gone の中の `fn used` の名前は数えない)
    let r = references(&[f], 0, at(src, "used()", 0), true, &no);
    assert_eq!(r.iter().map(|l| l.start).collect::<Vec<_>>(), vec![at(src, "used()", 0), at(src, "used(); deep", 0), at(src, "used(); } }", 0), at(src, "used(); }\n", 0)]);
    // P: 定義・impl・外れた inact の中の 3 つ・gone の中の crate::P
    assert_eq!(references(&[f], 0, at(src, "P {", 0), true, &no).len(), 6);
    // フィールド x: 定義と self.x (外れた中の `x: loc` / `s.x` は数えない)
    assert_eq!(references(&[f], 0, at(src, "x: u8", 0), true, &no).len(), 2);
    // hover も答える
    assert!(hover(&[f], 0, at(src, "used(); deep", 0), &no).is_some_and(|h| h.contains("pub fn used()")));
}

/// trait の impl のメソッドの rename: ワークスペースの trait なら宣言と全部の impl、ライブラリの trait なら断る
#[test]
fn rename_trait_impl_methods() {
    let lib = FileIndex::library("pub trait Default { fn default() -> Self; }\n".to_string(), s(&["default"]));
    let src = "trait Shape { fn area(&self) -> u8; }\nstruct A;\nstruct B;\n\
               impl Shape for A { fn area(&self) -> u8 { 1 } }\nimpl Shape for B { fn area(&self) -> u8 { 2 } }\n\
               impl Default for A { fn default() -> Self { A } }\n\
               fn f(a: A, b: B) -> u8 { a.area() + b.area() }\nfn g() -> A { A::default() }\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&lib, &fi];
    // impl A の area から: trait の宣言・impl B の area・両方の呼び出しも
    let r = apply(src, rename(&ws, 1, at(src, "area(&self) -> u8 { 1", 0), "size", &no).unwrap());
    assert_eq!(r.matches("size").count(), 5, "{r}");
    assert!(!r.contains("area"), "{r}");
    // 呼び出しの上からも同じ
    let r2 = apply(src, rename(&ws, 1, at(src, "area() + b", 0), "size", &no).unwrap());
    assert_eq!(r, r2);
    // ライブラリの trait の impl のメソッドは断る (宣言の上でも、呼び出しの上でも)
    let e = prepare_rename(&ws, 1, at(src, "default() -> Self { A", 0)).unwrap_err();
    assert!(e.contains("Default"), "{e}");
    assert!(rename(&ws, 1, at(src, "default() }", 0), "make", &no).is_err());
}

#[test]
fn rename_edges() {
    let src = "struct P { x: u8, y: u8 }\nimpl P { fn new() -> Self { Self { x: 1, y: 2 } } }\nuse std::sync::Mutex as Mx;\nfn f(x: u8) -> P { let y = 2; P { x, y } }\n";
    let lib = FileIndex::library("pub struct Mutex;\n".to_string(), s(&["sync"]));
    let fi = FileIndex::new(src.to_string(), vec![]);
    {
        let ws = [&lib, &fi];
        // (キーワードの Self は型に解決されても変えない)
        assert!(prepare_rename(&ws, 1, at(src, "Self {", 0)).is_err());
        // (別名で取り込んだライブラリのもの: 別名の付いた use の上でも変えない)
        assert!(prepare_rename(&ws, 1, at(src, "Mutex as", 0)).is_err());
        assert!(rename(&ws, 1, at(src, "x: u8)", 0), "_", &no).is_err());
        // 省略形 `P { x, y }`: カンマの前の x も
        let r = apply(src, rename(&ws, 1, at(src, "x: u8)", 0), "z", &no).unwrap());
        assert!(r.ends_with("P { x: z, y } }\n"), "{r}");
        let r = apply(src, rename(&ws, 1, at(src, "y: u8 }", 0), "w", &no).unwrap());
        assert!(r.contains("Self { x: 1, w: 2 }") && r.ends_with("P { x, w: y } }\n"), "{r}");
    }
    // メモリから外したファイル (slim) の中の参照も、読み直して書き換える
    let other = "fn g() -> u8 { let p = P::new(); p.x }\n";
    let mut fo = FileIndex::new(other.to_string(), vec![]);
    fo.make_slim();

    let fresh = Rc::new(FileIndex::new(src.to_string(), vec![]));
    let fo2 = Rc::new(FileIndex::new(other.to_string(), vec![]));
    let full = FileIndex::new(src.to_string(), vec![]);
    let ws = [&lib, &full, &fo];
    let load = |i: usize| match i {
        1 => Some(fresh.clone()),
        2 => Some(fo2.clone()),
        _ => None,
    };
    let e = rename(&ws, 1, at(src, "x: u8,", 0), "xx", &load).unwrap();
    let in_other: Vec<&str> = e.iter().filter(|x| x.0.file == 2).map(|x| &other[x.0.start..x.0.end]).collect();
    assert_eq!(in_other, vec!["x"]);
    assert_eq!(e.iter().filter(|x| x.0.file == 1).count(), 3);
}

#[test]
fn signature_help_edges() {
    let src = "struct P;\nimpl P {\n    fn m(&self, a: u8) {}\n    fn n(mut self, a: u8) {}\n    fn o(self: &Self, a: u8) {}\n    fn q(self, a: u8) {}\n}\n\
               fn h<F: Fn(u8) -> u8>(f: F, g: u8) {}\nfn z() {}\nfn y(a: u8,) {}\nfn k(a: [u8; 3], b: u8) {}\n\
               fn t(p: P) { P::m(&p, 1); p.n(1); p.o(1); p.q(1); h(|x| x, 2); z(); y(1); k([0; 3], 1); h(|x| { x }, 1); }\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&fi];
    let sh = |p: &str, d: usize| signature_help(&ws, 0, at(src, p, 0) + d, &no).map(|(s, ps, a, _)| (ps.iter().map(|&(x, y)| s[x..y].to_string()).collect::<Vec<_>>(), a));
    // (パスで呼ぶ `P::m(&p, ..)` は self も引数)
    assert_eq!(sh("1); p.n", 0), Some((s(&["&self", "a: u8"]), 1)));
    assert_eq!(sh("1); p.o", 0), Some((s(&["a: u8"]), 0)));
    assert_eq!(sh("1); p.q", 0), Some((s(&["a: u8"]), 0)));
    assert_eq!(sh("1); h(", 0), Some((s(&["a: u8"]), 0)));
    assert_eq!(sh("2); z", 0), Some((s(&["f: F", "g: u8"]), 1)));
    assert_eq!(sh("); y(", 0), Some((vec![], 0)));
    assert_eq!(sh("1); k", 0), Some((s(&["a: u8"]), 0)));
    // (`[0; 3]` の `;` は文の終わりではない)
    assert_eq!(sh("1); h(|x| {", 0), Some((s(&["a: [u8; 3]", "b: u8"]), 1)));
    // (引数の中のブロックの中は答えない)
    assert_eq!(sh("x }, 1", 0), None);
}

#[test]
fn cfg_target_keys() {
    let src = format!(
        "#[cfg(target_arch = \"{a}\")]\nfn arch() {{}}\n#[cfg(target_arch = \"nope\")]\nfn arch_no() {{}}\n\
         #[cfg(target_pointer_width = \"{w}\")]\nfn width() {{}}\n#[cfg(target_pointer_width = \"7\")]\nfn width_no() {{}}\n\
         #[cfg(target_has_atomic = \"64\")]\nfn atomic() {{}}\n#[cfg(panic = \"unwind\")]\nfn unwind() {{}}\n#[cfg(panic = \"abort\")]\nfn abort() {{}}\n\
         #[cfg(not(no_global_oom_handling))]\nfn oom() {{}}\n#[cfg(test)]\nfn t() {{}}\n#[cfg(debug_assertions)]\nfn da() {{}}\n#[cfg(doc)]\nfn doc_() {{}}\n\
         #[cfg(target_endian = \"little\")]\nfn little() {{}}\n#[cfg(target_endian = \"big\")]\nfn big() {{}}\n",
        a = std::env::consts::ARCH,
        w = usize::BITS
    );
    let mut f = FileIndex::new(src.clone(), vec![]);
    f.apply_cfg(vec![]);
    let names: Vec<&str> = f.defs.iter().map(|d| d.name.as_str()).collect();
    let mut want = vec!["arch", "width", "atomic", "unwind", "oom", "t", "da"];
    want.push(if cfg!(target_endian = "little") { "little" } else { "big" });
    assert_eq!(names, want);
}

#[test]
fn signature_help_closures_and_docs() {
    let src = "/// 二倍にする\nfn dbl(x: u8) -> u8 { x * 2 }\nfn f() {\n    let h = |name: &str, n: u8| -> Option<u8> { None };\n    let g = move |a| dbl(a);\n    let k = || 1u8;\n    let v = 3;\n    h(\"a\", 1); g(2); k(); dbl(v);\n}\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&fi];
    let sh = |p: &str| signature_help(&ws, 0, at(src, p, 0), &no).map(|(s, ps, a, d)| (s.clone(), ps.iter().map(|&(x, y)| s[x..y].to_string()).collect::<Vec<_>>(), a, d));
    let (sig, ps, a, _) = sh("1); g").unwrap();
    assert_eq!(sig, "impl Fn(name: &str, n: u8) -> Option<u8>");
    assert_eq!((ps, a), (s(&["name: &str", "n: u8"]), 1));
    let (sig, ps, _, _) = sh("2); k").unwrap();
    assert_eq!((sig.as_str(), ps), ("impl Fn(a) -> u8", s(&["a"])));
    let (sig, ps, _, _) = sh("); dbl").unwrap();
    assert_eq!((sig.as_str(), ps), ("impl Fn() -> u8", vec![]));
    // doc コメントも
    let (sig, _, _, doc) = sh("v);").unwrap();
    assert_eq!((sig.as_str(), doc.as_str()), ("fn dbl(x: u8) -> u8", "二倍にする"));
    // (クロージャでないローカルの呼び出しには答えない)
    let src2 = "fn f(p: u8) { let q = p; q(1); }\n";
    let f2 = FileIndex::new(src2.to_string(), vec![]);
    assert!(signature_help(&[&f2], 0, at(src2, "1)", 0), &no).is_none());
}

#[test]
fn literal_types() {
    let src = "fn f(x: u8) {\n    let a = \"s\";\n    let b = 'c';\n    let c = b\"x\";\n    let d = b'x';\n    let e = 1u8;\n    let g = 1.5;\n    let h = 10;\n    let i = true;\n    let j = 2.0f32;\n    let k = 1_000usize;\n    let t = (1, 2);\n    let l = t.0;\n    let m = r\"raw\";\n    let n = x + 1u16;\n    let o = x == 1;\n    let q = 3 == 3u8;\n    let r = (1u64);\n}\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&fi];
    let ty = |p: &str| hover(&ws, 0, at(src, p, 0), &no).unwrap();
    for (p, want) in [("a =", "a: &str"), ("b =", "b: char"), ("c =", "c: &[u8]"), ("d =", "d: u8"), ("e =", "e: u8"), ("i =", "i: bool"), ("j =", "j: f32"), ("k =", "k: usize"), ("m =", "m: &str")] {
        assert!(ty(p).contains(want), "{p}: {}", ty(p));
    }
    // 接尾辞のない数 (使い方で決まる)・タプルの添字・式の途中のリテラルは型にしない
    for p in ["g =", "h =", "l =", "n =", "o =", "q ="] {
        assert!(!ty(p).contains(':'), "{p}: {}", ty(p));
    }
}

/// ライブラリのファイルの `pub use` (再エクスポート): 補完 `use std::sync::Mu|` と、パスの解決
#[test]
fn library_reexports() {
    let lib = FileIndex::library("#[stable]\npub use self::poison::{\n    Mutex, MutexGuard,\n};\nuse self::private::Hidden;\npub mod poison;\n".to_string(), s(&["sync"]));
    let m = FileIndex::library("pub struct Mutex;\npub struct MutexGuard;\n".to_string(), s(&["sync", "poison"]));
    let m2 = FileIndex::library("pub struct Mutex;\n".to_string(), s(&["sync", "nonpoison"]));
    assert_eq!(lib.uses.iter().map(|u| u.0.as_str()).collect::<Vec<_>>(), vec!["Mutex", "MutexGuard"]);
    let src = "use std::sync::Mu;\nuse std::sync::Mutex;\nfn f(m: Mutex) {}\n";
    let f = FileIndex::new(src.to_string(), vec![]);
    let ws = [&lib, &m, &m2, &f];
    let (it, _) = complete(&ws, 3, at(src, "Mu;", 0) + 2);
    let labels: Vec<&str> = it.iter().map(|x| x.label.as_str()).collect();
    assert!(labels.contains(&"Mutex") && labels.contains(&"MutexGuard") && !labels.contains(&"Hidden"), "{labels:?}");
    // (Mutex は再エクスポートをたどって sync::poison の方: nonpoison ではない)
    assert_eq!(definition(&ws, 3, at(src, "Mutex)", 0)).first().map(|l| l.file), Some(1));
}

#[test]
fn literal_hovers() {
    let src = "fn f(x: u8) {\n    let a = \"CRUSTC_LOG\";\n    let b = \"a\\n\\u{41}\\x42\";\n    let c = r#\"raw \"q\"\"#;\n    let d = b\"xy\";\n    let e = 'z';\n    let g = b'q';\n    let h = format!(\"{x}\");\n}\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&fi];
    let h = |p: &str, d: usize| hover(&ws, 0, at(src, p, 0) + d, &no);
    assert_eq!(h("CRUSTC_LOG", 2).unwrap(), "```rust\n&'static str\n```\n---\n\nvalue of literal: ` CRUSTC_LOG `");
    assert!(h("a\\n", 0).unwrap().ends_with("` a\nAB `"), "{:?}", h("a\\n", 0));
    assert!(h("raw", 0).unwrap().ends_with("` raw \"q\" `"));
    assert!(h("xy", 0).unwrap().contains("&'static [u8; 2]"));
    assert!(h("z'", 0).unwrap().contains("char"));
    assert!(h("q'", 0).unwrap().contains("u8"));
    // (書式の取り込み `{x}` の上はローカルの hover)
    assert!(h("x}", 0).unwrap().contains("x: u8"));
    // (識別子の上はふつうの hover)
    assert!(h("a =", 0).unwrap().contains("a: &str"));
}

/// クロージャの引数の型: 受け手の要素の型 (補完・hover・定義に効く)
#[test]
fn closure_params_from_receivers() {
    let src = "struct D { a: u8 }\nimpl D { fn get(&self) -> u8 { self.a } }\n\
               fn f(v: Vec<D>, o: Option<D>, r: Result<D, String>) {\n    v.iter().filter(|d| d.a > 0);\n    v.iter().map(|e| e.get());\n    v.into_iter().for_each(move |g| { g.a; });\n    o.map(|x| x.a);\n    r.map_err(|s| s.len());\n    v.iter().fold(0, |acc, h| acc + h.a);\n}\n";
    let lib = FileIndex::library(
        "pub struct Vec<T> { b: T }\nimpl<T> Vec<T> {\n    pub fn iter(&self) -> Iter<'_, T> { todo!() }\n    pub fn into_iter(self) -> IntoIter<T> { todo!() }\n}\n\
         pub struct Iter<'a, T> { r: &'a T }\npub struct IntoIter<T> { t: T }\npub struct String;\nimpl String { pub fn len(&self) -> usize { 0 } }\n"
            .to_string(),
        s(&["vec"]),
    );
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&fi, &lib];
    let ty = |p: &str| hover(&ws, 0, at(src, p, 0), &no).unwrap();
    // (`iter()` の要素は &D、filter の引数はその参照: rust-analyzer と同じ)
    assert!(ty("d| d.a").contains("d: &&D"), "{}", ty("d| d.a"));
    assert!(ty("e| e").contains("e: &D"), "{}", ty("e| e"));
    assert!(ty("g| {").contains("g: D"), "{}", ty("g| {"));
    assert!(ty("x| x").contains("x: D"), "{}", ty("x| x"));
    assert!(ty("s| s").contains("s: String"), "{}", ty("s| s"));
    assert!(ty("h| acc").contains("h: &D"), "{}", ty("h| acc"));
    assert!(!ty("acc, h").contains(':'), "{}", ty("acc, h"));
    // d.a のフィールド・e.get() のメソッドの定義
    assert_eq!(definition(&ws, 0, at(src, "a > 0", 0)).first().map(|l| l.start), Some(at(src, "a: u8", 0)));
    assert_eq!(definition(&ws, 0, at(src, "get());", 0)).first().map(|l| l.start), Some(at(src, "get(&self)", 0)));
    // 補完 `d.|`
    let (it, _) = complete(&ws, 0, at(src, "a > 0", 0));
    assert!(it.iter().any(|x| x.label == "a") && it.iter().any(|x| x.label == "get"), "{:?}", it.iter().map(|x| &x.label).collect::<Vec<_>>());
}

/// アイテムのキーワードの上 (`fn f` の fn): その名前の定義と参照
#[test]
fn keyword_of_item() {
    let src = "fn f() {}\nstruct S;\nfn g() { f(); let _ = S; }\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&fi];
    assert_eq!(def1(&fi, 0), Some(3));
    // (`fn` の上の参照は関数の出口: rust-analyzer と同じ。`fn f() {}` は fn だけ)
    assert_eq!(references(&ws, 0, 0, true, &no).len(), 1);
    assert_eq!(references(&ws, 0, at(src, "f()", 0), true, &no).len(), 2);
    assert_eq!(def1(&fi, at(src, "struct", 0)), Some(at(src, "S;", 0)));
    assert_eq!(highlights(&ws, 0, at(src, "struct", 0), &no).len(), 2);
    // (キーワードでない所はそのまま)
    assert_eq!(def1(&fi, at(src, "let", 0)), None);
}

/// `fn` のキーワードの上: 関数の出口 (fn・return・?・最後の式)
#[test]
fn exit_points_of_fn() {
    let src = "fn f(x: Option<u8>) -> Option<u8> {\n    let y = x?;\n    if y > 3 {\n        return None;\n    }\n    fn inner() -> Option<u8> { None? }\n    Some(y + 1)\n}\nfn g() { h(); }\nfn k() -> u8 { if true { 1 } else { 2 } }\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&fi];
    let ex = |p: &str| highlights(&ws, 0, at(src, p, 0), &no).iter().map(|l| src[l.start..l.end].to_string()).collect::<Vec<_>>();
    assert_eq!(ex("fn f"), s(&["fn", "?", "return", "Some(y + 1)"]));
    assert_eq!(ex("fn g"), s(&["fn"]));
    // (最後の式が if / match なら、その中の各ブロック・各腕の最後の式: rust-analyzer と同じ)
    assert_eq!(ex("fn k"), s(&["fn", "1", "2"]));
    let src2 = "fn m(x: u8) -> u8 {\n    match x {\n        0 => 1,\n        1 => { let y = 2; y }\n        _ => match x { 2 => 3, _ => 4 },\n    }\n}\n";
    let f2 = FileIndex::new(src2.to_string(), vec![]);
    let w2 = [&f2];
    let got: Vec<String> = highlights(&w2, 0, 0, &no).iter().map(|l| src2[l.start..l.end].to_string()).collect();
    assert_eq!(got, s(&["fn", "1", "y", "3", "4"]));
}

/// `thread_local! { static X: .. }` の static
#[test]
fn thread_local_statics() {
    let src = "thread_local! {\n    /// 覚え\n    static MEMO: RefCell<u8> = RefCell::new(0);\n    pub static OTHER: u8 = 1;\n}\nfn f() { MEMO.with(|m| m); OTHER.with(|o| o); }\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&fi];
    assert_eq!(def1(&fi, at(src, "MEMO.with", 0)), Some(at(src, "MEMO:", 0)));
    assert_eq!(def1(&fi, at(src, "OTHER.with", 0)), Some(at(src, "OTHER:", 0)));
    assert_eq!(references(&ws, 0, at(src, "MEMO:", 0), true, &no).len(), 2);
    assert!(hover(&ws, 0, at(src, "MEMO.with", 0), &no).unwrap().contains("覚え"));
}

#[test]
fn fn_bounds_of_signatures() {
    let b = |s: &str| fn_bounds(s).into_iter().map(|(i, f, r, _)| format!("{i}:{f}:{r}")).collect::<Vec<_>>();
    let a = |s: &str| fn_bounds(s).into_iter().map(|(_, _, _, a)| a).collect::<Vec<_>>();
    assert_eq!(a("pub fn with<F, R>(&'static self, f: F) -> R where F: FnOnce(&T) -> R"), s(&["&T"]));
    assert_eq!(a("fn fold<B, F>(mut self, init: B, f: F) -> B where F: FnMut(B, Self::Item) -> B"), s(&["", "B, Self::Item"]));
    assert_eq!(b("pub fn map<U, F>(self, f: F) -> Option<U> where F: FnOnce(T) -> U"), s(&["0:F:U"]));
    assert_eq!(b("fn map<B, F>(self, f: F) -> Map<Self, F> where Self: Sized, F: FnMut(Self::Item) -> B,"), s(&["0:F:B"]));
    assert_eq!(b("pub const fn and_then<U, F: [const] FnOnce(T) -> Option<U> + [const] Destruct>(self, f: F) -> Option<U>"), s(&["0:F:Option<U>"]));
    // (`init: B` は型引数そのものの引数: 印は "=")
    assert_eq!(b("fn fold<B, F>(mut self, init: B, f: F) -> B where F: FnMut(B, Self::Item) -> B"), s(&["0:B:=", "1:F:B"]));
    assert_eq!(b("fn for_each<F: FnMut(Self::Item)>(self, f: F)"), s(&["0:F:()"]));
    assert_eq!(b("pub fn unwrap_or_else(self, op: impl FnOnce(E) -> T) -> T"), s(&["0::T"]));
    assert_eq!(b("fn find<P>(&mut self, predicate: P) -> Option<Self::Item> where P: FnMut(&Self::Item) -> bool"), s(&["0:P:bool"]));
    assert_eq!(b("fn len(&self) -> usize"), Vec::<String>::new());
    // (長いシグネチャを途中で切ったもの: 落ちない)
    assert_eq!(b("fn g<F: FnMut(u8, …"), Vec::<String>::new());
    assert_eq!(b("fn h<F>(f: F) where F: Fn(&[u8; 4], …"), s(&["0:F:="]));
}

/// 繰り返しの要素の型・射影の置き換え・タプル
#[test]
fn iterator_items_and_projections() {
    assert_eq!(iter_item("Iter<'_, D>").as_deref(), Some("&D"));
    assert_eq!(iter_item("Map<Iter<'_, D>, impl FnMut() -> u8>").as_deref(), Some("u8"));
    assert_eq!(iter_item("Enumerate<Iter<'_, D>>").as_deref(), Some("(usize, &D)"));
    assert_eq!(iter_item("FilterMap<Iter<'_, D>, impl FnMut() -> Option<char>>").as_deref(), Some("char"));
    assert_eq!(iter_item("Rev<Chars<'_>>").as_deref(), Some("char"));
    assert_eq!(iter_item("Cloned<Iter<'_, u8>>").as_deref(), Some("u8"));
    assert_eq!(iter_item("Zip<IntoIter<u8>, Range<usize>>").as_deref(), Some("(u8, usize)"));
    assert_eq!(iter_item("Iter<'_, K, V>").as_deref(), Some("(&K, &V)"));
    assert_eq!(iter_item("&Vec<D>").as_deref(), Some("&D"));
    assert_eq!(iter_item("Flatten<Iter<'_, Vec<u8>>>").as_deref(), Some("&u8"));
    assert_eq!(iter_item("Foo<u8>"), None);
    assert_eq!(norm_proj("Option<Map<Iter<'_, D>, impl FnMut() -> u8>::Item>"), "Option<u8>");
    assert_eq!(norm_proj("Option<<Rev<Iter<'_, D>> as Iterator>::Item>"), "Option<&D>");
    assert_eq!(norm_proj("Option<X::Item>"), "Option<X::Item>");
    assert_eq!(tuple_items("(String, Vec<(u8, u8)>)"), Some(s(&["String", "Vec<(u8, u8)>"])));
    let mut b = vec![];
    unify("Option<U>", "Option<char>", &s(&["U"]), &mut b);
    assert_eq!(b, vec![("U".to_string(), "char".to_string())]);
}

/// タプルの添字の後の添字・メソッド (`u.1[0].as_str()`) とクロージャの戻り値からの型引数 (`a.map(|u| ..)`)
#[test]
fn tuple_index_and_closure_returns() {
    let lib = FileIndex::library("pub struct String;\nimpl String { pub fn as_str(&self) -> &str { todo!() } }\npub enum Option<T> { Some(T), None }\nimpl<T> Option<T> { pub fn map<U, F>(self, f: F) -> Option<U> where F: FnOnce(T) -> U { todo!() } }\n".to_string(), s(&["x"]));
    let src = "fn f(a: Option<&(String, Vec<String>)>, u: &(String, Vec<String>)) {\n    let b = a.map(|u| u.1[0].as_str());\n    let c = u.1[0].as_str();\n    let d = u.1[0];\n    let e = u.1;\n    let g = a.map(|u| &u.0);\n}\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&fi, &lib];
    let ty = |p: &str| hover(&ws, 0, at(src, p, 0), &no).unwrap();
    for (p, want) in [("b =", "b: Option<&str>"), ("c =", "c: &str"), ("d =", "d: String"), ("e =", "e: Vec<String>"), ("g =", "g: Option<&String>")] {
        assert!(ty(p).contains(want), "{p}: {}", ty(p));
    }
}

/// HashMap の繰り返しとタプルのパターン・タプルの式 (`m.iter().map(|(k, x)| (k.len(), x.len())).next()`)
#[test]
fn map_iteration_and_tuples() {
    let lib = FileIndex::library("pub struct HashMap<K, V> { k: K, v: V }\nimpl<K, V> HashMap<K, V> { pub fn iter(&self) -> Iter<'_, K, V> { todo!() } }\npub struct Iter<'a, K, V> { k: &'a K, v: V }\npub struct Map<I, F> { i: I, f: F }\npub trait Iterator { type Item; fn map<B, F>(self, f: F) -> Map<Self, F> where F: FnMut(Self::Item) -> B { todo!() } fn next(&mut self) -> Option<Self::Item>; }\npub struct String;\nimpl String { pub fn len(&self) -> usize { 0 } }\n".to_string(), s(&["x"]));
    let src = "fn f(m: HashMap<String, String>) {\n    let b = m.iter().map(|(k, x)| k).next();\n    let c = m.iter().map(|(k, x)| (k.len(), x.len()));\n    let d = c.next();\n    let e = (1u8, \"a\");\n    let g = (2u16);\n}\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&fi, &lib];
    let ty = |p: &str| hover(&ws, 0, at(src, p, 0), &no).unwrap();
    for (p, want) in [("b =", "b: Option<&String>"), ("d =", "d: Option<(usize, usize)>"), ("k)", "k: &String"), ("x)", "x: &String"), ("e =", "e: (u8, &str)"), ("g =", "g: u16")] {
        assert!(ty(p).contains(want), "{p}: {}", ty(p));
    }
}

/// シグネチャの束縛からのクロージャの引数の型 (`KEY.with(|m| ..)` の m: &T)、関数を入れたローカルの signatureHelp
#[test]
fn closure_params_from_signatures() {
    let lib = FileIndex::library("pub struct LocalKey<T> { t: T }\nimpl<T: 'static> LocalKey<T> { pub fn with<F, R>(&'static self, f: F) -> R where F: FnOnce(&T) -> R { todo!() } }\npub struct Cell<T> { t: T }\nimpl<T> Cell<T> { pub fn get(&self) -> T { todo!() } }\n".to_string(), s(&["x"]));
    let src = "thread_local! { static K: Cell<u8> = Cell::new(0); }\nfn rank(d: &u8, e: u16) -> usize { 0 }\nfn f() {\n    let v = K.with(|m| m.get());\n    let score = rank;\n    score(&1, 2);\n}\n";
    let fi = FileIndex::new(src.to_string(), vec![]);
    let ws = [&fi, &lib];
    let ty = |p: &str| hover(&ws, 0, at(src, p, 0), &no).unwrap();
    assert!(ty("m| m").contains("m: &Cell<u8>"), "{}", ty("m| m"));
    assert!(ty("v =").contains("v: u8"), "{}", ty("v ="));
    let (sig, ps, a, _) = signature_help(&ws, 0, at(src, "2);", 0), &no).unwrap();
    assert_eq!((sig.as_str(), ps.len(), a), ("fn rank(d: &u8, e: u16) -> usize", 2, 1));
}
