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
    want.extend(["other", "other_kv", "other_fn", "m", "inner"]);
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
