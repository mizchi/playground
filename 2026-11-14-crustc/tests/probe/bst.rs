#[derive(Debug)]
struct Node { key: i32, left: Option<Box<Node>>, right: Option<Box<Node>> }
#[derive(Debug, Default)]
struct Bst { root: Option<Box<Node>>, size: usize }
impl Bst {
    fn insert(&mut self, key: i32) -> bool {
        let mut cur = &mut self.root;
        while let Some(node) = cur {
            if key == node.key { return false; }
            cur = if key < node.key { &mut node.left } else { &mut node.right };
        }
        *cur = Some(Box::new(Node { key, left: None, right: None }));
        self.size += 1;
        true
    }
    fn contains(&self, key: i32) -> bool {
        let mut cur = &self.root;
        while let Some(n) = cur {
            if key == n.key { return true; }
            cur = if key < n.key { &n.left } else { &n.right };
        }
        false
    }
    fn inorder(&self) -> Vec<i32> {
        fn go(n: &Option<Box<Node>>, out: &mut Vec<i32>) {
            if let Some(b) = n { go(&b.left, out); out.push(b.key); go(&b.right, out); }
        }
        let mut v = Vec::new();
        go(&self.root, &mut v);
        v
    }
    fn height(n: &Option<Box<Node>>) -> usize {
        match n { None => 0, Some(b) => 1 + Self::height(&b.left).max(Self::height(&b.right)) }
    }
}
fn main() {
    let mut t = Bst::default();
    for k in [50, 30, 70, 20, 40, 60, 80, 30] { t.insert(k); }
    println!("{:?} size={} h={}", t.inorder(), t.size, Bst::height(&t.root));
    println!("{} {}", t.contains(60), t.contains(65));
}
