// onnxruntime-node で ../lgbm.onnx・../mlp16.onnx を推論し、速さと、Python で計算した予測との差を測る。
//   node bench.ts
// onnxruntime-node の run() は非同期 (Promise) なので、同期の判断の中では呼べない。ここでは 1 行ごとに await する。
import { readFileSync } from "node:fs";
import * as ort from "onnxruntime-node";

const here = new URL("../", import.meta.url).pathname;
const data = new URL("../../data/", import.meta.url).pathname;
const b = readFileSync(`${data}/cache_X.f32`);
const X = new Float32Array(b.buffer, b.byteOffset, b.byteLength / 4);
const d = 11, rows = 100_000;
const pkg = JSON.parse(readFileSync(new URL("./node_modules/onnxruntime-node/package.json", import.meta.url), "utf8"));

for (const name of ["lgbm", "mlp16"]) {
  const sess = await ort.InferenceSession.create(`${here}/${name}.onnx`, { intraOpNumThreads: 1 });
  const xs = X.subarray(0, rows * d);
  const runBatch = async () => (await sess.run({ x: new ort.Tensor("float32", xs, [rows, d]) })).p.data as Float32Array;
  await runBatch();
  let t0 = process.hrtime.bigint();
  const out = await runBatch();
  const batchNs = Number(process.hrtime.bigint() - t0) / rows;

  const k = 20_000;
  for (let i = 0; i < 100; i++) await sess.run({ x: new ort.Tensor("float32", xs.subarray(i * d, (i + 1) * d), [1, d]) });
  t0 = process.hrtime.bigint();
  for (let i = 0; i < k; i++) await sess.run({ x: new ort.Tensor("float32", xs.subarray(i * d, (i + 1) * d), [1, d]) });
  const oneNs = Number(process.hrtime.bigint() - t0) / k;

  const r = readFileSync(`${here}/ref_${name}.f32`);
  const ref = new Float32Array(r.buffer, r.byteOffset, r.byteLength / 4);
  let maxDiff = 0;
  for (let i = 0; i < rows; i++) maxDiff = Math.max(maxDiff, Math.abs(out[i] - ref[i]));
  console.log(`ts onnxruntime-node ${pkg.version} ${name}: 1-row ${oneNs.toFixed(0)} ns (await), batch ${batchNs.toFixed(0)} ns/row, max |diff| vs python ${maxDiff.toExponential(1)}`);
  console.log(`bench lang=ts-ort task=onnx model=${name} one_row_ns=${oneNs.toFixed(0)} batch_ns=${batchNs.toFixed(0)}`);
}
