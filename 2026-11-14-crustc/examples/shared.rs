// Rc / RefCell / Cell と、Vec や Rc を通した再帰型
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug)]
struct Node {
    name: String,
    val: i32,
    children: Vec<Rc<RefCell<Node>>>,
}

impl Node {
    fn new(name: &str, val: i32) -> Rc<RefCell<Node>> {
        Rc::new(RefCell::new(Node { name: name.to_string(), val, children: vec![] }))
    }
}

fn sum(n: &Rc<RefCell<Node>>) -> i32 {
    let node = n.borrow();
    node.val + node.children.iter().map(|c| sum(c)).sum::<i32>()
}

fn depth(n: &Rc<RefCell<Node>>) -> usize {
    1 + n.borrow().children.iter().map(|c| depth(c)).max().unwrap_or(0)
}

#[derive(Debug, Clone)]
struct Tree {
    label: String,
    kids: Vec<Tree>,
}

fn count(t: &Tree) -> usize {
    1 + t.kids.iter().map(count).sum::<usize>()
}

struct Counter {
    hits: Cell<u32>,
    log: RefCell<Vec<String>>,
}

impl Counter {
    fn hit(&self, who: &str) {
        self.hits.set(self.hits.get() + 1);
        self.log.borrow_mut().push(format!("{}#{}", who, self.hits.get()));
    }
}

fn main() {
    let root = Node::new("root", 1);
    let a = Node::new("a", 2);
    let b = Node::new("b", 3);
    let c = Node::new("c", 4);
    a.borrow_mut().children.push(c.clone());
    root.borrow_mut().children.push(a.clone());
    root.borrow_mut().children.push(b.clone());
    c.borrow_mut().val *= 10;
    println!("sum={} depth={}", sum(&root), depth(&root));
    println!("rc(c)={} rc(root)={}", Rc::strong_count(&c), Rc::strong_count(&root));
    {
        let extra = c.clone();
        println!("rc(c)={} {}", Rc::strong_count(&extra), extra.borrow().name);
    }
    println!("rc(c)={}", Rc::strong_count(&c));
    let names: Vec<String> = root.borrow().children.iter().map(|n| n.borrow().name.clone()).collect();
    println!("{:?}", names);

    let shared = Rc::new(RefCell::new(HashMap::new()));
    let s2 = Rc::clone(&shared);
    for (i, w) in "x y x z x y".split(' ').enumerate() {
        *s2.borrow_mut().entry(w.to_string()).or_insert(0) += i;
    }
    let mut kv: Vec<(String, usize)> = shared.borrow().iter().map(|(k, v)| (k.clone(), *v)).collect();
    kv.sort();
    println!("{:?} same={}", kv, Rc::ptr_eq(&shared, &s2));

    let t = Tree {
        label: "t".to_string(),
        kids: vec![
            Tree { label: "u".to_string(), kids: vec![] },
            Tree { label: "v".to_string(), kids: vec![Tree { label: "w".to_string(), kids: vec![] }] },
        ],
    };
    let t2 = t.clone();
    println!("{} {} {}", count(&t), t2.kids[1].kids[0].label, t.label);

    let ctr = Counter { hits: Cell::new(0), log: RefCell::new(vec![]) };
    ctr.hit("a");
    ctr.hit("b");
    println!("{} {:?}", ctr.hits.get(), ctr.log.borrow());
    let cell = RefCell::new(vec![1, 2, 3]);
    let old = cell.replace(vec![9]);
    println!("{:?} {:?} {:?}", old, cell, Cell::new(7));
    let rcs: Vec<Rc<String>> = (0..3).map(|i| Rc::new(format!("s{}", i))).collect();
    let copies: Vec<Rc<String>> = rcs.iter().cloned().collect();
    println!("{} {} {}", copies[2], copies[0].len(), Rc::strong_count(&rcs[1]));
}
