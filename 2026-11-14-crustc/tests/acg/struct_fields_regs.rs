// minclif がスタックの struct のフィールドをレジスタに置く (値渡し・返り値・フィールドの更新・f64 と u32 の混在)
#[derive(Clone, Copy)]
struct P { x: f64, y: f64, n: u32 }
fn step(p: P, d: f64) -> P { P { x: p.x + d * p.y, y: p.y - d * p.x, n: p.n + 1 } }
fn main() {
    let mut p = P { x: 1.0, y: 0.0, n: 0 };
    let mut acc = 0.0;
    for i in 0..200_000u32 {
        p = step(p, 0.000001);
        acc += p.x * (i & 7) as f64;
    }
    println!("{:.6} {:.6} {} {:.3}", p.x, p.y, p.n, acc);
}
