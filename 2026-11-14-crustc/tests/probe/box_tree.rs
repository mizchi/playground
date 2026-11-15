#[derive(Debug)]
enum Tree { Leaf, Node(Box<Tree>, i32, Box<Tree>) }
impl Tree {
    fn insert(self, v: i32) -> Tree {
        match self {
            Tree::Leaf => Tree::Node(Box::new(Tree::Leaf), v, Box::new(Tree::Leaf)),
            Tree::Node(l, x, r) => {
                if v < x { Tree::Node(Box::new(l.insert(v)), x, r) } else { Tree::Node(l, x, Box::new(r.insert(v))) }
            }
        }
    }
    fn walk(&self, out: &mut Vec<i32>) {
        if let Tree::Node(l, x, r) = self { l.walk(out); out.push(*x); r.walk(out); }
    }
    fn depth(&self) -> usize { match self { Tree::Leaf => 0, Tree::Node(l, _, r) => 1 + l.depth().max(r.depth()) } }
}
fn main() {
    let mut t = Tree::Leaf;
    for v in [5, 3, 8, 1, 4, 9, 7] { t = t.insert(v); }
    let mut o = vec![];
    t.walk(&mut o);
    println!("{:?} {}", o, t.depth());
}
