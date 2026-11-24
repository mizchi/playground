//! 依存ゼロの小さな機械学習部品。
//!
//! - [`Rng`]: 再現性のための決定的 PRNG
//! - [`Mlp`]: ReLU 隠れ層 + sigmoid 出力の MLP。誤差逆伝搬 + Adam (TS 版 `nn.ts` と同じ構造)
//! - [`QuantMlp`]: 学習後に重みを int8 に量子化した推論専用モデル (組み込み・カーネル向け)
//! - [`Bloom`]: 普通の Bloom filter
//! - [`hashed_ngrams`]: 文字 n-gram の feature hashing

pub mod urls;

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

// ------------------------------------------------------------------ Bloom filter
pub fn fnv1a(s: &[u8], seed: u32) -> u32 {
    let mut h = seed;
    for &c in s {
        h ^= c as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^ (h >> 16)
}

pub struct Bloom {
    bits: Vec<u64>,
    m: u32,
    k: u32,
}

impl Bloom {
    pub fn for_capacity(n: usize, fpr: f64) -> Self {
        if n == 0 {
            return Bloom { bits: vec![0], m: 1, k: 1 };
        }
        let ln2 = std::f64::consts::LN_2;
        let m = ((-(n as f64) * fpr.ln()) / (ln2 * ln2)).ceil().max(1.0);
        let k = ((m / n as f64) * ln2).round().max(1.0);
        Bloom { bits: vec![0; (m as usize).div_ceil(64)], m: m as u32, k: k as u32 }
    }
    fn pos(&self, key: &[u8]) -> impl Iterator<Item = u32> + '_ {
        let (h1, h2) = (fnv1a(key, 0x811c_9dc5), fnv1a(key, 0x9747_b28c) | 1);
        (0..self.k).map(move |i| h1.wrapping_add(i.wrapping_mul(h2)) % self.m)
    }
    pub fn add(&mut self, key: &[u8]) {
        let ps: Vec<u32> = self.pos(key).collect();
        for p in ps {
            self.bits[(p / 64) as usize] |= 1 << (p % 64);
        }
    }
    pub fn has(&self, key: &[u8]) -> bool {
        self.pos(key).all(|p| self.bits[(p / 64) as usize] >> (p % 64) & 1 == 1)
    }
    pub fn size_bits(&self) -> usize {
        self.m as usize
    }
}

/// 文字 3/4-gram を D 次元にハッシュ → log1p → L2 正規化。
pub fn hashed_ngrams(s: &str, out: &mut [f32]) {
    let d = out.len() as u32;
    out.fill(0.0);
    let mut t = Vec::with_capacity(s.len() + 2);
    t.push(b'^');
    t.extend_from_slice(s.as_bytes());
    t.push(b'$');
    for n in [3usize, 4] {
        for g in t.windows(n) {
            out[(fnv1a(g, n as u32) % d) as usize] += 1.0;
        }
    }
    let mut norm = 0.0;
    for v in out.iter_mut().filter(|v| **v != 0.0) {
        *v = v.ln_1p();
        norm += *v * *v;
    }
    let norm = norm.sqrt().max(1e-12);
    out.iter_mut().for_each(|v| *v /= norm);
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
}
