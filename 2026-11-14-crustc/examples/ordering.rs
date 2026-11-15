// 大小比較のサンプル: タプルの < / sort、#[derive(PartialOrd, Ord)] (struct / enum)、手書きの impl Ord、
// cmp / partial_cmp / Ordering のメソッド、sort_by / sort_by_key / Reverse / max_by_key、Option / 配列 / String / Vec の比較
use std::cmp::Ordering;
use std::cmp::Reverse;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Ver {
    major: u32,
    minor: u32,
    patch: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prio {
    Low,
    Mid(u8),
    High { boost: i32 },
}

#[derive(Debug, PartialEq, Eq)]
struct Person {
    name: String,
    age: u32,
}

impl PartialOrd for Person {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Person {
    fn cmp(&self, other: &Self) -> Ordering {
        self.age.cmp(&other.age).then_with(|| self.name.cmp(&other.name))
    }
}

fn largest<T: PartialOrd + Copy>(xs: &[T]) -> T {
    let mut m = xs[0];
    for &x in xs {
        if x > m {
            m = x;
        }
    }
    m
}

fn main() {
    // タプル
    let mut v: Vec<(u32, i32)> = vec![(3, 1), (1, 9), (3, -2), (2, 5), (1, 2)];
    v.sort();
    println!("{:?}", v);
    println!("{} {} {}", (1, 2) < (1, 3), (2, 0) <= (1, 9), ("b", 1) > ("a", 5));
    println!("{:?} {:?}", (1, 'a').cmp(&(1, 'b')), (5u8, 5u8).partial_cmp(&(5, 5)));
    let best = v.iter().max().unwrap();
    println!("{:?} {:?}", best, v.iter().min());
    // derive(PartialOrd, Ord)
    let a = Ver { major: 1, minor: 2, patch: 3 };
    let b = Ver { major: 1, minor: 10, patch: 0 };
    println!("{} {:?} {:?}", a < b, a.cmp(&b), a.max(b));
    let mut ps = vec![Prio::High { boost: 1 }, Prio::Low, Prio::Mid(7), Prio::Mid(2), Prio::High { boost: -4 }];
    ps.sort();
    println!("{:?}", ps);
    println!("{}", Prio::Mid(200) < Prio::High { boost: -100 });
    println!("{:?}", largest(&[Ver { major: 0, minor: 9, patch: 9 }, a, b]));
    // 手書きの Ord
    let mut people = vec![
        Person { name: String::from("bob"), age: 30 },
        Person { name: String::from("alice"), age: 30 },
        Person { name: String::from("carol"), age: 25 },
    ];
    people.sort();
    for p in &people {
        println!("{} {}", p.name, p.age);
    }
    println!("{}", people[0] < people[1]);
    // sort_by / sort_by_key / Reverse / max_by_key
    let mut words = vec!["pear", "fig", "banana", "kiwi", "apple"];
    words.sort_by(|x, y| y.len().cmp(&x.len()).then(x.cmp(y)));
    println!("{:?}", words);
    words.sort_by_key(|w| Reverse(w.len()));
    println!("{:?}", words);
    let mut nums = vec![5, 1, 4, 2, 3];
    nums.sort_by_key(|&n| Reverse(n));
    println!("{:?}", nums);
    nums.sort_unstable();
    println!("{:?}", nums);
    let longest = words.iter().max_by_key(|w| w.len());
    let shortest = words.iter().min_by_key(|w| w.len());
    println!("{:?} {:?}", longest, shortest);
    let oldest = people.iter().map(|p| p.age).max();
    println!("{:?}", oldest);
    let o = 3.cmp(&5);
    println!("{:?} {:?} {} {}", o, o.reverse(), o.is_lt(), o == Ordering::Less);
    match "abc".cmp("abd") {
        Ordering::Less => println!("less"),
        Ordering::Equal => println!("eq"),
        Ordering::Greater => println!("gt"),
    }
    // Option / 配列 / String / Vec
    println!("{} {} {}", Some(3) > None, Some(1) < Some(2), [1, 2, 3] < [1, 3, 0]);
    println!("{} {}", String::from("abc") < String::from("abd"), vec![1, 2] < vec![1, 2, 0]);
    let mut names = vec![String::from("zed"), String::from("amy"), String::from("bo")];
    names.sort();
    println!("{:?} {:?}", names, String::from("x").max(String::from("y")));
    let mut pairs = vec![(String::from("b"), 2), (String::from("a"), 2), (String::from("c"), 1)];
    pairs.sort_by(|x, y| x.1.cmp(&y.1).then_with(|| x.0.cmp(&y.0)));
    println!("{:?}", pairs);
}
