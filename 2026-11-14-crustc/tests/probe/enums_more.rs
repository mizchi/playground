use std::fmt;
#[derive(Debug, Clone, PartialEq)]
enum Expr { Num(f64), Add(Box<Expr>, Box<Expr>), Mul(Box<Expr>, Box<Expr>), Neg(Box<Expr>) }
use Expr::*;
fn eval(e: &Expr) -> f64 {
    match e { Num(n) => *n, Add(a, b) => eval(a) + eval(b), Mul(a, b) => eval(a) * eval(b), Neg(a) => -eval(a) }
}
impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Num(n) => write!(f, "{}", n),
            Add(a, b) => write!(f, "({} + {})", a, b),
            Mul(a, b) => write!(f, "{} * {}", a, b),
            Neg(a) => write!(f, "-{}", a),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Suit { Hearts = 1, Spades = 4 }
enum List { Cons(i32, Box<List>), Nil }
impl List {
    fn sum(&self) -> i32 { match self { List::Cons(v, rest) => v + rest.sum(), List::Nil => 0 } }
}
fn main() {
    let e = Add(Box::new(Num(2.0)), Box::new(Mul(Box::new(Num(3.0)), Box::new(Neg(Box::new(Num(4.0)))))));
    println!("{} = {}", e, eval(&e));
    println!("{:?}", e.clone() == e);
    println!("{} {:?}", Suit::Spades as u8, Suit::Hearts.max(Suit::Spades));
    let l = List::Cons(1, Box::new(List::Cons(2, Box::new(List::Cons(3, Box::new(List::Nil))))));
    println!("{}", l.sum());
    let pair = (3, "x");
    let msg = match pair { (0, _) => "zero".to_string(), (n, s) if n > 2 => format!("{}{}", s, n), _ => "other".into() };
    println!("{}", msg);
    let opt: Option<Option<i32>> = Some(None);
    println!("{}", match opt { Some(Some(v)) => v, Some(None) => -1, None => -2 });
}
