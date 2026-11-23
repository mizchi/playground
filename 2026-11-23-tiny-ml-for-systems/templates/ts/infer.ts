// 製品の中での使い方: どの言語の雛形が書いた重み (JSON) でも読み込み、1 行ずつ同期で推論する。
//
//   node infer.ts <weights.json> [学習側が書いたスコア.f32]
//
// スコアを渡すと、学習した言語での予測と、ここ (TS) での予測の最大の差を出す (重みの受け渡しの確認)。
import { readFileSync } from "node:fs";
import { MLP } from "./tinyml.ts";

// TTL モデルの重みなら --ttl を付ける。比べる相手は学習側が書いた TTL (.f32)
const ttlMode = process.argv.includes("--ttl");
const [weights, scoresPath] = process.argv.slice(2).filter((a) => a !== "--ttl");
if (!weights) throw new Error("usage: node infer.ts <weights.json> [scores.f32]");
const model = MLP.fromJSON(readFileSync(weights, "utf8"));
const d = model.sizes[0];

// 製品のコードでは、ここが「判断」の 1 行になる
const admit = (features: Float32Array) => model.predict(features) > 0.5;

const data = new URL("../../bench/data/", import.meta.url).pathname;
const b = readFileSync(`${data}/cache_X.f32`);
const X = new Float32Array(b.buffer, b.byteOffset, b.byteLength / 4);
const rows = 100_000;
const row = new Float32Array(d);
const ours = new Float32Array(rows);
let admitted = 0;
const t0 = process.hrtime.bigint();
for (let i = 0; i < rows; i++) {
  row.set(X.subarray(i * d, (i + 1) * d));
  ours[i] = ttlMode ? Math.exp(model.predictLogit(row)) : model.predict(row);
  if (admit(row)) admitted++;
}
const ns = Number(process.hrtime.bigint() - t0) / rows / 2;
let line = `ts infer ${weights.split("/").slice(-3).join("/")}: ${rows} rows, ${ns.toFixed(0)} ns/row` + (ttlMode ? `, median TTL ${[...ours].sort((a, b) => a - b)[rows >> 1].toFixed(0)}` : `, admit ${((admitted / rows) * 100).toFixed(1)}%`);
if (scoresPath) {
  const s = readFileSync(scoresPath);
  const theirs = new Float32Array(s.buffer, s.byteOffset, s.byteLength / 4);
  let maxDiff = 0;
  // TTL は値の幅が大きいので相対誤差で比べる
  for (let i = 0; i < rows; i++) {
    const diff = Math.abs(ours[i] - theirs[i]);
    maxDiff = Math.max(maxDiff, ttlMode ? diff / Math.max(1e-9, Math.abs(theirs[i])) : diff);
  }
  line += `, max ${ttlMode ? "relative " : ""}|diff| vs trainer = ${maxDiff.toExponential(1)}`;
}
console.log(line);
