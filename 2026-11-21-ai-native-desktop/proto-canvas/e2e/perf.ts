// 性能: shell の 1 フレーム (展開・レイアウト・描画命令の生成) と、veriss の検証時間 (VALIDATION.md の 6)
// 使い方: node e2e/perf.ts
import { analyzeHTML } from '@mizchi/veriss/html';
import { verifyLayoutReport } from '@mizchi/veriss/z3/node';
import { createNotesApp } from '../src/apps/notes.ts';
import { createSettingsApp } from '../src/apps/settings.ts';
import { createTodoApp } from '../src/apps/todo.ts';
import { memoryStorage } from '../src/cache.ts';
import { layout } from '../src/layout.ts';
import { lower } from '../src/lower.ts';
import { Shell } from '../src/shell.ts';
import type { TileNode } from '../src/tiling.ts';

const cache = new Map<string, number>();
const measure = (s: string, size: number, weight: number) => {
  const k = `${size}/${weight}/${s}`;
  let v = cache.get(k);
  if (v === undefined) cache.set(k, (v = [...s].length * size * 0.6));
  return v;
};
const stats = (xs: number[]) => {
  const s = [...xs].sort((a, b) => a - b);
  return { mean: s.reduce((a, b) => a + b, 0) / s.length, p95: s[Math.floor(s.length * 0.95)], max: s.at(-1)! };
};
const f = (n: number) => n.toFixed(2).padStart(6);

console.log('## shell の 1 フレーム (ms)。1400x800、アニメーション中 = 境界を動かした直後から 60 フレーム');
console.log('構成                         | 静止 平均 / p95 | アニメ 平均 / p95 / 最大 | 描画命令');
for (const [label, todos, extra] of [
  ['窓 3 枚、Todo 3 件', 3, 0],
  ['窓 3 枚、Todo 1000 件', 1000, 0],
  ['窓 10 枚、各 20 件', 20, 7],
  ['窓 10 枚、Todo 1000 件', 1000, 7],
] as const) {
  const apps = [createTodoApp({ items: todos }), createSettingsApp(), createNotesApp(), ...Array.from({ length: extra }, (_, i) => createTodoApp({ id: `todo${i + 2}`, items: 20 }))];
  const shell = new Shell(apps, measure, memoryStorage());
  let t = 0;
  for (let i = 0; i < 10; i++) shell.frame(1400, 800, (t += 16)); // 温める
  const still: number[] = [];
  for (let i = 0; i < 60; i++) {
    const a = performance.now();
    shell.frame(1400, 800, (t += 16));
    still.push(performance.now() - a);
  }
  shell.tiles = { ...(shell.tiles as Extract<TileNode, { kind: 'split' }>), ratio: 0.25 };
  const anim: number[] = [];
  let ops = 0;
  for (let i = 0; i < 60; i++) {
    const a = performance.now();
    const fr = shell.frame(1400, 800, (t += 16));
    anim.push(performance.now() - a);
    ops = fr.ops.length;
  }
  const s1 = stats(still);
  const s2 = stats(anim);
  console.log(`${label.padEnd(26)} | ${f(s1.mean)} / ${f(s1.p95)} | ${f(s2.mean)} / ${f(s2.p95)} / ${f(s2.max)} | ${ops}`);
}

console.log('\n## veriss の検証時間 (ms)。Todo の既定の view、幅 252〜900px、高さ 300〜4000px');
for (const n of [3, 10, 30, 100]) {
  const shell = new Shell([createTodoApp({ items: n })], measure, memoryStorage());
  const { view } = shell.viewOf('todo');
  const min = layout(view, 0, measure).minWidth;
  const low = lower(view, measure, { pxIntrinsic: true });
  const a = performance.now();
  const r = analyzeHTML(low.html, { name: 'todo', css: low.css, boxes: low.boxes, integrity: true, accessibility: true, viewport: { min, max: 900, height: { min: 300, max: 4000 } } });
  const b = performance.now();
  const p = await verifyLayoutReport(r);
  const c = performance.now();
  console.log(`${String(n).padStart(4)} 件: analyze ${f(b - a)} / Z3 ${f(c - b)}  (${r.status} / ${p.status})`);
}
