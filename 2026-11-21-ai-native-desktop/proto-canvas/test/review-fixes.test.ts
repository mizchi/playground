// PR のレビュー (CodeRabbit) で見つかった穴の回帰テスト
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createSettingsApp } from '../src/apps/settings.ts';
import { createTodoApp } from '../src/apps/todo.ts';
import { TemplateCache, memoryStorage } from '../src/cache.ts';
import type { Storage } from '../src/cache.ts';
import { defaultTemplate, validateTemplate } from '../src/catalog.ts';
import { collapseSpace, expandTree } from '../src/expand.ts';
import { lower } from '../src/lower.ts';
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

test('文字列の空白は HTML と同じ規則で潰す (計測・描画・lowering で同じ文字列)', () => {
  assert.equal(collapseSpace('  A  \t B\n'), 'A B');
  assert.equal(collapseSpace('全角\u3000空白\u00a0NBSP'), '全角\u3000空白\u00a0NBSP');
  // 端の NBSP と U+3000 も残す (CSS は落とさない)
  assert.equal(collapseSpace('\u3000A\u00a0'), '\u3000A\u00a0');
  assert.equal(collapseSpace(' \u00a0 A \u3000 '), '\u00a0 A \u3000');
  const tree = new SemanticTree();
  tree.apply({ t: 'tree-update', app: 'a', root: 'r', focus: null, nodes: [
    { id: 'r', role: 'group', children: ['h', 'b', 'i'] },
    { id: 'h', role: 'heading', label: 'A  B' },
    { id: 'b', role: 'button', label: ' 押す   ボタン ', actions: ['click'] },
    { id: 'i', role: 'textbox', label: 'x', value: 'a  b', actions: ['setValue'] },
  ] });
  const { view } = expandTree(tree, (n) => ({ template: defaultTemplate(n), from: 'default' }));
  const leaves: string[] = [];
  const walk = (v: typeof view) => {
    if (v.kind === 'text') leaves.push(v.str);
    else if (v.kind === 'widget') leaves.push(`${v.node.label}|${v.node.value ?? ''}`);
    else v.children.forEach(walk);
  };
  walk(view);
  assert.deepEqual(leaves, ['A B', '押す ボタン|', 'x|a  b'], '入力欄の value は潰さない');
  // lowering の HTML と文字の契約も同じ文字列
  const low = lower(view, measure);
  assert.ok(low.html.includes('>A B<') && low.html.includes('>押す ボタン<'));
});

test('人のフォーカスは shell が先に動かす: アプリの返事 (Worker など) より前に打った文字も、その入力欄に届く', () => {
  // 返事を後回しにするアプリ (Worker で動くコンポーネントの代わり)
  const got: string[] = [];
  let emit: (u: import('../src/protocol.ts').TreeUpdate) => void = () => {};
  let focus: string | null = null;
  const pending: (() => void)[] = [];
  const publish = (root?: string) =>
    emit({ t: 'tree-update', app: 'slow', root, focus, nodes: [
      { id: 'root', role: 'x-window', label: 'Slow', children: ['a', 'b'] },
      { id: 'a', role: 'textbox', label: 'A', value: '', actions: ['focus', 'replaceSelectedText'] },
      { id: 'b', role: 'textbox', label: 'B', value: '', actions: ['focus', 'replaceSelectedText'] },
    ] });
  const slow = {
    id: 'slow',
    start(e: typeof emit) {
      emit = e;
      publish('root');
    },
    onAction(req: import('../src/protocol.ts').ActionRequest) {
      got.push(`${req.action}:${req.target}:${req.data ?? ''}`);
      pending.push(() => {
        if (req.action === 'focus') focus = req.target;
        publish();
      });
    },
  };
  const shell = new Shell([slow], measure, memoryStorage());
  shell.frame(800, 600);
  shell.activate('slow', 'b');
  assert.equal(shell.tree('slow').focus, 'b', '返事の前にフォーカスが動く');
  shell.input({ t: 'ime-commit', text: 'あ' });
  assert.deepEqual(got, ['focus:b:', 'replaceSelectedText:b:あ']);
  // アプリの返事が来ても同じ (アプリが別の場所へ回せば、そちらが勝つ)
  for (const f of pending.splice(0)) f();
  assert.equal(shell.tree('slow').focus, 'b');
});

test('保存領域の写しは、どんなキーもただの文字列として扱う (__proto__ や constructor)', async () => {
  const { memoryKeyValue, ServiceHost } = await import('../src/platform/services.ts');
  const kv = memoryKeyValue();
  kv.set('app.x.__proto__', 'p');
  kv.set('app.x.constructor', 'c');
  kv.set('app.y.other', 'o');
  const host = new ServiceHost(kv);
  assert.deepEqual(new Map(host.storageEntries('x')), new Map([['__proto__', 'p'], ['constructor', 'c']]));
});
