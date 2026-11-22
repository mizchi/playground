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
      { path: '0.1', role: 'tree table', name: '', states: ['showing', 'sensitive'], actions: [], text: null, editable: false, children: [
        { path: '0.1.0', role: 'table cell', name: 'セル', states: ['showing', 'sensitive'], actions: [], text: null, editable: false, children: [] },
      ] },
    ] },
  ] };
  const m = mapAtspi(odd);
  assert.deepEqual(m.unknownRoles, { calendar: 1, 'tree table': 1, 'table cell': 1 });
  assert.deepEqual(m.nodes.map((n) => n.role).sort(), ['paragraph', 'paragraph', 'x-window']);
});
