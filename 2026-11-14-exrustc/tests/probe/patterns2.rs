#[derive(Debug, Clone)]
enum Shape {
    Circle(u32),
    Square(u32),
    Rect(u32, u32),
    Named { name: String, side: u32 },
    Empty,
}

fn size(s: &Shape) -> u32 {
    match s {
        Shape::Circle(r) | Shape::Square(r) => *r,
        Shape::Rect(w, h) if w == h => *w,
        Shape::Rect(w, _) | Shape::Named { side: w, .. } => *w * 10,
        Shape::Empty => 0,
    }
}

fn describe(xs: &[i32]) -> String {
    match xs {
        [] => String::from("empty"),
        [x] => format!("one {}", x),
        [first, .., last] if first == last => format!("same ends {}", first),
        [first, rest @ ..] => format!("{} then {} more", first, rest.len()),
    }
}

fn sum(xs: &[i32]) -> i32 {
    match xs {
        [] => 0,
        [x, rest @ ..] => x + sum(rest),
    }
}

fn parse(s: &str) -> Option<i32> {
    let Some(c) = s.chars().next() else {
        return None;
    };
    let (Some(d) | Some(d)) = c.to_digit(10).map(|d| d as i32).or(Some(-1)) else {
        return None;
    };
    Some(d)
}

fn main() {
    let shapes = vec![
        Shape::Circle(3),
        Shape::Square(4),
        Shape::Rect(2, 2),
        Shape::Rect(5, 1),
        Shape::Named { name: String::from("n"), side: 7 },
        Shape::Empty,
    ];
    for s in &shapes {
        print!("{} ", size(s));
    }
    println!();
    println!("{} | {} | {} | {}", describe(&[]), describe(&[4]), describe(&[1, 2, 1]), describe(&[1, 2, 3]));
    println!("{}", sum(&[1, 2, 3, 4]));
    let arr = [10, 20, 30, 40];
    let [a, b, rest @ ..] = arr;
    let [.., y, z] = arr;
    println!("{} {} {:?} {} {}", a, b, rest, y, z);
    if let [first, second, ..] = arr[..] {
        println!("{}", first + second);
    }
    let v = vec![1, 2, 3];
    match v.as_slice() {
        [1, mid @ .., 3] => println!("mid {:?}", mid),
        _ => println!("other"),
    }
    println!("{:?} {:?} {:?}", parse("7x"), parse(""), parse("a"));
    let n = 5;
    let kind = match n {
        1 | 2 => "small",
        x @ (3 | 4 | 5) if x % 2 == 1 => "odd mid",
        _ => "other",
    };
    println!("{}", kind);
    let pairs = [(1, 'a'), (2, 'b')];
    for &(k, c) in &pairs {
        if let (1, ch) | (2, ch) = (k, c) {
            print!("{}{} ", k, ch);
        }
    }
    println!();
}
