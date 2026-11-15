// 行列の積 (Vec<Vec<f64>>、二重の添字)
fn main() {
    let n = 200;
    let a: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| ((i * j) % 7) as f64 + 0.5).collect()).collect();
    let b: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| ((i + 2 * j) % 5) as f64 - 1.0).collect()).collect();
    let mut c = vec![vec![0.0; n]; n];
    for _ in 0..5 {
        for i in 0..n {
            for k in 0..n {
                let aik = a[i][k];
                for j in 0..n {
                    c[i][j] += aik * b[k][j];
                }
            }
        }
    }
    let tr: f64 = (0..n).map(|i| c[i][i]).sum();
    println!("{}", tr);
}
