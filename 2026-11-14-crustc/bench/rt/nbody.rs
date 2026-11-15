// n 体問題 (f64 の struct の配列、sqrt)
#[derive(Clone, Copy)]
struct Body {
    p: [f64; 3],
    v: [f64; 3],
    m: f64,
}
fn energy(bs: &[Body]) -> f64 {
    let mut e = 0.0;
    for i in 0..bs.len() {
        let b = &bs[i];
        e += 0.5 * b.m * (b.v[0] * b.v[0] + b.v[1] * b.v[1] + b.v[2] * b.v[2]);
        for j in i + 1..bs.len() {
            let c = &bs[j];
            let mut d2 = 0.0;
            for k in 0..3 {
                let d = b.p[k] - c.p[k];
                d2 += d * d;
            }
            e -= b.m * c.m / d2.sqrt();
        }
    }
    e
}
fn step(bs: &mut Vec<Body>, dt: f64) {
    let n = bs.len();
    for i in 0..n {
        for j in i + 1..n {
            let mut d = [0.0; 3];
            let mut d2 = 0.0;
            for k in 0..3 {
                d[k] = bs[i].p[k] - bs[j].p[k];
                d2 += d[k] * d[k];
            }
            let mag = dt / (d2 * d2.sqrt());
            let (mi, mj) = (bs[i].m, bs[j].m);
            for k in 0..3 {
                bs[i].v[k] -= d[k] * mj * mag;
                bs[j].v[k] += d[k] * mi * mag;
            }
        }
    }
    for b in bs.iter_mut() {
        for k in 0..3 {
            b.p[k] += dt * b.v[k];
        }
    }
}
fn main() {
    let mut bs = vec![];
    for i in 0..5 {
        let f = i as f64;
        bs.push(Body { p: [f * 1.1, f * -0.7 + 0.3, f * 0.2], v: [0.01 * f, -0.02, 0.015 * f], m: 1.0 + f * 0.1 });
    }
    println!("{:.9}", energy(&bs));
    for _ in 0..1_000_000 {
        step(&mut bs, 0.001);
    }
    println!("{:.9}", energy(&bs));
}
