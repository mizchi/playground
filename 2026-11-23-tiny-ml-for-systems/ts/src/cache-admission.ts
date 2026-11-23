// キャッシュの「入れるかどうか (admission)」を小さな MLP で判断する実験。
// LRB (NSDI'20) / Baleen (FAST'24) と同じ発想を極小化したもの。
//
// ラベル = 「このオブジェクトは H リクエスト以内にもう一度来るか」。
// これはトレースを後から見れば自動で付く (人手のアノテーション不要) のが重要なポイント。
import { auc, MLP, mulberry32, type Rng } from "./nn.ts";

const rng = mulberry32(2026);
const T = 600_000;
const TRAIN_FRAC = 0.3;

// ---------------------------------------------------------------- trace
const KINDS = ["html", "image", "video", "api"] as const;
const CLIENTS = ["browser", "app", "bot"] as const;
type Kind = 0 | 1 | 2 | 3;
type Client = 0 | 1 | 2;
interface Req { id: number; kind: Kind; size: number; client: Client }

const pickW = (r: Rng, w: number[]) => {
  let x = r() * w.reduce((a, b) => a + b, 0);
  for (let i = 0; i < w.length; i++) if ((x -= w[i]) < 0) return i;
  return w.length - 1;
};
const lognormal = (r: Rng, mu: number, sigma: number) => {
  const u = 1 - r(), v = r();
  return Math.exp(mu + sigma * Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * v));
};
const SIZE_MU = [Math.log(20e3), Math.log(60e3), Math.log(800e3), Math.log(2e3)];
const objSize = (r: Rng, k: Kind) => Math.max(200, Math.round(lognormal(r, SIZE_MU[k], 0.8)));

function genTrace(r: Rng): Req[] {
  // (1) Zipf 人気の固定カタログ
  const C = 30_000, alpha = 0.8;
  const cdf = new Float64Array(C);
  let acc = 0;
  for (let i = 0; i < C; i++) cdf[i] = acc += 1 / Math.pow(i + 1, alpha);
  const cat = Array.from({ length: C }, () => {
    const kind = pickW(r, [3, 4, 1, 2]) as Kind;
    return { kind, size: objSize(r, kind) };
  });
  // (3) バースト (ニュース的): 生まれて短期間に何度か参照され、消える
  const burst: { id: number; kind: Kind; size: number; left: number }[] = [];
  let nextId = C;
  const out: Req[] = [];
  for (let t = 0; t < T; t++) {
    const u = r();
    if (u < 0.5) {
      const x = r() * acc;
      let lo = 0, hi = C - 1;
      while (lo < hi) { const m = (lo + hi) >> 1; if (cdf[m] < x) lo = m + 1; else hi = m; }
      out.push({ id: lo, ...cat[lo], client: pickW(r, [6, 3.5, 0.5]) as Client });
    } else if (u < 0.82) {
      // (2) one-hit wonder: 二度と来ない (クローラ・ロングテール)。bot が多いが browser も混ざる
      const kind = pickW(r, [2, 4, 2, 2]) as Kind;
      out.push({ id: nextId++, kind, size: objSize(r, kind), client: pickW(r, [3, 1, 6]) as Client });
    } else {
      if (burst.length === 0 || r() < 0.12) {
        const kind = pickW(r, [5, 4, 1, 0]) as Kind;
        burst.push({ id: nextId++, kind, size: objSize(r, kind), left: 1 + Math.floor(-Math.log(1 - r()) * 12) });
      }
      const i = Math.floor(r() * burst.length);
      const b = burst[i];
      out.push({ id: b.id, kind: b.kind, size: b.size, client: pickW(r, [5, 5, 0.3]) as Client });
      if (--b.left <= 0) { burst[i] = burst[burst.length - 1]; burst.pop(); }
    }
  }
  return out;
}

const trace = genTrace(rng);

// 各リクエストについて「同じ id が次に来る時刻」(未来を見て付けるラベル)
const nextUse = new Int32Array(T).fill(-1);
{
  const last = new Map<number, number>();
  for (let t = T - 1; t >= 0; t--) {
    const id = trace[t].id;
    const n = last.get(id);
    if (n !== undefined) nextUse[t] = n;
    last.set(id, t);
  }
}

// ---------------------------------------------------------------- features
// 実システムでは count / lastSeen は count-min sketch 等の近似で持つ。ここでは Map で簡略化。
const NF = 11;
interface Hist { count: number; last: number }
function features(req: Req, h: Hist | undefined, t: number, out: Float32Array, off: number) {
  out.fill(0, off, off + NF);
  out[off + req.kind] = 1;
  out[off + 4 + req.client] = 1;
  out[off + 7] = Math.log2(req.size) / 24;
  out[off + 8] = h ? Math.log1p(h.count) / 6 : 0;
  out[off + 9] = h ? 1 : 0;
  out[off + 10] = h ? Math.log1p(t - h.last) / 14 : 0;
}

// ---------------------------------------------------------------- cache sim
class LRUCache {
  private map = new Map<number, number>(); // id -> size (Map は挿入順を保つので LRU に使える)
  used = 0;
  readonly capacity: number;
  constructor(capacity: number) { this.capacity = capacity; }
  get(id: number): boolean {
    const s = this.map.get(id);
    if (s === undefined) return false;
    this.map.delete(id); this.map.set(id, s);
    return true;
  }
  put(id: number, size: number) {
    if (size > this.capacity) return;
    while (this.used + size > this.capacity) {
      const [vid, vs] = this.map.entries().next().value as [number, number];
      this.map.delete(vid); this.used -= vs;
    }
    this.map.set(id, size); this.used += size;
  }
}

type Admit = (req: Req, h: Hist | undefined, t: number) => boolean;

function simulate(capacity: number, admit: Admit) {
  const cache = new LRUCache(capacity);
  const hist = new Map<number, Hist>();
  const start = Math.floor(T * TRAIN_FRAC);
  let hits = 0, reqs = 0, hitBytes = 0, bytes = 0;
  for (let t = 0; t < T; t++) {
    const req = trace[t];
    const h = hist.get(req.id);
    const hit = cache.get(req.id);
    if (!hit && admit(req, h, t)) cache.put(req.id, req.size);
    if (t >= start) { reqs++; bytes += req.size; if (hit) { hits++; hitBytes += req.size; } }
    if (h) { h.count++; h.last = t; } else hist.set(req.id, { count: 1, last: t });
  }
  return { ohr: hits / reqs, bhr: hitBytes / bytes };
}

// ---------------------------------------------------------------- main
const catalogBytes = (() => {
  const seen = new Map<number, number>();
  for (const r of trace) seen.set(r.id, r.size);
  let s = 0; for (const v of seen.values()) s += v; return s;
})();

const pct = (x: number) => `${(x * 100).toFixed(2)}%`.padStart(7);

for (const capFrac of [0.005, 0.02]) {
  const capacity = Math.floor(catalogBytes * capFrac);
  // ラベルの地平線 H: キャッシュ容量 ÷ 平均オブジェクトサイズ ≒ LRU で生き残れるリクエスト数の目安
  const meanReqSize = trace.reduce((s, r) => s + r.size, 0) / T;
  const H = Math.round(capacity / meanReqSize);

  // DUMP_DIR=... のときは、全リクエストの特徴量とラベルを書き出して終わる (bench/ の Rust・Python 比較用)
  if (process.env.DUMP_DIR) {
    const { writeFileSync } = await import("node:fs");
    const X = new Float32Array(T * NF), y = new Float32Array(T);
    const hist = new Map<number, Hist>();
    for (let t = 0; t < T; t++) {
      const req = trace[t];
      const h = hist.get(req.id);
      features(req, h, t, X, t * NF);
      y[t] = nextUse[t] >= 0 && nextUse[t] - t <= H ? 1 : 0;
      if (h) { h.count++; h.last = t; } else hist.set(req.id, { count: 1, last: t });
    }
    const dir = process.env.DUMP_DIR;
    writeFileSync(`${dir}/cache_X.f32`, new Uint8Array(X.buffer));
    writeFileSync(`${dir}/cache_y.f32`, new Uint8Array(y.buffer));
    writeFileSync(`${dir}/cache_id.u32`, new Uint8Array(Uint32Array.from(trace, (r) => r.id).buffer));
    writeFileSync(`${dir}/cache_size.u32`, new Uint8Array(Uint32Array.from(trace, (r) => r.size).buffer));
    writeFileSync(`${dir}/cache.json`, JSON.stringify({ n: T, d: NF, train: Math.floor(T * TRAIN_FRAC), H, capacity }));
    console.log(`dumped ${T} x ${NF} to ${dir} (H=${H})`);
    process.exit(0);
  }

  // 学習データ: 先頭 30% のミス時点の特徴量とラベル
  const n = Math.floor(T * TRAIN_FRAC);
  const X = new Float32Array(n * NF), y = new Float32Array(n);
  const hist = new Map<number, Hist>();
  for (let t = 0; t < n; t++) {
    const req = trace[t];
    const h = hist.get(req.id);
    features(req, h, t, X, t * NF);
    y[t] = nextUse[t] >= 0 && nextUse[t] - t <= H ? 1 : 0;
    if (h) { h.count++; h.last = t; } else hist.set(req.id, { count: 1, last: t });
  }
  const model = new MLP([NF, 16, 1], mulberry32(3));
  const t0 = Date.now();
  model.fit(X, y, { epochs: 4, batchSize: 128, lr: 3e-3, rng: mulberry32(4) });
  const scores = Array.from({ length: n }, (_, i) => model.predict(X, i * NF));
  const posRate = y.reduce((a, b) => a + b, 0) / n;

  console.log(`\n=== cache = ${(capFrac * 100).toFixed(1)}% of unique bytes (${(capacity / 1e6).toFixed(0)} MB), H=${H} reqs ===`);
  console.log(`model: ${model.paramCount()} params, train ${Date.now() - t0} ms, train AUC=${auc(scores, y).toFixed(3)}, positive rate=${pct(posRate)}`);

  const feat = new Float32Array(NF);
  const policies: [string, Admit][] = [
    ["LRU (admit all)", () => true],
    ["LRU + doorkeeper (2nd hit)", (_r, h) => h !== undefined],
    ["LRU + MLP admission p>0.5", (r, h, t) => (features(r, h, t, feat, 0), model.predict(feat) > 0.5)],
    ["LRU + oracle admission", (_r, _h, t) => nextUse[t] >= 0 && nextUse[t] - t <= H],
  ];
  console.log(`${"policy".padEnd(30)}   OHR      BHR`);
  for (const [name, admit] of policies) {
    const r = simulate(capacity, admit);
    console.log(`${name.padEnd(30)} ${pct(r.ohr)}  ${pct(r.bhr)}`);
  }
}
