// 既存アプリ (GTK3 の AT-SPI ツリー) の写像。fixture は実際の GTK3 アプリ (legacy/gtk_app.py) から取ったもの
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';
import { mapAtspi } from '../src/legacy/atspi.ts';
import type { AtspiNode } from '../src/legacy/atspi.ts';
import { SemanticTree } from '../src/tree.ts';

const dump = JSON.parse(fs.readFileSync(path.join(import.meta.dirname, 'fixtures', 'atspi-gtk.json'), 'utf8')) as AtspiNode;

test('GTK の AT-SPI ツリーを ARIA の意味ツリーに写す (入れ物は平らにする)', () => {
  const m = mapAtspi(dump);
  const t = new SemanticTree();
  t.apply({ t: 'tree-update', app: 'legacy', root: m.root, focus: null, nodes: m.nodes });
  const flat = t.walk().map((n) => [n.role, n.label, n.checked ?? null, n.actions?.join('/') ?? '']);
  assert.deepEqual(flat, [
    ['x-window', 'Legacy Todo', null, ''],
    ['heading', 'やること (GTK)', null, ''],
    ['checkbox', '牛乳を買う', false, 'click/focus'],
    ['checkbox', '請求書を送る', true, 'click/focus'],
    ['textbox', '新しいタスク', null, 'focus/setValue/replaceSelectedText'],
    ['button', '追加', null, 'click/focus'],
    ['paragraph', '通知', null, ''],
    ['switch', '通知', false, 'click/focus'],
    ['paragraph', '変更なし', null, ''],
  ]);
  assert.deepEqual(m.unknownRoles, {});
  assert.equal(m.paths[t.walk()[2].id], '0.0.1.0', '操作のための AT-SPI の道を持つ');
});

test('写せない role は数えて、group / paragraph に落とす', () => {
  const odd: AtspiNode = { path: '', role: 'application', name: 'x', states: [], actions: [], text: null, editable: false, children: [
    { path: '0', role: 'frame', name: 'W', states: ['showing', 'sensitive'], actions: [], text: null, editable: false, children: [
      { path: '0.0', role: 'calendar', name: '予定', states: ['showing', 'sensitive'], actions: [], text: null, editable: false, children: [] },
      { path: '0.1', role: 'terminal', name: '', states: ['showing', 'sensitive'], actions: [], text: null, editable: false, children: [
        { path: '0.1.0', role: 'ruler', name: '目盛り', states: ['showing', 'sensitive'], actions: [], text: null, editable: false, children: [] },
      ] },
    ] },
  ] };
  const m = mapAtspi(odd);
  assert.deepEqual(m.unknownRoles, { calendar: 1, terminal: 1, ruler: 1 });
  assert.deepEqual(m.nodes.map((n) => n.role).sort(), ['paragraph', 'paragraph', 'x-window']);
});

test('表は列見出しの数ごとにセルを行にまとめ、スライダーは値と increment / decrement を持つ', () => {
  const st = ['showing', 'sensitive'];
  const leafNode = (path: string, role: string, name: string, extra: Partial<AtspiNode> = {}): AtspiNode => ({ path, role, name, states: st, actions: [], text: null, editable: false, children: [], ...extra });
  const app: AtspiNode = { ...leafNode('', 'application', 'x'), children: [
    { ...leafNode('0', 'frame', 'W'), children: [
      { ...leafNode('0.0', 'tree table', ''), children: [
        leafNode('0.0.0', 'table column header', '名前'), leafNode('0.0.1', 'table column header', '値'),
        leafNode('0.0.2', 'table cell', 'a'), leafNode('0.0.3', 'table cell', '1'), leafNode('0.0.4', 'table cell', 'b'), leafNode('0.0.5', 'table cell', '2'),
      ] },
      leafNode('0.1', 'slider', '音量', { value: { current: 30, min: 0, max: 100, step: 1 } }),
      leafNode('0.2', 'progress bar', '進捗', { value: { current: 0.25, min: 0, max: 1, step: 0 } }),
    ] },
  ] };
  const m = mapAtspi(app);
  const t = new SemanticTree();
  t.apply({ t: 'tree-update', app: 'x', root: m.root, focus: null, nodes: m.nodes });
  const table = t.walk().find((n) => n.role === 'table')!;
  const rows = t.children(table).map((r) => t.children(r).map((c) => c.label));
  assert.deepEqual(rows, [['名前', '値'], ['a', '1'], ['b', '2']]);
  const slider = t.walk().find((n) => n.role === 'slider')!;
  assert.deepEqual([slider.valueNow, slider.valueMin, slider.valueMax, slider.actions], [30, 0, 100, ['increment', 'decrement', 'focus']]);
  assert.equal(t.walk().find((n) => n.role === 'progressbar')!.value, '25%');
});

test('子を持つ部品 (コンボボックスの中の入力欄) も、既定の表示で子の操作を落とさない', async () => {
  const { expandTree } = await import('../src/expand.ts');
  const { defaultTemplate } = await import('../src/catalog.ts');
  const st = ['showing', 'sensitive'];
  const app: AtspiNode = { path: '', role: 'application', name: 'x', states: st, actions: [], text: null, editable: false, children: [
    { path: '0', role: 'frame', name: 'W', states: st, actions: [], text: null, editable: false, children: [
      { path: '0.0', role: 'combo box', name: '色', states: st, actions: ['press'], text: null, editable: false, children: [
        { path: '0.0.0', role: 'text', name: 'comboboxentry', states: st, actions: ['activate'], text: '赤', editable: true, children: [] },
      ] },
    ] },
  ] };
  const m = mapAtspi(app);
  const t = new SemanticTree();
  t.apply({ t: 'tree-update', app: 'x', root: m.root, focus: null, nodes: m.nodes });
  const { issues } = expandTree(t, (node) => ({ template: defaultTemplate(node), from: 'default' }));
  assert.deepEqual(issues, []);
});
