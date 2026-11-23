// フレームワーク無しの最小 MLP (多層パーセプトロン) + 誤差逆伝搬 + Adam。
//
// - 隠れ層: ReLU
// - 出力層: 1 ユニット + sigmoid (二値分類) / 損失: binary cross entropy
//   sigmoid と BCE を組み合わせると出力の誤差が δ = p - y という単純な形になる。
// - 隠れ層ゼロ (sizes = [d, 1]) にすればそのままロジスティック回帰。

export type Rng = () => number;

/** 決定的な 32bit PRNG。実験の再現性のために Math.random は使わない。 */
export function mulberry32(seed: number): Rng {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function gaussian(rng: Rng): number {
  const u = 1 - rng();
  const v = rng();
  return Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * v);
}

const sigmoid = (z: number) => 1 / (1 + Math.exp(-z));

export interface TrainOptions {
  epochs: number;
  batchSize?: number;
  lr?: number;
  l2?: number;
  /** 正例の重み (クラス不均衡対策)。 */
  posWeight?: number;
  rng: Rng;
  log?: boolean;
}

export class MLP {
  readonly sizes: number[];
  // W[l] は (out x in) の row-major。b[l] は out 長。
  readonly W: Float64Array[] = [];
  readonly b: Float64Array[] = [];
  // 勾配
  readonly gW: Float64Array[] = [];
  readonly gb: Float64Array[] = [];
  // Adam の1次/2次モーメント
  private mW: Float64Array[] = [];
  private vW: Float64Array[] = [];
  private mb: Float64Array[] = [];
  private vb: Float64Array[] = [];
  private t = 0;
  // forward のバッファ: acts[0] = 入力, acts[l+1] = 層 l の出力 (活性化後)
  private acts: Float64Array[] = [];
  private deltas: Float64Array[] = [];

  constructor(sizes: number[], rng: Rng) {
    if (sizes[sizes.length - 1] !== 1) throw new Error("output size must be 1");
    this.sizes = sizes;
    for (let l = 0; l < sizes.length - 1; l++) {
      const nin = sizes[l], nout = sizes[l + 1];
      const w = new Float64Array(nin * nout);
      const scale = Math.sqrt(2 / nin); // He 初期化 (ReLU 向け)
      for (let i = 0; i < w.length; i++) w[i] = gaussian(rng) * scale;
      this.W.push(w);
      this.b.push(new Float64Array(nout));
      for (const arr of [this.gW, this.mW, this.vW]) arr.push(new Float64Array(nin * nout));
      for (const arr of [this.gb, this.mb, this.vb]) arr.push(new Float64Array(nout));
      this.deltas.push(new Float64Array(nout));
    }
    for (const s of sizes) this.acts.push(new Float64Array(s));
  }

  paramCount(): number {
    let n = 0;
    for (let l = 0; l < this.W.length; l++) n += this.W[l].length + this.b[l].length;
    return n;
  }

  /** x (d 次元) → P(y=1)。offset で row-major 行列の行を直接指せる。 */
  predict(x: ArrayLike<number>, offset = 0): number {
    const a0 = this.acts[0];
    for (let i = 0; i < a0.length; i++) a0[i] = x[offset + i];
    const L = this.W.length;
    for (let l = 0; l < L; l++) {
      const nin = this.sizes[l], nout = this.sizes[l + 1];
      const w = this.W[l], bias = this.b[l], ain = this.acts[l], aout = this.acts[l + 1];
      for (let o = 0; o < nout; o++) {
        let z = bias[o];
        const row = o * nin;
        for (let i = 0; i < nin; i++) z += w[row + i] * ain[i];
        // 最終層は logit のまま残し、sigmoid は外で掛ける
        aout[o] = l < L - 1 ? (z > 0 ? z : 0) : z;
      }
    }
    return sigmoid(this.acts[L][0]);
  }

  zeroGrad(): void {
    for (const g of this.gW) g.fill(0);
    for (const g of this.gb) g.fill(0);
  }

  /**
   * 1 サンプル分の勾配を gW/gb に加算し、損失を返す。これが誤差逆伝搬の本体。
   *   δ_L = (p - y) * weight            (sigmoid + BCE の微分)
   *   ∂L/∂W_l = δ_l ⊗ a_l              (外積)
   *   δ_{l-1} = (W_lᵀ δ_l) ⊙ relu'(z_{l-1})
   */
  accumulate(x: ArrayLike<number>, offset: number, y: number, weight = 1): number {
    const p = this.predict(x, offset);
    const L = this.W.length;
    this.deltas[L - 1][0] = (p - y) * weight;
    for (let l = L - 1; l >= 0; l--) {
      const nin = this.sizes[l], nout = this.sizes[l + 1];
      const w = this.W[l], gw = this.gW[l], gb = this.gb[l];
      const ain = this.acts[l], delta = this.deltas[l];
      for (let o = 0; o < nout; o++) {
        const d = delta[o];
        if (d === 0) continue;
        gb[o] += d;
        const row = o * nin;
        for (let i = 0; i < nin; i++) gw[row + i] += d * ain[i];
      }
      if (l > 0) {
        const prev = this.deltas[l - 1];
        for (let i = 0; i < nin; i++) {
          // ain[i] > 0 ⇔ ReLU が通した (= 微分が 1)
          if (ain[i] <= 0) { prev[i] = 0; continue; }
          let s = 0;
          for (let o = 0; o < nout; o++) s += w[o * nin + i] * delta[o];
          prev[i] = s;
        }
      }
    }
    const eps = 1e-12;
    return -weight * (y * Math.log(p + eps) + (1 - y) * Math.log(1 - p + eps));
  }

  /** Adam 1 ステップ。g は (バッチ平均済みでない) 合計勾配なので scale で割る。 */
  step(lr: number, scale: number, l2 = 0): void {
    const b1 = 0.9, b2 = 0.999, eps = 1e-8;
    this.t++;
    const c1 = 1 - Math.pow(b1, this.t), c2 = 1 - Math.pow(b2, this.t);
    const upd = (p: Float64Array, g: Float64Array, m: Float64Array, v: Float64Array, decay: number) => {
      for (let i = 0; i < p.length; i++) {
        const gi = g[i] * scale + decay * p[i];
        m[i] = b1 * m[i] + (1 - b1) * gi;
        v[i] = b2 * v[i] + (1 - b2) * gi * gi;
        p[i] -= (lr * (m[i] / c1)) / (Math.sqrt(v[i] / c2) + eps);
      }
    };
    for (let l = 0; l < this.W.length; l++) {
      upd(this.W[l], this.gW[l], this.mW[l], this.vW[l], l2);
      upd(this.b[l], this.gb[l], this.mb[l], this.vb[l], 0);
    }
  }

  /** X: n x d の row-major, y: 0/1 ラベル。ミニバッチ Adam で学習する。 */
  fit(X: Float32Array, y: Float32Array, opt: TrainOptions): void {
    const d = this.sizes[0];
    const n = y.length;
    const bs = opt.batchSize ?? 64, lr = opt.lr ?? 1e-2, l2 = opt.l2 ?? 0;
    const pw = opt.posWeight ?? 1;
    const order = Array.from({ length: n }, (_, i) => i);
    for (let ep = 0; ep < opt.epochs; ep++) {
      // Fisher–Yates シャッフル
      for (let i = n - 1; i > 0; i--) {
        const j = Math.floor(opt.rng() * (i + 1));
        [order[i], order[j]] = [order[j], order[i]];
      }
      let loss = 0;
      for (let s = 0; s < n; s += bs) {
        this.zeroGrad();
        const e = Math.min(n, s + bs);
        for (let k = s; k < e; k++) {
          const i = order[k];
          loss += this.accumulate(X, i * d, y[i], y[i] > 0.5 ? pw : 1);
        }
        this.step(lr, 1 / (e - s), l2);
      }
      if (opt.log) console.log(`  epoch ${ep + 1}/${opt.epochs} loss=${(loss / n).toFixed(4)}`);
    }
  }

  toJSON() {
    return { sizes: this.sizes, W: this.W.map((w) => Array.from(w)), b: this.b.map((b) => Array.from(b)) };
  }
}

/** ROC AUC (順位統計で計算)。 */
export function auc(scores: ArrayLike<number>, labels: ArrayLike<number>): number {
  const idx = Array.from({ length: scores.length }, (_, i) => i).sort((a, b) => scores[a] - scores[b]);
  let pos = 0, neg = 0, rankSum = 0;
  for (let r = 0; r < idx.length; r++) {
    if (labels[idx[r]] > 0.5) { pos++; rankSum += r + 1; } else neg++;
  }
  return (rankSum - (pos * (pos + 1)) / 2) / (pos * neg);
}
