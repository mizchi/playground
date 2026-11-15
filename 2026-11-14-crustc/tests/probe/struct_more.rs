#[derive(Debug, Default, Clone, PartialEq)]
struct Config { name: String, retries: u32, verbose: bool, tags: Vec<String> }
impl Config {
    fn builder() -> Self { Self::default() }
    fn name(mut self, n: &str) -> Self { self.name = n.to_string(); self }
    fn retries(mut self, r: u32) -> Self { self.retries = r; self }
    fn tag(mut self, t: &str) -> Self { self.tags.push(t.into()); self }
}
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
struct P { x: f64, y: f64 }
impl P {
    const ORIGIN: P = P { x: 0.0, y: 0.0 };
    fn dist(&self, o: &P) -> f64 { ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt() }
}
struct Matrix { rows: usize, cols: usize, data: Vec<f64> }
impl Matrix {
    fn new(rows: usize, cols: usize) -> Self { Matrix { rows, cols, data: vec![0.0; rows * cols] } }
    fn identity(n: usize) -> Self { let mut m = Self::new(n, n); for i in 0..n { m.set(i, i, 1.0); } m }
    fn get(&self, r: usize, c: usize) -> f64 { self.data[r * self.cols + c] }
    fn set(&mut self, r: usize, c: usize, v: f64) { self.data[r * self.cols + c] = v; }
    fn mul(&self, o: &Matrix) -> Matrix {
        let mut m = Matrix::new(self.rows, o.cols);
        for i in 0..self.rows { for j in 0..o.cols { let mut s = 0.0; for k in 0..self.cols { s += self.get(i, k) * o.get(k, j); } m.set(i, j, s); } }
        m
    }
}
fn main() {
    let c = Config::builder().name("svc").retries(3).tag("a").tag("b");
    println!("{:?}", c);
    let d = Config { verbose: true, ..c.clone() };
    println!("{} {}", d.verbose, d == c);
    let p = P { x: 3.0, y: 4.0 };
    println!("{} {:?}", p.dist(&P::ORIGIN), p.partial_cmp(&P::ORIGIN));
    let mut a = Matrix::new(2, 2);
    a.set(0, 0, 2.0); a.set(0, 1, 1.0); a.set(1, 0, 0.5); a.set(1, 1, 3.0);
    let b = a.mul(&Matrix::identity(2)).mul(&a);
    println!("{:?}", b.data);
}
