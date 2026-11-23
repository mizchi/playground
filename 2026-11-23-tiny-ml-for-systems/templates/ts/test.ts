// 逆伝搬の勾配チェック、XOR、JSON の往復。  node test.ts
import { auc, MLP, mulberry32 } from "./tinyml.ts";

const rng = mulberry32(1);
const d = 5, n = 8;
const X = new Float32Array(n * d).map(() => rng() * 2 - 1);
const y = new Float32Array(n).map(() => (rng() < 0.5 ? 1 : 0));
const net = new MLP([d, 7, 4, 1], rng);
const loss = () => {
  let s = 0;
  for (let i = 0; i < n; i++) {
    const p = net.predict(X, i * d);
    s += -(y[i] * Math.log(p) + (1 - y[i]) * Math.log(1 - p));
  }
  return s;
};
net.zeroGrad();
for (let i = 0; i < n; i++) net.accumulate(X, i * d, y[i]);
let worst = 0;
const h = 1e-5;
const params = [...net.W.map((p, l) => [p, net.gW[l]] as const), ...net.b.map((p, l) => [p, net.gb[l]] as const)];
for (const [p, g] of params) {
  for (let i = 0; i < p.length; i++) {
    const o = p[i];
    p[i] = o + h; const lp = loss();
    p[i] = o - h; const lm = loss();
    p[i] = o;
    const num = (lp - lm) / (2 * h);
    worst = Math.max(worst, Math.abs(num - g[i]) / Math.max(1e-8, Math.abs(num) + Math.abs(g[i])));
  }
}
console.log(`gradient check: max relative error = ${worst.toExponential(2)} (${net.paramCount()} params)`);
if (worst > 1e-5) throw new Error("backprop is wrong");

const xorX = new Float32Array([0, 0, 0, 1, 1, 0, 1, 1]);
const xorY = new Float32Array([0, 1, 1, 0]);
const xor = new MLP([2, 8, 1], mulberry32(7));
xor.fit(xorX, xorY, { epochs: 2000, batchSize: 4, lr: 0.02, rng: mulberry32(1) });
const out = [0, 1, 2, 3].map((i) => xor.predict(xorX, i * 2));
console.log(`XOR: ${out.map((v) => v.toFixed(3)).join(" ")} (expected ~0 1 1 0)`);
if (!out.every((v, i) => Math.round(v) === xorY[i])) throw new Error("XOR failed");

if (Math.abs(auc([0.1, 0.4, 0.35, 0.8], [0, 0, 1, 1]) - 0.75) > 1e-12) throw new Error("auc");
const back = MLP.fromJSON(JSON.stringify(xor.toJSON()));
if (![0, 1, 2, 3].every((i) => Math.abs(back.predict(xorX, i * 2) - out[i]) < 1e-12)) throw new Error("json round trip");
console.log("OK");
