// mizchi/veriss に報告した、accessibility と縦方向の検査を併用したときの解析時間の再現。
// test/fixtures/veriss-slow-todo20.json は、試作の Todo (20 件) を lowering した HTML・CSS・文字の契約そのもの。
// 使い方: node e2e/veriss-perf-repro.mjs   (Node 24 以上)
import fs from 'node:fs';
import { analyzeHTML } from '@mizchi/veriss/html';

const low = JSON.parse(fs.readFileSync(new URL('../test/fixtures/veriss-slow-todo20.json', import.meta.url), 'utf8'));
for (const accessibility of [false, true]) {
  for (const height of [{ min: 300, max: 1200 }, { min: 1300, max: 1600 }]) {
    const t = performance.now();
    const r = analyzeHTML(low.html, { name: 'todo', css: low.css, boxes: low.boxes, accessibility, viewport: { min: 252, max: 900, height } });
    console.log(`accessibility=${accessibility} height=${height.min}-${height.max}: ${(performance.now() - t).toFixed(0)}ms ${r.status}`);
  }
}
