// TS 側のベンチ。Rust (../rust/src/main.rs)・Python (../python/bench.py) と同じデータ・同じ指標で測る。
//   node bench/ts/bench.ts   (実験ディレクトリから)
import { readFileSync } from "node:fs";
import { auc, MLP, mulberry32 } from "../../ts/src/nn.ts";

const DATA = new URL("../data/", import.meta.url);
const f32 = (name: string) => {
  const b = readFileSync(new URL(name, DATA));
  return new Float32Array(b.buffer, b.byteOffset, b.byteLength / 4);
};

interface Split { d: number; xtr: Float32Array; ytr: Float32Array; xte: Float32Array; yte: Float32Array }
const url: Split = { d: 256, xtr: f32("bloom_train_X.f32"), ytr: f32("bloom_train_y.f32"), xte: f32("bloom_test_X.f32"), yte: f32("bloom_test_y.f32") };
const cx = f32("cache_X.f32"), cy = f32("cache_y.f32");
const tr = (600_000 * 3) / 10;
const cache: Split = { d: 11, xtr: cx.subarray(0, tr * 11), ytr: cy.subarray(0, tr), xte: cx.subarray(tr * 11), yte: cy.subarray(tr) };

const fmt = (ns: number) => (ns >= 1000 ? `${(ns / 1000).toFixed(1)}µs` : `${ns.toFixed(0)}ns`);

function bench(task: string, s: Split, sizes: number[], epochs: number, batchSize: number) {
  const m = new MLP(sizes, mulberry32(1));
  const t0 = performance.now();
  m.fit(s.xtr, s.ytr, { epochs, batchSize, lr: 3e-3, rng: mulberry32(2) });
  const trainS = (performance.now() - t0) / 1000;
  const n = s.yte.length;
  const t1 = performance.now();
  const scores = new Float64Array(n);
  for (let i = 0; i < n; i++) scores[i] = m.predict(s.xte, i * s.d);
  const perRow = ((performance.now() - t1) * 1e6) / n;
  const name = sizes.length === 2 ? "own logreg (1 thread)" : `own MLP [${sizes.slice(1, -1)}] (1 thread)`;
  console.log(`${task.padEnd(6)} ${name.padEnd(28)} train ${trainS.toFixed(2).padStart(7)}s  AUC ${auc(scores, s.yte).toFixed(4)}  1-row ${fmt(perRow).padStart(9)}  batch ${fmt(perRow).padStart(9)}`);
}

console.log(`node ${process.versions.node}`);
bench("url", url, [256, 1], 8, 64);
bench("url", url, [256, 8, 1], 8, 64);
bench("cache", cache, [11, 1], 4, 128);
bench("cache", cache, [11, 16, 1], 4, 128);

// ---- キャッシュのヒット率用：先頭 30% で学習したモデルの、全 60 万行のスコアを書く (../rust の sim が読む)
if (process.argv.includes("--scores")) {
  const { writeFileSync } = await import("node:fs");
  for (const [name, sizes] of [["ts-own-logreg", [11, 1]], ["ts-own-mlp16", [11, 16, 1]]] as const) {
    const m = new MLP([...sizes], mulberry32(1));
    m.fit(cache.xtr, cache.ytr, { epochs: 4, batchSize: 128, lr: 3e-3, rng: mulberry32(2) });
    const s = new Float32Array(600_000);
    for (let i = 0; i < s.length; i++) s[i] = m.predict(cx, i * 11);
    writeFileSync(new URL(`scores/${name}.f32`, DATA), new Uint8Array(s.buffer));
    console.log(`wrote scores/${name}.f32`);
  }
}
