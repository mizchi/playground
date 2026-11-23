// Learned Bloom filter (Kraska et al. 2018) の実験。
//
//   query(x) = model(x) >= τ  ||  backup.has(x)
//
// - backup には「モデルが取りこぼした鍵 (score < τ)」だけを入れる → 偽陰性ゼロは保証される
// - 偽陽性率は「モデルの FPR + 残りに対する backup の FPR」になる
// - モデルが鍵集合の"形"を覚えれば、backup に入れる鍵が激減してビット数が減る
import { auc, MLP, mulberry32 } from "./nn.ts";
import { BloomFilter, hashedNgrams } from "./bloom.ts";
import { benignUrl, maliciousUrl, uniqueSet } from "./urls.ts";

const D = 256;
const N_KEYS = 30_000;
const rng = mulberry32(42);

const keys = uniqueSet(rng, N_KEYS, maliciousUrl);
const keySet = new Set(keys);
const trainNeg = uniqueSet(rng, 30_000, benignUrl, keySet);
const valNeg = uniqueSet(rng, 30_000, benignUrl, keySet);
const testNeg = uniqueSet(rng, 100_000, benignUrl, keySet);
// 「怪しい見た目だが鍵集合には無い」問い合わせ (分布シフト / 敵対的クエリ)
const shiftedNeg = uniqueSet(rng, 20_000, maliciousUrl, keySet);

function featurize(xs: string[]): Float32Array {
  const X = new Float32Array(xs.length * D);
  xs.forEach((s, i) => hashedNgrams(s, D, X, i * D));
  return X;
}

const Xtrain = featurize([...keys, ...trainNeg]);
const ytrain = new Float32Array(keys.length + trainNeg.length).fill(1, 0, keys.length);
const feat = new Float32Array(D);
const score = (m: MLP, s: string) => (hashedNgrams(s, D, feat, 0), m.predict(feat));

const bloomBits = (n: number, fpr: number) => (n === 0 ? 0 : Math.ceil((-n * Math.log(fpr)) / (Math.LN2 * Math.LN2)));
const kb = (bits: number) => `${(bits / 8 / 1024).toFixed(1)} KB`;
const pct = (x: number) => `${(x * 100).toFixed(3)}%`;

function fprOf(q: (s: string) => boolean, xs: string[]) {
  let fp = 0;
  for (const s of xs) if (q(s)) fp++;
  return fp / xs.length;
}
function nsPerQuery(q: (s: string) => boolean, xs: string[]) {
  const t0 = process.hrtime.bigint();
  let c = 0;
  for (const s of xs) if (q(s)) c++;
  return Number(process.hrtime.bigint() - t0) / xs.length;
}

const configs: { name: string; sizes: number[]; epochs: number }[] = [
  { name: "logistic regression", sizes: [D, 1], epochs: 8 },
  { name: "MLP 256-8-1", sizes: [D, 8, 1], epochs: 8 },
  { name: "MLP 256-32-1", sizes: [D, 32, 1], epochs: 8 },
];

for (const target of [0.01, 0.001]) {
  const plain = BloomFilter.forCapacity(keys.length, target);
  for (const k of keys) plain.add(k);
  console.log(`\n=== target FPR ${pct(target)} / ${keys.length} keys ===`);
  console.log(`plain Bloom        : ${kb(plain.sizeBits).padStart(9)}  test FPR=${pct(fprOf((s) => plain.has(s), testNeg))}  shifted FPR=${pct(fprOf((s) => plain.has(s), shiftedNeg))}  ${nsPerQuery((s) => plain.has(s), testNeg).toFixed(0)} ns/q`);
}

for (const cfg of configs) {
  const model = new MLP(cfg.sizes, mulberry32(1));
  const t0 = Date.now();
  model.fit(Xtrain, ytrain, { epochs: cfg.epochs, batchSize: 64, lr: 3e-3, rng: mulberry32(2) });
  const keyScores = keys.map((s) => score(model, s));
  const valScores = valNeg.map((s) => score(model, s));
  const testAuc = auc([...keyScores, ...valScores], [...keyScores.map(() => 1), ...valScores.map(() => 0)]);
  const modelBits = model.paramCount() * 32; // float32 で保存する想定
  console.log(`\n--- ${cfg.name}: ${model.paramCount()} params (${kb(modelBits)} as f32), train ${Date.now() - t0} ms, AUC=${testAuc.toFixed(4)}`);

  const sortedVal = [...valScores].sort((a, b) => a - b);
  for (const target of [0.01, 0.001]) {
    // τ を候補 (検証用負例スコアの上位分位点) で走査し、総ビット数が最小になるものを選ぶ
    let best = { bits: Infinity, tau: 1, fm: 0, nb: 0, fb: 0 };
    for (let q = 0; q < 200; q++) {
      const fmTarget = target * (q / 200); // モデルに割り当てる FPR
      const idx = Math.min(sortedVal.length - 1, Math.floor(sortedVal.length * (1 - fmTarget)));
      const tau = sortedVal[idx] + 1e-12;
      const fm = valScores.filter((v) => v >= tau).length / valScores.length;
      if (fm >= target) continue;
      const nb = keyScores.filter((v) => v < tau).length;
      const fb = (target - fm) / (1 - fm);
      const bits = modelBits + bloomBits(nb, fb);
      if (bits < best.bits) best = { bits, tau, fm, nb, fb };
    }
    const backup = BloomFilter.forCapacity(best.nb, best.fb);
    keys.forEach((k, i) => { if (keyScores[i] < best.tau) backup.add(k); });
    const query = (s: string) => score(model, s) >= best.tau || backup.has(s);
    const fn = keys.filter((k) => !query(k)).length;
    const total = modelBits + backup.sizeBits;
    console.log(
      `  target ${pct(target)}: total ${kb(total).padStart(9)} (model ${kb(modelBits)} + backup ${kb(backup.sizeBits)} for ${best.nb} keys)` +
      `  test FPR=${pct(fprOf(query, testNeg))}  shifted FPR=${pct(fprOf(query, shiftedNeg))}  FN=${fn}  ${nsPerQuery(query, testNeg.slice(0, 20000)).toFixed(0)} ns/q`,
    );
  }
}
