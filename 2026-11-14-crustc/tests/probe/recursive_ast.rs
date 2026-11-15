// 互いに再帰する型 (構文木): 非ジェネリックな enum の中の Option<T> (T は後で決まる型) の配置、
// 後で代入する `let x;`
#[derive(Clone, Debug)]
pub struct Path { pub segs: Vec<String> }
#[derive(Clone, Debug)]
pub enum Ty { Path(Option<Box<Ty>>, Path), Ref(bool, Box<Ty>), Array(Box<Ty>, Box<Expr>), Infer }
#[derive(Clone, Debug)]
pub enum Expr { Lit(String), Cast(Box<Expr>, Box<Ty>), Block(Block), Closure(Vec<(String, Option<Ty>)>, Box<Expr>) }
#[derive(Clone, Debug)]
pub struct Block { pub stmts: Vec<Stmt> }
#[derive(Clone, Debug)]
pub enum Stmt { Let(String, Option<Ty>, Option<Expr>), E(Expr), Item(Item) }
#[derive(Clone, Debug)]
pub struct Item { pub kind: ItemKind, pub line: u32 }
#[derive(Clone, Debug)]
pub enum ItemKind { Fn(String, Option<Block>), Const(String, Option<Ty>, Option<Expr>), Mod(String, Vec<Item>) }

fn ty(s: &str) -> Ty {
    let k;
    if let Some(r) = s.strip_prefix('&') {
        k = Ty::Ref(false, Box::new(ty(r)));
    } else if s == "_" {
        k = Ty::Infer;
    } else {
        k = Ty::Path(None, Path { segs: s.split("::").map(String::from).collect() });
    }
    k
}

fn main() {
    let c = ItemKind::Const("N".into(), Some(ty("&usize")), Some(Expr::Cast(Box::new(Expr::Lit("3".into())), Box::new(ty("u8")))));
    let f = ItemKind::Fn("f".into(), Some(Block { stmts: vec![
        Stmt::Let("x".into(), Some(ty("std::string::String")), None),
        Stmt::E(Expr::Closure(vec![("a".into(), Some(ty("_")))], Box::new(Expr::Lit("1".into())))),
        Stmt::Item(Item { kind: c.clone(), line: 3 }),
    ] }));
    let m = Item { kind: ItemKind::Mod("m".into(), vec![Item { kind: f, line: 1 }, Item { kind: c, line: 2 }]), line: 0 };
    println!("{:?}", m);
    let n: u32;
    let label;
    match m.kind { ItemKind::Mod(ref s, ref v) => { n = v.len() as u32; label = s.clone(); } _ => { n = 0; label = String::new(); } }
    println!("{label} {n}");
}
