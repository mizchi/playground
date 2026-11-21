// V1: 既定レンダラの view を veriss で検証する (Node 24 以上)。
// 試作の layout.ts が出す最小幅と、veriss が証明する「崩れない幅の下限」が一致することを確かめる。
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { analyzeHTML } from '@mizchi/veriss/html';
import { verifyLayoutReport } from '@mizchi/veriss/z3/node';
import { createSettingsApp } from '../src/apps/settings.ts';
import { createTodoApp } from '../src/apps/todo.ts';
import { memoryStorage } from '../src/cache.ts';
import { layout } from '../src/layout.ts';
import { lower } from '../src/lower.ts';
import { Shell } from '../src/shell.ts';

// 決定的な計測 (veriss には契約として渡すので、値そのものの正しさは問わない)
const measure = (s: string, size: number, weight: number) => [...s].length * size * (weight >= 600 ? 0.66 : 0.6);
const HEIGHT = { min: 300, max: 1200 };

for (const app of ['todo', 'settings']) {
  test(`veriss: ${app} の既定 view は、layout.ts の最小幅以上で verified、未満で violated`, async () => {
    const shell = new Shell([createTodoApp(), createSettingsApp()], measure, memoryStorage());
    const { view } = shell.viewOf(app);
    const min = layout(view, 0, measure).minWidth;
    const low = lower(view, measure, { pxIntrinsic: true });
    const opts = { name: app, css: low.css, boxes: low.boxes, integrity: true, accessibility: true };

    const wide = analyzeHTML(low.html, { ...opts, viewport: { min: 120, max: 900, height: HEIGHT } });
    assert.equal(wide.status, 'violated');
    const errorEnd = Math.max(...wide.diagnostics.filter((d) => d.severity === 'error').map((d) => Number(d.range!.max)));
    assert.equal(errorEnd, min, 'veriss の違反の上端 = layout.ts の最小幅');

    const safe = analyzeHTML(low.html, { ...opts, viewport: { min, max: 900, height: HEIGHT } });
    assert.equal(safe.status, 'verified', JSON.stringify(safe.diagnostics));
    const proof = await verifyLayoutReport(safe);
    assert.equal(proof.status, 'verified');
  });
}

test('veriss: px の固有幅を明示しないと Unknown (veriss の既知の制限。Issue に報告)', () => {
  const shell = new Shell([createTodoApp(), createSettingsApp()], measure, memoryStorage());
  const low = lower(shell.viewOf('todo').view, measure);
  const r = analyzeHTML(low.html, { name: 'todo', css: low.css, boxes: low.boxes, viewport: { min: 300, max: 900, height: HEIGHT } });
  assert.equal(r.status, 'unknown');
  assert.ok(r.diagnostics.some((d) => d.code === 'U_LAYOUT_FLEX'));
});
