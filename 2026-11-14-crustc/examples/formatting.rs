// format! / println! / write! のサンプル: {} / {:?} / {:#?}、幅・寄せ・0 埋め・16 進・名前つき引数・
// {:>w$}、自前の impl fmt::Display、#[derive(Debug)] (struct / enum / タプル構造体 / ジェネリック)、to_string
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Point {
    x: i32,
    y: i32,
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

#[derive(Debug)]
enum Shape {
    Circle { center: Point, r: u32 },
    Rect(Point, Point),
    Empty,
}

#[derive(Debug)]
struct Unit;

#[derive(Debug)]
struct Wrap<T>(T);

struct Matrix {
    rows: Vec<Vec<i64>>,
}

impl fmt::Display for Matrix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, row) in self.rows.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            for x in row {
                write!(f, "{:>3}", x)?;
            }
        }
        Ok(())
    }
}

fn describe<T: fmt::Display>(x: T) -> String {
    format!("<{}>", x)
}

fn main() {
    let p = Point { x: 1, y: -2 };
    println!("{} {:?}", p, p);
    println!("{:#?}", p);
    let shapes = vec![
        Shape::Circle { center: p, r: 3 },
        Shape::Rect(Point { x: 0, y: 0 }, Point { x: 2, y: 2 }),
        Shape::Empty,
    ];
    println!("{:?}", shapes);
    println!("{:#?}", shapes[1]);
    println!("{:?} {:?} {:?}", Unit, Wrap(5), Wrap("s"));
    let m = Matrix { rows: vec![vec![1, 2, 3], vec![40, 50, 60]] };
    println!("{}", m);
    println!("{}", describe(p));
    println!("{}", describe(7));
    let s = p.to_string();
    println!("{} {}", s.len(), s);
    let o: Option<Point> = None;
    println!("{:?} {:?}", o, Some(p));
    let r: Result<i32, String> = Err(String::from("bad"));
    println!("{:?}", r);
    let mut out = String::new();
    for i in 0..3 {
        out.push_str(&format!("{i},"));
    }
    println!("{out}");
    println!("{:>8}|{:<8}|{:^8}|", "ab", "cd", "ef");
    println!("{:*^9}", "mid");
    println!("{:08.3}", "abcdef");
    println!("{:?}", "quote\"and\\slash\n");
    println!("{{}} {0} {1} {0}", "a", "b");
    let nested = vec![Some(vec![1, 2]), None];
    println!("{:?}", nested);
    println!("{:#?}", nested);
    println!("{:?}", (1, (2, 'c'), [1u8, 2]));
    let width = 6;
    println!("[{:>w$}]", 42, w = 6);
    let _ = width;
    eprintln!("this goes to stderr {}", 1);
}
