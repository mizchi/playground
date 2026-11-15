use std::cell::RefCell;
use std::rc::Rc;
#[derive(Debug)]
struct Node { val: i32, children: Vec<Rc<RefCell<Node>>> }
fn main() {
    let leaf = Rc::new(RefCell::new(Node { val: 3, children: vec![] }));
    let root = Rc::new(RefCell::new(Node { val: 1, children: vec![leaf.clone()] }));
    leaf.borrow_mut().val += 10;
    println!("{} {}", root.borrow().children[0].borrow().val, Rc::strong_count(&leaf));
    let c = RefCell::new(vec![1, 2]);
    c.borrow_mut().push(3);
    println!("{:?}", c.borrow());
}
