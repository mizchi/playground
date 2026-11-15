use std::fmt;
#[derive(Debug, Clone, Copy, PartialEq)]
struct Mat<const R: usize, const C: usize> {
    m: [[i32; C]; R],
}
impl<const R: usize, const C: usize> Mat<R, C> {
    fn zero() -> Self {
        Mat { m: [[0; C]; R] }
    }
    fn dims(&self) -> (usize, usize) {
        (R, C)
    }
    fn transpose(&self) -> Mat<C, R> {
        let mut t = Mat::<C, R>::zero();
        for i in 0..R {
            for j in 0..C {
                t.m[j][i] = self.m[i][j];
            }
        }
        t
    }
}
impl<const R: usize, const C: usize> fmt::Display for Mat<R, C> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}x{}", R, C)
    }
}
fn inner<const N: usize>() -> usize {
    N * 10
}
fn outer<const N: usize>() -> usize {
    inner::<N>() + N
}
fn mk<const N: usize>() -> Vec<usize> {
    (0..N).map(|i| i * N).collect()
}
fn main() {
    let mut a = Mat::<2, 3>::zero();
    a.m[0][2] = 7;
    a.m[1][0] = 4;
    let t = a.transpose();
    println!("{:?} {:?} {}", a.dims(), t.dims(), t);
    println!("{:?}", t);
    println!("{}", t == t.transpose().transpose());
    println!("{} {:?}", outer::<3>(), mk::<4>());
    let n = { const W: usize = 2; inner::<W>() };
    println!("{}", n);
}
