// 既存の Linux アプリ (GTK3) を AT-SPI 経由で試作の shell に入れ、同じ権限モデルで操作できるかを確かめる (VALIDATION.md の 5b)。
// 使い方: legacy/run-env.sh node e2e/atspi-bridge.ts   (sway / at-spi2-core / python3-pyatspi / gtk-3-examples)
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { createSettingsApp } from '../src/apps/settings.ts';
import { memoryStorage } from '../src/cache.ts';
import { mapAtspi } from '../src/legacy/atspi.ts';
import type { AtspiNode } from '../src/legacy/atspi.ts';
import { createAtspiApp } from '../src/legacy/atspi-app.ts';
import { Shell } from '../src/shell.ts';

const PY = '/usr/bin/python3.12';
const here = import.meta.dirname;
const sleep = (ms: number) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
const waitApp = (name: string) => {
  for (let i = 0; i < 50; i++) {
    try {
      return JSON.parse(execFileSync(PY, [path.join(here, '..', 'legacy', 'atspi_bridge.py'), 'dump', name], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] })) as AtspiNode;
    } catch {
      sleep(200);
    }
  }
  throw new Error(`${name} が AT-SPI に現れない`);
};

let failures = 0;
const results: Record<string, unknown> = {};
function step(name: string, fn: () => void) {
  try {
    fn();
    console.log(`ok   ${name}`);
  } catch (e) {
    failures++;
    console.log(`FAIL ${name}\n     ${String((e as Error).message).split('\n').join('\n     ')}`);
  }
}

const gtk = spawn(PY, [path.join(here, '..', 'legacy', 'gtk_app.py')], { stdio: 'ignore' });
waitApp('gtk_app.py');
const legacy = createAtspiApp('gtk', 'gtk_app.py');
const shell = new Shell([legacy, createSettingsApp()], (s, z) => [...s].length * z * 0.6, memoryStorage());
shell.frame(1400, 800, 0);
const byLabel = (label: string) => shell.tree('gtk').walk().find((n) => n.label === label)!;
const gtkState = (label: string) => {
  legacy.refresh();
  return byLabel(label);
};

step('GTK アプリのツリーが、ARIA の意味ツリーとして shell に入る', () => {
  const roles = shell.tree('gtk').walk().map((n) => `${n.role}:${n.label}`);
  assert.deepEqual(roles, ['x-window:Legacy Todo', 'heading:やること (GTK)', 'checkbox:牛乳を買う', 'checkbox:請求書を送る', 'textbox:新しいタスク', 'button:追加', 'paragraph:通知', 'switch:通知', 'paragraph:変更なし']);
});

step('試作の描画で GTK アプリを描ける (意味ツリーからの再描画)', () => {
  const f = shell.frame(1400, 800, 16);
  assert.ok(f.ops.some((o) => o[0] === 'text' && o[1].str === '牛乳を買う'));
  assert.ok(f.windows.some((w) => w.app === 'gtk' && w.title === 'Legacy Todo'));
});

step('人の操作 (click) が、GTK のチェックボックスを実際に切り替える', () => {
  shell.dispatch({ app: 'gtk', target: byLabel('牛乳を買う').id, action: 'click', by: 'human' });
  assert.equal(gtkState('牛乳を買う').checked, true);
  assert.equal(gtkState('牛乳を買う: 完了') !== undefined || byLabel('牛乳を買う: 完了') !== undefined, true, 'GTK 側の状態表示も変わる');
});

step('agent の直接操作は拒否され、GTK は変わらない', () => {
  const r = shell.dispatch({ app: 'gtk', target: byLabel('請求書を送る').id, action: 'click', by: 'agent' });
  assert.equal(r.result, 'denied');
  assert.equal(gtkState('請求書を送る').checked, true);
});

step('agent の提案 (入力して追加) は、承認すると GTK で実行される', () => {
  const r = shell.propose('卵を追加する', [
    { app: 'gtk', target: byLabel('新しいタスク').id, action: 'setValue', data: '卵を買う' },
    { app: 'gtk', target: byLabel('追加').id, action: 'click' },
  ]);
  assert.ok(r.ok, JSON.stringify(r));
  assert.equal(byLabel('卵を買う'), undefined, '承認までは何も起きない');
  shell.approve(r.proposal.id);
  assert.equal(gtkState('卵を買う')?.role, 'checkbox', 'GTK に新しいチェックボックスができた');
  assert.equal(byLabel('新しいタスク').value, '', '入力欄は空に戻る');
});

step('提案のあとに人が同じスイッチを操作したら、承認しても実行しない (stale)', () => {
  const sw = () => shell.tree('gtk').walk().find((n) => n.role === 'switch')!;
  const r = shell.propose('通知をオンにする', [{ app: 'gtk', target: sw().id, action: 'click' }]);
  assert.ok(r.ok);
  shell.dispatch({ app: 'gtk', target: sw().id, action: 'click', by: 'human' });
  legacy.refresh();
  assert.equal(sw().checked, true);
  shell.approve(r.proposal.id);
  legacy.refresh();
  assert.equal(r.proposal.status, 'stale');
  assert.equal(sw().checked, true, 'オフに戻らない');
});
gtk.kill();

// ---- 実在の GTK アプリでの写像の網羅率 (gtk3-widget-factory) ----
step('gtk3-widget-factory: 写せる role の割合', () => {
  const wf = spawn('gtk3-widget-factory', [], { stdio: 'ignore' });
  const dump = waitApp('gtk3-widget-factory');
  wf.kill();
  const count = (n: AtspiNode, acc: Record<string, number>) => {
    if (n.states.includes('showing')) acc[n.role] = (acc[n.role] ?? 0) + 1;
    n.children.forEach((c) => count(c, acc));
    return acc;
  };
  const all = count(dump, {});
  const m = mapAtspi(dump);
  const total = Object.values(all).reduce((a, b) => a + b, 0);
  const unknown = Object.values(m.unknownRoles).reduce((a, b) => a + b, 0);
  results.widgetFactory = { total, unknown, unknownRoles: m.unknownRoles, roles: all, mappedNodes: m.nodes.length, interactive: m.nodes.filter((n) => n.actions?.length).length };
  console.log(`     表示中の節点 ${total} 個、写せない role ${unknown} 個 (${((unknown / total) * 100).toFixed(1)}%)、意味ツリーの節点 ${m.nodes.length} 個 (操作できる ${m.nodes.filter((n) => n.actions?.length).length} 個)`);
  console.log(`     写せない role: ${JSON.stringify(m.unknownRoles)}`);
  assert.ok(total > 50);
});

fs.writeFileSync(path.join(here, '..', 'atspi-bridge.json'), JSON.stringify(results, null, 2));
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
