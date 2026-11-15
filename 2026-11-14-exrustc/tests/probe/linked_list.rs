#[derive(Debug)]
struct Node<T> { val: T, next: Option<Box<Node<T>>> }
#[derive(Debug)]
struct List<T> { head: Option<Box<Node<T>>>, len: usize }
impl<T: std::fmt::Debug + Clone> List<T> {
    fn new() -> Self { List { head: None, len: 0 } }
    fn push(&mut self, v: T) { let old = self.head.take(); self.head = Some(Box::new(Node { val: v, next: old })); self.len += 1; }
    fn pop(&mut self) -> Option<T> {
        self.head.take().map(|n| { self.head = n.next; self.len -= 1; n.val })
    }
    fn peek(&self) -> Option<&T> { self.head.as_ref().map(|n| &n.val) }
    fn reverse(&mut self) {
        let mut prev: Option<Box<Node<T>>> = None;
        let mut cur = self.head.take();
        while let Some(mut n) = cur { cur = n.next.take(); n.next = prev; prev = Some(n); }
        self.head = prev;
    }
    fn to_vec(&self) -> Vec<T> {
        let mut v = vec![];
        let mut cur = &self.head;
        while let Some(n) = cur { v.push(n.val.clone()); cur = &n.next; }
        v
    }
}
fn main() {
    let mut l = List::new();
    for i in 1..=5 { l.push(i * 10); }
    println!("{:?} {} {:?}", l.to_vec(), l.len, l.peek());
    l.reverse();
    println!("{:?}", l.to_vec());
    println!("{:?} {:?}", l.pop(), l.pop());
    println!("{:?} {}", l.to_vec(), l.len);
    let mut s: List<String> = List::new();
    s.push("a".to_string()); s.push("b".to_string());
    println!("{:?}", s.pop());
    println!("{:?}", s.to_vec());
}
