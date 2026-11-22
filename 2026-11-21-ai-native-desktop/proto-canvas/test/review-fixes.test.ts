// PR のレビュー (CodeRabbit) で見つかった穴の回帰テスト
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createSettingsApp } from '../src/apps/settings.ts';
import { createTodoApp } from '../src/apps/todo.ts';
import { TemplateCache, memoryStorage } from '../src/cache.ts';
import type { Storage } from '../src/cache.ts';
import { validateTemplate } from '../src/catalog.ts';
import { expandTree } from '../src/expand.ts';
import { layout } from '../src/layout.ts';
import { Shell } from '../src/shell.ts';
import type { Step } from '../src/shell.ts';
import { SemanticTree } from '../src/tree.ts';

const measure = (s: string, size: number) => [...s].length * size * 0.6;
const make = () => {
  const shell = new Shell([createTodoApp(), createSettingsApp()], measure, memoryStorage());
  shell.frame(1000, 600);
  return shell;
};

test('保存先の中身が壊れていても、正しい entry だけを読んで起動する', () => {
  const good = { sig: 's', template: { type: 'children' }, state: 'pinned', origin: 'human', verification: 'unchecked' };
  for (const bad of [null, [null], [{ sig: 1 }], [{ ...good, state: 'x' }], 'text']) {
    const st: Storage = { load: () => bad as never, save: () => {} };
    assert.equal(new TemplateCache(st).list().length, 0, JSON.stringify(bad));
  }
  const mixed: Storage = { load: () => [null, good, { ...good, template: { type: 'nope' } }] as never, save: () => {} };
  assert.deepEqual(new TemplateCache(mixed).list().map((e) => e.sig), ['s']);
  const throwing: Storage = { load: () => { throw new Error('x'); }, save: () => {} };
  assert.equal(new TemplateCache(throwing).list().length, 0);
});

test('提案の手順の型が違えば、理由付きで断る (data が数など)', () => {
  const shell = make();
  const bad = (steps: unknown) => shell.propose('x', steps as Step[]);
  const r1 = bad([{ app: 'todo', target: 'input', action: 'setValue', data: 42 }]);
  assert.ok(!r1.ok && r1.errors[0].includes('data'));
  const r2 = bad([null]);
  assert.ok(!r2.ok);
  const r3 = bad([{ kind: 'layout', op: { type: 'explode' } }]);
  assert.ok(!r3.ok && r3.errors[0].includes('op.type'));
  const r4 = bad('steps');
  assert.ok(!r4.ok);
  assert.equal(shell.proposals.length, 0, '断った提案は残さない');
});

test('フォーカスが無いとき、Shift+Tab は最後の部品へ行く', () => {
  const shell = new Shell([createSettingsApp()], measure, memoryStorage());
  shell.frame(1000, 600);
  shell.input({ t: 'key', key: 'ShiftTab' });
  assert.equal(shell.tree('settings').focus, 'reset');
});

test('format の {} には値をそのまま入れる ($& などを解釈しない)', () => {
  const tree = new SemanticTree();
  tree.apply({ t: 'tree-update', app: 'a', root: 'r', focus: null, nodes: [{ id: 'r', role: 'heading', label: '$& and $1' }] });
  const { view } = expandTree(tree, () => ({ template: { type: 'text', text: { bind: 'label', format: '名前: {}' } }, from: 'cache' }));
  assert.equal(view.kind === 'text' && view.str, '名前: $& and $1');
});

test('根が部品 1 つでも、幅が足りなければはみ出しを報告する', () => {
  const view = { kind: 'widget' as const, widget: 'button' as const, node: { id: 'b', role: 'button' as const, label: 'とても長いボタンの名前', actions: ['click' as const] } };
  assert.equal(layout(view, 60, measure).overflow.length, 1);
  assert.equal(layout(view, 600, measure).overflow.length, 0);
});

test('grow は整数だけ (小数は CSS と意味が変わる)', () => {
  assert.ok(validateTemplate({ type: 'box', grow: 0.5, children: [] }).some((e) => e.includes('grow')));
  assert.deepEqual(validateTemplate({ type: 'box', grow: 2, children: [] }), []);
});

test('隣の窓にはみ出した部品は、はみ出した所では押せない (当たり判定を窓の中身で切る)', () => {
  const shell = make();
  shell.frame(1000, 600);
  const rects = [...shell.accessibility().rects.values()];
  const wins = shell.frame(1000, 600).windows.map((w) => w.tile);
  for (const r of rects) assert.ok(wins.some((t) => r.x >= t.x - 0.01 && r.x + r.w <= t.x + t.w + 0.01), `窓の外の当たり判定 ${JSON.stringify(r)}`);
});
