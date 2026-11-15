use std::ops::{Add, Mul, Sub, Index, IndexMut};
#[derive(Debug, Clone, PartialEq)]
struct M { r: usize, c: usize, d: Vec<i64> }
impl M {
    fn new(r: usize, c: usize) -> M { M { r, c, d: vec![0; r * c] } }
    fn id(n: usize) -> M { let mut m = M::new(n, n); for i in 0..n { m[(i, i)] = 1; } m }
}
impl Index<(usize, usize)> for M { type Output = i64; fn index(&self, (i, j): (usize, usize)) -> &i64 { &self.d[i * self.c + j] } }
impl IndexMut<(usize, usize)> for M { fn index_mut(&mut self, (i, j): (usize, usize)) -> &mut i64 { &mut self.d[i * self.c + j] } }
impl Add for &M { type Output = M; fn add(self, o: &M) -> M { M { r: self.r, c: self.c, d: self.d.iter().zip(&o.d).map(|(a, b)| a + b).collect() } } }
impl Sub for M { type Output = M; fn sub(self, o: M) -> M { M { r: self.r, c: self.c, d: self.d.iter().zip(o.d.iter()).map(|(a, b)| a - b).collect() } } }
impl Mul for &M {
    type Output = M;
    fn mul(self, o: &M) -> M {
        let mut m = M::new(self.r, o.c);
        for i in 0..self.r { for j in 0..o.c { for k in 0..self.c { m[(i, j)] += self[(i, k)] * o[(k, j)]; } } }
        m
    }
}
fn pow(m: &M, mut e: u32) -> M { let mut r = M::id(m.r); let mut b = m.clone(); while e > 0 { if e & 1 == 1 { r = &r * &b; } b = &b * &b; e >>= 1; } r }
fn main() {
    let mut f = M::new(2, 2);
    f[(0, 0)] = 1; f[(0, 1)] = 1; f[(1, 0)] = 1;
    println!("{:?}", pow(&f, 10).d);
    let s = &f + &M::id(2);
    println!("{:?} {:?}", s.d, (s.clone() - M::id(2)) == f);
}
