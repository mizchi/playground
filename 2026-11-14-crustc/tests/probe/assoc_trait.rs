trait Container {
    type Item;
    fn get(&self, i: usize) -> Option<&Self::Item>;
    fn first(&self) -> Option<&Self::Item> { self.get(0) }
    fn size(&self) -> usize;
}
struct Stack<T> { v: Vec<T> }
impl<T> Container for Stack<T> {
    type Item = T;
    fn get(&self, i: usize) -> Option<&T> { self.v.get(i) }
    fn size(&self) -> usize { self.v.len() }
}
fn describe<C: Container>(c: &C) -> String where C::Item: std::fmt::Debug {
    format!("size={} first={:?}", c.size(), c.first())
}
struct Coll { items: Vec<u32> }
impl<'a> IntoIterator for &'a Coll { type Item = &'a u32; type IntoIter = std::slice::Iter<'a, u32>; fn into_iter(self) -> Self::IntoIter { self.items.iter() } }
fn main() {
    let s = Stack { v: vec!["a", "b"] };
    println!("{}", describe(&s));
    println!("{:?} {:?}", s.get(1), s.get(5));
    let c = Coll { items: vec![3, 1, 2] };
    let mut total = 0;
    for x in &c { total += x; }
    println!("{}", total);
    let o: Option<&u32> = c.items.first();
    println!("{:?} {:?}", o.copied(), o.cloned().map(|x| x * 2));
    let name: Option<String> = Some("z".to_string());
    println!("{:?}", name.as_deref());
    println!("{:?}", Some(4).and_then(|x| if x > 3 { Some(x * 2) } else { None }));
}
