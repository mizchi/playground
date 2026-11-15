use std::collections::HashMap;
use std::fmt;
#[derive(Debug, Clone, PartialEq)]
enum Tok { Num(f64), Ident(String), Op(char), LParen, RParen, Let, Eq }
fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = vec![];
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() { i += 1; continue; }
        if c.is_ascii_digit() || c == '.' {
            let st = i;
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') { i += 1; }
            let t: String = cs[st..i].iter().collect();
            out.push(Tok::Num(t.parse().map_err(|_| format!("bad number {}", t))?));
            continue;
        }
        if c.is_alphabetic() {
            let st = i;
            while i < cs.len() && cs[i].is_alphanumeric() { i += 1; }
            let w: String = cs[st..i].iter().collect();
            out.push(if w == "let" { Tok::Let } else { Tok::Ident(w) });
            continue;
        }
        out.push(match c { '(' => Tok::LParen, ')' => Tok::RParen, '=' => Tok::Eq, '+' | '-' | '*' | '/' => Tok::Op(c), _ => return Err(format!("unexpected {:?}", c)) });
        i += 1;
    }
    Ok(out)
}
#[derive(Debug)]
enum Expr { Num(f64), Var(String), Bin(Box<Expr>, char, Box<Expr>), Neg(Box<Expr>) }
impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Expr::Num(n) => write!(f, "{}", n),
            Expr::Var(v) => write!(f, "{}", v),
            Expr::Bin(a, op, b) => write!(f, "({} {} {})", a, op, b),
            Expr::Neg(a) => write!(f, "-{}", a),
        }
    }
}
struct Parser { toks: Vec<Tok>, pos: usize }
impl Parser {
    fn peek(&self) -> Option<&Tok> { self.toks.get(self.pos) }
    fn next(&mut self) -> Option<Tok> { let t = self.toks.get(self.pos).cloned(); self.pos += 1; t }
    fn expr(&mut self) -> Result<Expr, String> {
        let mut l = self.term()?;
        while let Some(Tok::Op(c)) = self.peek().cloned() {
            if c != '+' && c != '-' { break; }
            self.next();
            let r = self.term()?;
            l = Expr::Bin(Box::new(l), c, Box::new(r));
        }
        Ok(l)
    }
    fn term(&mut self) -> Result<Expr, String> {
        let mut l = self.atom()?;
        while let Some(Tok::Op(c)) = self.peek().cloned() {
            if c != '*' && c != '/' { break; }
            self.next();
            let r = self.atom()?;
            l = Expr::Bin(Box::new(l), c, Box::new(r));
        }
        Ok(l)
    }
    fn atom(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(Expr::Num(n)),
            Some(Tok::Ident(v)) => Ok(Expr::Var(v)),
            Some(Tok::Op('-')) => Ok(Expr::Neg(Box::new(self.atom()?))),
            Some(Tok::LParen) => { let e = self.expr()?; match self.next() { Some(Tok::RParen) => Ok(e), t => Err(format!("expected ), got {:?}", t)) } }
            t => Err(format!("unexpected token {:?}", t)),
        }
    }
}
fn eval(e: &Expr, env: &HashMap<String, f64>) -> Result<f64, String> {
    Ok(match e {
        Expr::Num(n) => *n,
        Expr::Var(v) => *env.get(v).ok_or_else(|| format!("unbound {}", v))?,
        Expr::Neg(a) => -eval(a, env)?,
        Expr::Bin(a, op, b) => {
            let (x, y) = (eval(a, env)?, eval(b, env)?);
            match op { '+' => x + y, '-' => x - y, '*' => x * y, '/' => { if y == 0.0 { return Err("div by zero".into()); } x / y } _ => unreachable!() }
        }
    })
}
fn run(line: &str, env: &mut HashMap<String, f64>) -> Result<String, String> {
    let toks = lex(line)?;
    if toks.first() == Some(&Tok::Let) {
        let name = match toks.get(1) { Some(Tok::Ident(n)) => n.clone(), _ => return Err("expected name".into()) };
        if toks.get(2) != Some(&Tok::Eq) { return Err("expected =".into()); }
        let mut p = Parser { toks: toks[3..].to_vec(), pos: 0 };
        let e = p.expr()?;
        let v = eval(&e, env)?;
        env.insert(name.clone(), v);
        return Ok(format!("{} = {}", name, v));
    }
    let mut p = Parser { toks, pos: 0 };
    let e = p.expr()?;
    Ok(format!("{} => {}", e, eval(&e, env)?))
}
fn main() {
    let mut env = HashMap::new();
    for line in ["let x = 2 + 3 * 4", "x * (1 + 1)", "let y = x / 4", "-y + x", "z + 1", "1 / 0", "2 $ 3", "(1 + 2"] {
        match run(line, &mut env) { Ok(s) => println!("{}", s), Err(e) => println!("error: {}", e) }
    }
}
