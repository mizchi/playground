// 雛形の使い方の例: キャッシュ admission のモデルを学習し、評価し、重みとスコアを書き出す。
//   node train_cache.ts [../../bench/data]
// データ・分け方・書き出し先は ../python/train_cache.py と同じ。
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { auc, MLP, mulberry32 } from "./tinyml.ts";

const data = resolve(process.argv[2] ?? new URL("../../bench/data", import.meta.url).pathname);
const f32 = (name: string) => {
  const b = readFileSync(`${data}/${name}`);
  return new Float32Array(b.buffer, b.byteOffset, b.byteLength / 4);
};
const n = 600_000, d = 11, train = (n * 3) / 10;
const X = f32("cache_X.f32"), y = f32("cache_y.f32");

const out = new URL("./out/", import.meta.url).pathname;
mkdirSync(out, { recursive: true });
mkdirSync(`${data}/scores`, { recursive: true });
for (const [name, sizes] of [["logreg", [d, 1]], ["mlp16", [d, 16, 1]]] as const) {
  const m = new MLP([...sizes], mulberry32(1));
  const t0 = performance.now();
  m.fit(X.subarray(0, train * d), y.subarray(0, train), { epochs: 4, batchSize: 128, lr: 3e-3, rng: mulberry32(2) });
  const trainS = (performance.now() - t0) / 1000;
  const scores = new Float32Array(n);
  for (let i = 0; i < n; i++) scores[i] = m.predict(X, i * d);
  console.log(`${name.padEnd(7)} ${String(m.paramCount()).padStart(4)} params  train ${trainS.toFixed(2)}s  test AUC ${auc(scores.subarray(train), y.subarray(train)).toFixed(4)}`);
  writeFileSync(`${out}/${name}.json`, JSON.stringify(m.toJSON()));
  writeFileSync(`${data}/scores/ts-template-${name}.f32`, new Uint8Array(scores.buffer));
}
console.log(`wrote out/*.json and ${data}/scores/ts-template-*.f32`);
