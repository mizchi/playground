//! 小さいモデルを自前で作るための雛形 (Rust 版)。依存ゼロ。
//!
//! ../../rust/src/lib.rs の RNG・MLP・int8 量子化をコピーし、JSON の読み書きを足したもの。
//! TS (../ts/tinyml.ts)・Python (../python/tinyml.py)・Julia (../julia/TinyML.jl) と同じ構造と JSON 形式:
//!   {"sizes":[...],"W":[[(out,in) の row-major]],"b":[[...]]}

// ------------------------------------------------------------------ RNG
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9e37_79b9_7f4a_7c15)
    }
    /// splitmix64
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// [0, 1)
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.f64() * n as f64) as usize
    }
    pub fn gaussian(&mut self) -> f64 {
        let u = 1.0 - self.f64();
        let v = self.f64();
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

// ------------------------------------------------------------------ MLP
fn sigmoid(z: f32) -> f32 {
    1.0 / (1.0 + (-z).exp())
}

/// 出力 1 ユニットの二値分類 MLP。`sizes = [d, 1]` ならロジスティック回帰。
pub struct Mlp {
    pub sizes: Vec<usize>,
    pub w: Vec<Vec<f32>>, // w[l]: (out x in) row-major
    pub b: Vec<Vec<f32>>,
    gw: Vec<Vec<f32>>,
    gb: Vec<Vec<f32>>,
    m: Vec<Vec<f32>>, // Adam state (w と b を交互に並べる)
    v: Vec<Vec<f32>>,
    t: i32,
    acts: Vec<Vec<f32>>,
    deltas: Vec<Vec<f32>>,
}

impl Mlp {
    pub fn new(sizes: &[usize], rng: &mut Rng) -> Self {
        assert_eq!(*sizes.last().unwrap(), 1);
        let mut net = Mlp {
            sizes: sizes.to_vec(),
            w: vec![], b: vec![], gw: vec![], gb: vec![], m: vec![], v: vec![], t: 0,
            acts: sizes.iter().map(|&s| vec![0.0; s]).collect(),
            deltas: vec![],
        };
        for l in 0..sizes.len() - 1 {
            let (nin, nout) = (sizes[l], sizes[l + 1]);
            let scale = (2.0 / nin as f64).sqrt(); // He 初期化
            net.w.push((0..nin * nout).map(|_| (rng.gaussian() * scale) as f32).collect());
            net.b.push(vec![0.0; nout]);
            net.gw.push(vec![0.0; nin * nout]);
            net.gb.push(vec![0.0; nout]);
            for len in [nin * nout, nout] {
                net.m.push(vec![0.0; len]);
                net.v.push(vec![0.0; len]);
            }
            net.deltas.push(vec![0.0; nout]);
        }
        net
    }

    pub fn param_count(&self) -> usize {
        self.w.iter().map(Vec::len).sum::<usize>() + self.b.iter().map(Vec::len).sum::<usize>()
    }

    pub fn predict(&mut self, x: &[f32]) -> f32 {
        self.acts[0].copy_from_slice(x);
        let layers = self.w.len();
        for l in 0..layers {
            let (nin, nout) = (self.sizes[l], self.sizes[l + 1]);
            let (head, tail) = self.acts.split_at_mut(l + 1);
            let (ain, aout) = (&head[l], &mut tail[0]);
            for o in 0..nout {
                let row = &self.w[l][o * nin..(o + 1) * nin];
                let z = self.b[l][o] + row.iter().zip(ain).map(|(w, a)| w * a).sum::<f32>();
                aout[o] = if l < layers - 1 { z.max(0.0) } else { z };
            }
        }
        sigmoid(self.acts[layers][0])
    }

    /// sigmoid をかける前の出力。TTL モデル (log TTL を出す) は predict_logit(x).exp() で TTL になる
    pub fn predict_logit(&mut self, x: &[f32]) -> f32 {
        self.predict(x);
        self.acts[self.w.len()][0]
    }

    /// 1 サンプル分の勾配を加算 (誤差逆伝搬)。δ_L = p - y, δ_{l-1} = (W_lᵀ δ_l) ⊙ relu'
    pub fn accumulate(&mut self, x: &[f32], y: f32) -> f32 {
        let p = self.predict(x);
        let layers = self.w.len();
        self.deltas[layers - 1][0] = p - y;
        for l in (0..layers).rev() {
            let (nin, nout) = (self.sizes[l], self.sizes[l + 1]);
            for o in 0..nout {
                let d = self.deltas[l][o];
                self.gb[l][o] += d;
                let g = &mut self.gw[l][o * nin..(o + 1) * nin];
                for (gi, a) in g.iter_mut().zip(&self.acts[l]) {
                    *gi += d * a;
                }
            }
            if l > 0 {
                let (lo, hi) = self.deltas.split_at_mut(l);
                for i in 0..nin {
                    lo[l - 1][i] = if self.acts[l][i] > 0.0 {
                        (0..nout).map(|o| self.w[l][o * nin + i] * hi[0][o]).sum()
                    } else {
                        0.0
                    };
                }
            }
        }
        -(y * (p + 1e-7).ln() + (1.0 - y) * (1.0 - p + 1e-7).ln())
    }

    pub fn step(&mut self, lr: f32, scale: f32) {
        let (b1, b2, eps) = (0.9f32, 0.999f32, 1e-8f32);
        self.t += 1;
        let (c1, c2) = (1.0 - b1.powi(self.t), 1.0 - b2.powi(self.t));
        let params = self.w.iter_mut().zip(self.gw.iter_mut()).zip(self.b.iter_mut().zip(self.gb.iter_mut()));
        let mut k = 0;
        for ((w, gw), (b, gb)) in params {
            for (p, g) in [(w, gw), (b, gb)] {
                let (m, v) = (&mut self.m[k], &mut self.v[k]);
                for i in 0..p.len() {
                    let gi = g[i] * scale;
                    m[i] = b1 * m[i] + (1.0 - b1) * gi;
                    v[i] = b2 * v[i] + (1.0 - b2) * gi * gi;
                    p[i] -= lr * (m[i] / c1) / ((v[i] / c2).sqrt() + eps);
                    g[i] = 0.0;
                }
                k += 1;
            }
        }
    }

    /// x: n x d row-major
    pub fn fit(&mut self, x: &[f32], y: &[f32], epochs: usize, batch: usize, lr: f32, rng: &mut Rng) {
        let d = self.sizes[0];
        let mut order: Vec<usize> = (0..y.len()).collect();
        for _ in 0..epochs {
            for i in (1..order.len()).rev() {
                order.swap(i, rng.below(i + 1));
            }
            for chunk in order.chunks(batch) {
                for &i in chunk {
                    self.accumulate(&x[i * d..(i + 1) * d], y[i]);
                }
                self.step(lr, 1.0 / chunk.len() as f32);
            }
        }
    }
}

// ------------------------------------------------------------------ int8 量子化
/// 層ごとに対称量子化: w ≈ q * scale, q ∈ [-127, 127]。
/// 重み 1 個 1 byte になるので f32 比で 1/4。推論は i8 を読みながら f32 で積和する。
pub struct QuantMlp {
    sizes: Vec<usize>,
    q: Vec<Vec<i8>>,
    scale: Vec<f32>,
    b: Vec<Vec<f32>>,
    buf: [Vec<f32>; 2],
}

impl QuantMlp {
    pub fn from(m: &Mlp) -> Self {
        let mut q = vec![];
        let mut scale = vec![];
        for w in &m.w {
            let maxabs = w.iter().fold(0f32, |a, &x| a.max(x.abs())).max(1e-12);
            let s = maxabs / 127.0;
            q.push(w.iter().map(|&x| (x / s).round().clamp(-127.0, 127.0) as i8).collect());
            scale.push(s);
        }
        let maxw = *m.sizes.iter().max().unwrap();
        QuantMlp { sizes: m.sizes.clone(), q, scale, b: m.b.clone(), buf: [vec![0.0; maxw], vec![0.0; maxw]] }
    }

    /// 保存に必要なバイト数 (i8 重み + f32 バイアス + f32 スケール)
    pub fn size_bytes(&self) -> usize {
        self.q.iter().map(Vec::len).sum::<usize>() + 4 * (self.b.iter().map(Vec::len).sum::<usize>() + self.scale.len())
    }

    pub fn predict(&mut self, x: &[f32]) -> f32 {
        let layers = self.q.len();
        let [a, b] = &mut self.buf;
        a[..x.len()].copy_from_slice(x);
        let (mut cur, mut nxt) = (a, b);
        for l in 0..layers {
            let (nin, nout) = (self.sizes[l], self.sizes[l + 1]);
            for o in 0..nout {
                let row = &self.q[l][o * nin..(o + 1) * nin];
                let acc: f32 = row.iter().zip(&cur[..nin]).map(|(&w, &a)| w as f32 * a).sum();
                let z = self.b[l][o] + acc * self.scale[l];
                nxt[o] = if l < layers - 1 { z.max(0.0) } else { z };
            }
            std::mem::swap(&mut cur, &mut nxt);
        }
        sigmoid(cur[0])
    }
}

// ------------------------------------------------------------------ JSON (この形式専用の小さな読み書き)
impl Mlp {
    /// TS の MLP.toJSON() と同じ形で書く
    pub fn to_json(&self) -> String {
        let nums = |v: &[f32]| v.iter().map(|x| format!("{x:e}")).collect::<Vec<_>>().join(",");
        let arr = |vs: &[Vec<f32>]| vs.iter().map(|v| format!("[{}]", nums(v))).collect::<Vec<_>>().join(",");
        let sizes = self.sizes.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",");
        format!("{{\"sizes\":[{sizes}],\"W\":[{}],\"b\":[{}]}}", arr(&self.w), arr(&self.b))
    }

    /// どの言語の雛形が書いた重みでも読める (この形式だけを想定した最小の読み取り)
    pub fn from_json(s: &str) -> Result<Mlp, String> {
        fn section<'a>(s: &'a str, key: &str) -> Result<&'a str, String> {
            let start = s.find(&format!("\"{key}\"")).ok_or(format!("missing {key}"))?;
            let rest = &s[start..];
            let open = rest.find('[').ok_or("no [")?;
            let mut depth = 0;
            for (i, c) in rest[open..].char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(&rest[open..open + i + 1]);
                        }
                    }
                    _ => {}
                }
            }
            Err(format!("unclosed {key}"))
        }
        fn numbers(s: &str) -> Result<Vec<f32>, String> {
            s.split(|c: char| c == ',' || c == '[' || c == ']' || c.is_whitespace())
                .filter(|t| !t.is_empty())
                .map(|t| t.parse::<f32>().map_err(|e| format!("{t}: {e}")))
                .collect()
        }
        fn rows(s: &str) -> Result<Vec<Vec<f32>>, String> {
            let inner = &s[1..s.len() - 1];
            inner.split(']').map(|p| p.trim_start_matches(',').trim()).filter(|p| !p.is_empty()).map(numbers).collect()
        }
        let sizes: Vec<usize> = numbers(section(s, "sizes")?)?.iter().map(|&v| v as usize).collect();
        let mut m = Mlp::new(&sizes, &mut Rng::new(0));
        let w = rows(section(s, "W")?)?;
        let b = rows(section(s, "b")?)?;
        for l in 0..m.w.len() {
            if w[l].len() != m.w[l].len() || b[l].len() != m.b[l].len() {
                return Err(format!("layer {l}: shape mismatch"));
            }
            m.w[l].copy_from_slice(&w[l]);
            m.b[l].copy_from_slice(&b[l]);
        }
        Ok(m)
    }
}

/// ROC AUC (順位統計)
pub fn auc(scores: &[f32], labels: &[f32]) -> f64 {
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    idx.sort_by(|&a, &b| scores[a].partial_cmp(&scores[b]).unwrap());
    let (mut pos, mut neg, mut rank_sum) = (0f64, 0f64, 0f64);
    for (r, &i) in idx.iter().enumerate() {
        if labels[i] > 0.5 {
            pos += 1.0;
            rank_sum += (r + 1) as f64;
        } else {
            neg += 1.0;
        }
    }
    (rank_sum - pos * (pos + 1.0) / 2.0) / (pos * neg)
}

/// little-endian の f32 ファイル (bench/data の形式)
pub fn read_f32(path: &str) -> Vec<f32> {
    std::fs::read(path).unwrap().chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}
pub fn write_f32(path: &str, xs: &[f32]) {
    std::fs::write(path, xs.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 解析的勾配 vs 数値微分 (f64 で計算しないので許容誤差は緩め)
    #[test]
    fn gradient_check() {
        let mut rng = Rng::new(1);
        let (d, n) = (4, 6);
        let x: Vec<f32> = (0..n * d).map(|_| rng.f64() as f32 * 2.0 - 1.0).collect();
        let y: Vec<f32> = (0..n).map(|i| (i % 2) as f32).collect();
        let mut net = Mlp::new(&[d, 5, 3, 1], &mut rng);
        for i in 0..n {
            net.accumulate(&x[i * d..(i + 1) * d], y[i]);
        }
        let grads = net.gw.clone();
        let loss = |net: &mut Mlp| -> f64 {
            (0..n)
                .map(|i| {
                    let p = net.predict(&x[i * d..(i + 1) * d]) as f64;
                    -(y[i] as f64 * p.ln() + (1.0 - y[i] as f64) * (1.0 - p).ln())
                })
                .sum()
        };
        let h = 1e-2f32;
        for l in 0..net.w.len() {
            for i in 0..net.w[l].len() {
                let orig = net.w[l][i];
                net.w[l][i] = orig + h;
                let lp = loss(&mut net);
                net.w[l][i] = orig - h;
                let lm = loss(&mut net);
                net.w[l][i] = orig;
                let num = (lp - lm) / (2.0 * h as f64);
                let g = grads[l][i] as f64;
                assert!((num - g).abs() < 1e-2 * (1.0 + num.abs()), "layer {l} idx {i}: num={num} analytic={g}");
            }
        }
    }

    #[test]
    fn learns_xor() {
        let mut rng = Rng::new(7);
        let x = [0., 0., 0., 1., 1., 0., 1., 1.];
        let y = [0., 1., 1., 0.];
        let mut net = Mlp::new(&[2, 8, 1], &mut rng);
        net.fit(&x, &y, 2000, 4, 0.02, &mut rng);
        for i in 0..4 {
            assert_eq!(net.predict(&x[i * 2..i * 2 + 2]).round(), y[i]);
        }
    }

    #[test]
    fn json_round_trip() {
        let mut rng = Rng::new(3);
        let mut m = Mlp::new(&[3, 4, 1], &mut rng);
        let x = [0.1, -0.2, 0.3];
        let p = m.predict(&x);
        let mut back = Mlp::from_json(&m.to_json()).unwrap();
        assert_eq!(back.predict(&x), p);
    }
}
