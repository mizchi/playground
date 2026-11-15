// clone のサンプル: #[derive(Clone)] の struct / enum / ジェネリック / Box を持つ型、タプル・配列・Option・Result の clone、
// Vec<String> の clone / cloned / to_vec、手書きの impl Clone。clone した方を変えても元が変わらず、二重解放もしない (valgrind)
#[derive(Debug, Clone, PartialEq)]
struct Person {
    name: String,
    tags: Vec<String>,
    age: u32,
}

#[derive(Debug, Clone)]
enum Node {
    Leaf(String),
    Pair(Box<Node>, Box<Node>),
    Empty,
}

#[derive(Debug, Clone)]
struct Wrap<T> {
    inner: T,
    n: usize,
}

#[derive(Debug)]
struct Counter {
    hits: u32,
}

impl Clone for Counter {
    fn clone(&self) -> Counter {
        Counter { hits: self.hits + 100 }
    }
}

fn main() {
    let p = Person { name: String::from("ann"), tags: vec![String::from("a"), String::from("b")], age: 30 };
    let mut q = p.clone();
    q.name.push_str("ie");
    q.tags.push(String::from("c"));
    println!("{:?}\n{:?} {}", p, q, p == q);
    let t = Node::Pair(Box::new(Node::Leaf(String::from("x"))), Box::new(Node::Empty));
    let mut u = t.clone();
    if let Node::Pair(l, _) = &mut u {
        **l = Node::Leaf(String::from("changed"));
    }
    println!("{:?}\n{:?}", t, u);
    let w = Wrap { inner: vec![String::from("w")], n: 1 };
    let w2 = w.clone();
    println!("{:?} {:?}", w, w2);
    let tup = (String::from("t"), 5, vec![1, 2]);
    let arr = [String::from("p"), String::from("q")];
    let (t2, a2) = (tup.clone(), arr.clone());
    println!("{:?} {:?} {:?} {:?}", tup, t2, arr, a2);
    let o: Option<String> = Some(String::from("o"));
    let r: Result<Vec<String>, String> = Ok(vec![String::from("r")]);
    println!("{:?} {:?} {:?}", o.clone(), r.clone(), o.as_ref().cloned());
    let names = vec![String::from("x"), String::from("y")];
    let n2 = names.clone();
    let n3: Vec<String> = names.iter().cloned().collect();
    let n4 = names[..1].to_vec();
    println!("{:?} {:?} {:?} {:?}", names, n2, n3, n4);
    let c = Counter { hits: 1 };
    let c2 = c.clone();
    let cs = vec![Counter { hits: 2 }];
    let cs2 = cs.clone();
    println!("{:?} {:?} {:?}", c2, cs2, (1u8, 'x').clone());
}
