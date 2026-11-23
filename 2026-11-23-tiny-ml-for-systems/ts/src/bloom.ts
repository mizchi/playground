// 普通の Bloom filter と、文字 n-gram のハッシュ特徴量 (feature hashing)。

/** FNV-1a 32bit。seed を混ぜて複数の独立っぽいハッシュを作る。 */
export function fnv1a(s: string, seed = 0x811c9dc5): number {
  let h = seed >>> 0;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  // murmur3 の finalizer で下位ビットを混ぜる
  h ^= h >>> 16; h = Math.imul(h, 0x85ebca6b);
  h ^= h >>> 13; h = Math.imul(h, 0xc2b2ae35);
  h ^= h >>> 16;
  return h >>> 0;
}

export class BloomFilter {
  readonly m: number;
  readonly k: number;
  private bits: Uint32Array;

  constructor(m: number, k: number) {
    this.m = Math.max(1, Math.ceil(m));
    this.k = Math.max(1, Math.round(k));
    this.bits = new Uint32Array(Math.ceil(this.m / 32));
  }

  /** n 要素で偽陽性率 fpr を狙う最適な m, k。 */
  static forCapacity(n: number, fpr: number): BloomFilter {
    if (n === 0) return new BloomFilter(1, 1);
    const m = Math.ceil((-n * Math.log(fpr)) / (Math.LN2 * Math.LN2));
    return new BloomFilter(m, (m / n) * Math.LN2);
  }

  private *positions(key: string) {
    // double hashing: h1 + i*h2 (Kirsch–Mitzenmacher)
    const h1 = fnv1a(key), h2 = fnv1a(key, 0x9747b28c) | 1;
    for (let i = 0; i < this.k; i++) yield ((h1 + Math.imul(i, h2)) >>> 0) % this.m;
  }

  add(key: string): void {
    for (const p of this.positions(key)) this.bits[p >>> 5] |= 1 << (p & 31);
  }

  has(key: string): boolean {
    for (const p of this.positions(key)) if ((this.bits[p >>> 5] & (1 << (p & 31))) === 0) return false;
    return true;
  }

  get sizeBits(): number { return this.m; }
}

/**
 * 文字列 → 固定長ベクトル。文字 3-gram / 4-gram を D 個のバケツにハッシュして数え、
 * log1p で潰して L2 正規化する。辞書を持たないのでメモリ 0 で任意の文字列を扱える。
 */
export function hashedNgrams(s: string, D: number, out: Float32Array, offset: number): void {
  out.fill(0, offset, offset + D);
  const t = `^${s}$`;
  for (const n of [3, 4]) {
    for (let i = 0; i + n <= t.length; i++) {
      const h = fnv1a(t.slice(i, i + n), n);
      out[offset + (h % D)] += 1;
    }
  }
  let norm = 0;
  for (let i = 0; i < D; i++) {
    if (out[offset + i] === 0) continue;
    const v = Math.log1p(out[offset + i]);
    out[offset + i] = v;
    norm += v * v;
  }
  norm = Math.sqrt(norm) || 1;
  for (let i = 0; i < D; i++) out[offset + i] /= norm;
}
