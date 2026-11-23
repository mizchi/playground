// 手書きの逆伝搬が正しいかを数値微分と比べて確かめる (自作するなら必須)。
// ついでに XOR (線形分離不可能) が学習できることも確認する。
import { MLP, mulberry32 } from "./nn.ts";

const rng = mulberry32(1);
const d = 5, n = 8;
const X = new Float32Array(n * d).map(() => rng() * 2 - 1);
const y = new Float32Array(n).map(() => (rng() < 0.5 ? 1 : 0));
const net = new MLP([d, 7, 4, 1], rng);

const totalLoss = () => {
  let s = 0;
  for (let i = 0; i < n; i++) {
    const p = net.predict(X, i * d);
    s += -(y[i] * Math.log(p) + (1 - y[i]) * Math.log(1 - p));
  }
  return s;
};

net.zeroGrad();
for (let i = 0; i < n; i++) net.accumulate(X, i * d, y[i]);

let maxRel = 0;
const h = 1e-5;
const params = [...net.W.map((p, l) => [p, net.gW[l]] as const), ...net.b.map((p, l) => [p, net.gb[l]] as const)];
for (const [p, g] of params) {
  for (let i = 0; i < p.length; i++) {
    const orig = p[i];
    p[i] = orig + h; const lp = totalLoss();
    p[i] = orig - h; const lm = totalLoss();
    p[i] = orig;
    const num = (lp - lm) / (2 * h);
    const rel = Math.abs(num - g[i]) / Math.max(1e-8, Math.abs(num) + Math.abs(g[i]));
    maxRel = Math.max(maxRel, rel);
  }
}
console.log(`gradient check: max relative error = ${maxRel.toExponential(2)} (${net.paramCount()} params)`);
if (maxRel > 1e-5) { console.error("FAIL"); process.exit(1); }

// XOR
const xorX = new Float32Array([0, 0, 0, 1, 1, 0, 1, 1]);
const xorY = new Float32Array([0, 1, 1, 0]);
const xor = new MLP([2, 8, 1], mulberry32(7));
xor.fit(xorX, xorY, { epochs: 2000, batchSize: 4, lr: 0.02, rng });
const out = [0, 1, 2, 3].map((i) => xor.predict(xorX, i * 2).toFixed(3));
console.log(`XOR: ${out.join(" ")} (expected ~0 1 1 0)`);
const ok = [0, 1, 2, 3].every((i) => Math.round(xor.predict(xorX, i * 2)) === xorY[i]);
if (!ok) { console.error("FAIL"); process.exit(1); }
console.log("OK");
