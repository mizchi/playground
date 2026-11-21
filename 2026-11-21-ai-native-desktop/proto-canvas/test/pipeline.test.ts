import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createSettingsApp } from '../src/apps/settings.ts';
import { createTodoApp } from '../src/apps/todo.ts';
import { memoryStorage } from '../src/cache.ts';
import { validateTemplate } from '../src/catalog.ts';
import type { ViewTemplate } from '../src/catalog.ts';
import { Shell } from '../src/shell.ts';

const measure = (s: string, size: number) => [...s].length * size * 0.6;
const make = () => {
  const shell = new Shell([createTodoApp(), createSettingsApp()], measure, memoryStorage());
  shell.frame(1000, 600);
  return shell;
};
const sigOf = (shell: Shell, app: string, node: string) => shell.signatures(app).find((s) => s.node === node)!.sig;
const node = (shell: Shell, app: string, id: string) => shell.tree(app).get(id)!;

/** タスク一覧をカードにする template (LLM が出す想定のもの) */
const card: ViewTemplate = {
  type: 'box',
  axis: 'vertical',
  gap: 2,
  padding: 10,
  background: 'bg',
  border: 'border',
  radius: 8,
  children: [
    { type: 'text', text: { bind: 'count', format: '{} 件のタスク' }, size: 12, weight: 700, color: 'muted' },
    { type: 'each', item: { type: 'box', axis: 'horizontal', padding: 4, children: [{ type: 'self' }] } },
  ],
};

test('構造シグネチャは値と件数に依存しない', () => {
  const shell = make();
  const before = sigOf(shell, 'todo', 'list');
  assert.equal(before, 'list[list-of(checkbox|click|focus)]');
  shell.dispatch({ app: 'todo', target: 'input', action: 'setValue', data: '卵', by: 'human' });
  shell.dispatch({ app: 'todo', target: 'add', action: 'click', by: 'human' });
  assert.equal(node(shell, 'todo', 'list').children!.length, 4);
  assert.equal(sigOf(shell, 'todo', 'list'), before);
});

test('人間のクリックは hit test で意味的な action になる', () => {
  const shell = make();
  const { windows } = shell.frame(1000, 600);
  assert.equal(windows.length, 2);
  // i1 のチェックボックスを探してクリック
  const ops = shell.frame(1000, 600).ops;
  const label = ops.find((o) => o[0] === 'text' && o[1].str === '牛乳を買う');
  assert.ok(label && label[0] === 'text');
  shell.pointerDown(label[1].x - 12, label[1].y);
  assert.equal(node(shell, 'todo', 'i1').checked, true);
  assert.deepEqual(
    shell.audit.map((a) => [a.by, a.target, a.action, a.result]),
    [
      ['human', 'i1', 'focus', 'ok'],
      ['human', 'i1', 'click', 'ok'],
    ],
  );
});

test('IME: preedit は木に入れず、確定文字列だけを replaceSelectedText にする', () => {
  const shell = make();
  shell.dispatch({ app: 'todo', target: 'input', action: 'focus', by: 'human' });
  assert.equal(shell.frame(1000, 600).ime.t, 'ime-enable');
  shell.input({ t: 'ime-preedit', text: 'たまご' });
  assert.equal(node(shell, 'todo', 'input').value, '');
  const preedit = shell.frame(1000, 600).ops.find((o) => o[0] === 'text' && o[1].underline);
  assert.ok(preedit && preedit[0] === 'text' && preedit[1].str === 'たまご');
  shell.input({ t: 'ime-commit', text: '卵' });
  assert.equal(node(shell, 'todo', 'input').value, '卵');
  shell.input({ t: 'key', key: 'Backspace' });
  assert.equal(node(shell, 'todo', 'input').value, '');
  assert.ok(shell.getTree('todo')[0].nodes.every((n) => !String(n.value ?? '').includes('たまご')));
});

test('agent は承認された提案の範囲でしか操作できない', () => {
  const shell = make();
  const direct = shell.dispatch({ app: 'todo', target: 'i1', action: 'click', by: 'agent' });
  assert.equal(direct.result, 'denied');
  assert.equal(node(shell, 'todo', 'i1').checked, false);

  const r = shell.propose('牛乳を完了にする', [{ app: 'todo', target: 'i1', action: 'click' }]);
  assert.ok(r.ok);
  assert.equal(r.proposal.status, 'pending');
  assert.equal(node(shell, 'todo', 'i1').checked, false, 'プレビュー中は副作用なし');
  const ghost = shell.frame(1000, 600).ops.some((o) => o[0] === 'rect' && o[1].dash);
  assert.ok(ghost, '提案はゴーストで表示される');
  shell.approve(r.proposal.id);
  assert.equal(node(shell, 'todo', 'i1').checked, true);
  assert.equal(r.proposal.status, 'done');
});

test('agent は公開されていない action や focus を提案できない', () => {
  const shell = make();
  const bad = shell.propose('x', [
    { app: 'todo', target: 'status', action: 'click' },
    { app: 'todo', target: 'input', action: 'focus' },
  ]);
  assert.equal(bad.ok, false);
  if (!bad.ok) assert.equal(bad.errors.length, 2);
});

test('自動承認は sensitive を含まない提案だけ', () => {
  const shell = make();
  shell.policy.autoApproveSafe = true;
  const safe = shell.propose('コンパクト表示', [{ app: 'settings', target: 'compact', action: 'click' }]);
  assert.ok(safe.ok && safe.proposal.status === 'done');
  const risky = shell.propose('全削除', [{ app: 'settings', target: 'reset', action: 'click' }]);
  assert.ok(risky.ok && risky.proposal.status === 'pending' && risky.proposal.sensitive);
  shell.approve(risky.proposal.id);
  // アプリ自身も actor を見て拒否できる
  assert.equal(node(shell, 'settings', 'status').label, 'エージェントからの全削除は拒否しました');
});

test('template: 静的検査で語彙外・範囲外・自己再帰を拒否する', () => {
  assert.deepEqual(validateTemplate(card), []);
  const errs = validateTemplate({ type: 'box', gap: 999, background: '#f00', onClick: 'x', children: [{ type: 'self' }, { type: 'widget', kind: 'constructor' }] });
  assert.ok(errs.some((e) => e.includes('gap')));
  assert.ok(errs.some((e) => e.includes('色はトークン')));
  assert.ok(errs.some((e) => e.includes('onClick')));
  assert.ok(errs.some((e) => e.includes('self は each の中')));
  assert.ok(errs.some((e) => e.includes('kind')));
});

test('template: キャッシュに入れると同じ構造に再利用され、件数が変わってもヒットする', () => {
  const shell = make();
  const sig = sigOf(shell, 'todo', 'list');
  assert.ok(shell.cache.propose(sig, card, 'test').ok);
  const has = (s: string) => shell.frame(1000, 600).ops.some((o) => o[0] === 'text' && o[1].str === s);
  assert.ok(has('3 件のタスク'));
  shell.dispatch({ app: 'todo', target: 'input', action: 'setValue', data: '卵', by: 'human' });
  shell.dispatch({ app: 'todo', target: 'add', action: 'click', by: 'human' });
  assert.ok(has('4 件のタスク'));
  assert.deepEqual(shell.frame(1000, 600).windows[0].issues, []);
});

test('template: 操作できるノードを落とす・重複させる・role を偽る template は既定に戻る', () => {
  const shell = make();
  const sig = sigOf(shell, 'todo', 'list');
  const cases: [ViewTemplate, RegExp][] = [
    [{ type: 'text', text: { bind: 'label' } }, /view にない/],
    [{ type: 'box', children: [{ type: 'children' }, { type: 'children' }] }, /2 回描かれている/],
    [{ type: 'widget', kind: 'button' }, /role list に結びつけられない/],
  ];
  for (const [t, re] of cases) {
    assert.ok(shell.cache.propose(sig, t, 'test').ok);
    const { windows, ops } = shell.frame(1000, 600);
    assert.match(windows[0].issues.map((i) => i.reason).join('\n'), re);
    assert.ok(ops.some((o) => o[0] === 'text' && o[1].str === '牛乳を買う'), '既定レンダラで描かれている');
  }
});

test('タイルが最小幅を下回るとはみ出しを報告する', () => {
  const shell = make();
  shell.split = 0.15;
  const { windows } = shell.frame(800, 600);
  assert.ok(windows[0].overflow.length > 0);
  assert.ok(windows[0].minWidth > windows[0].tile.w);
  shell.split = 0.5;
  assert.equal(shell.frame(800, 600).windows[0].overflow.length, 0);
});
