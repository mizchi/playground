// trait の静的呼び出し (`Tr::f(..)` / `T::f(..)` / ジェネリックな impl)、スライス → 配列の try_into、
// スライスの get(範囲)、ハッシャ付きの HashMap 型
use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::collections::hash_map::DefaultHasher;

trait D: Sized {
    fn d(x: &mut u64) -> Option<Self>;
    fn n(&self) -> usize;
}
impl D for u64 {
    fn d(x: &mut u64) -> Option<u64> { *x += 1; if *x > 20 { None } else { Some(*x) } }
    fn n(&self) -> usize { 1 }
}
impl D for bool {
    fn d(x: &mut u64) -> Option<bool> { *x += 1; Some(*x % 2 == 0) }
    fn n(&self) -> usize { 1 }
}
impl<T: D> D for Vec<T> {
    fn d(x: &mut u64) -> Option<Self> { let mut v = Vec::new(); for _ in 0..2 { v.push(T::d(x)?); } Some(v) }
    fn n(&self) -> usize { self.iter().map(|x| x.n()).sum() }
}
impl<A: D, B: D> D for (A, B) {
    fn d(x: &mut u64) -> Option<Self> { Some((A::d(x)?, B::d(x)?)) }
    fn n(&self) -> usize { self.0.n() + self.1.n() }
}
impl<T: D> D for Option<T> {
    fn d(x: &mut u64) -> Option<Self> { Some(Some(T::d(x)?)) }
    fn n(&self) -> usize { match self { Some(v) => v.n(), None => 0 } }
}
#[derive(Default, Debug)]
struct S { a: u64, b: bool, c: Vec<u64>, p: Option<(u64, bool)> }

fn g(x: &mut u64) -> Option<S> {
    let mut s = <S>::default();
    s.a = D::d(x)?;
    s.b = crate::D::d(x)?;
    s.c = D::d(x)?;
    s.p = D::d(x)?;
    Some(s)
}

fn word(b: &[u8]) -> u32 { u32::from_le_bytes(b[..4].try_into().unwrap()) }

type M<K, V> = HashMap<K, V, BuildHasherDefault<DefaultHasher>>;

fn main() {
    let mut x = 0;
    let s = g(&mut x).unwrap();
    println!("{:?} {} {}", s, D::n(&s.c), D::n(&s.p));
    let w: Option<Vec<(bool, u64)>> = D::d(&mut x);
    println!("{:?}", w);
    let mut y = 18;
    let z: Option<Vec<u64>> = D::d(&mut y);
    println!("{:?}", z);
    let v = vec![1u8, 2, 3, 4, 5, 6];
    println!("{} {}", word(&v), word(&v[2..]));
    let a: [u8; 2] = v[1..3].try_into().unwrap();
    let r: Result<[u8; 3], _> = v[..].try_into();
    println!("{:?} {}", a, r.is_err());
    println!("{:?} {:?} {:?}", v.get(1..3), v.get(4..9), v.get(3..));
    let mut m: M<String, u32> = M::default();
    for (i, w) in "a b c a b a".split(' ').enumerate() {
        *m.entry(w.to_string()).or_insert(0) += i as u32;
    }
    let mut kv: Vec<_> = m.into_iter().collect();
    kv.sort();
    println!("{:?}", kv);
}
