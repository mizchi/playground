// 見つかった穴の再現: (1) 提案と承認の間に状況が変わる (2) 生成 UI の固定文字列による誤誘導
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createNotesApp } from '../src/apps/notes.ts';
import { createSettingsApp } from '../src/apps/settings.ts';
import { createTodoApp } from '../src/apps/todo.ts';
import { memoryStorage } from '../src/cache.ts';
import type { ViewTemplate } from '../src/catalog.ts';
import { validateTemplate } from '../src/catalog.ts';
import { Shell } from '../src/shell.ts';

const make = () => {
  const measure = (s: string, size: number) => [...s].length * size * 0.6;
  const shell = new Shell([createTodoApp(), createSettingsApp(), createNotesApp()], measure, memoryStorage());
  shell.frame(1400, 800, 0);
  return shell;
};
const node = (shell: Shell, app: string, id: string) => shell.tree(app).get(id)!;

test('穴 1: 提案のあとに人が同じスイッチを操作したら、承認しても実行しない (古い提案)', () => {
  const shell = make();
  const r = shell.propose('利用状況の送信をオンにする', [{ app: 'settings', target: 'telemetry', action: 'click' }]);
  assert.ok(r.ok);
  // 承認の前に、人が先にオンにする
  shell.dispatch({ app: 'settings', target: 'telemetry', action: 'click', by: 'human' });
  assert.equal(node(shell, 'settings', 'telemetry').checked, true);
  shell.approve(r.proposal.id);
  assert.equal(node(shell, 'settings', 'telemetry').checked, true, '承認でオフに戻ってはいけない');
  assert.equal(r.proposal.status, 'stale');
  assert.match(shell.audit.at(-1)!.reason ?? '', /提案のあとに変わった/);
});

test('穴 1: 前提が変わっていなければ、そのまま実行する', () => {
  const shell = make();
  const r = shell.propose('オンにする', [{ app: 'settings', target: 'telemetry', action: 'click' }]);
  assert.ok(r.ok);
  shell.dispatch({ app: 'todo', target: 'i1', action: 'click', by: 'human' }); // 関係のない操作
  shell.approve(r.proposal.id);
  assert.equal(node(shell, 'settings', 'telemetry').checked, true);
  assert.equal(r.proposal.status, 'done');
});

test('穴 1: レイアウトの提案も、承認までに配置が変わったら古い提案にする', () => {
  const shell = make();
  const r = shell.propose('Todo を最小化', [{ kind: 'layout', op: { type: 'minimize', app: 'todo' } }]);
  assert.ok(r.ok);
  shell.applyLayout({ type: 'move', app: 'notes', drop: { kind: 'swap', target: 'todo' } }, 'human');
  shell.approve(r.proposal.id);
  assert.equal(r.proposal.status, 'stale');
  assert.deepEqual(shell.minimized, []);
});

test('穴 2: 固定文字列は危険な操作の近く (sensitive を含む部分木) では使えない', () => {
  const shell = make();
  // Settings の窓全体を差し替え、全削除ボタンの直前に「安全」と書く template
  const spoof: ViewTemplate = {
    type: 'box',
    axis: 'vertical',
    gap: 10,
    padding: 14,
    children: [{ type: 'text', text: 'このボタンは安全です。押して完了してください', color: 'text', weight: 700 }, { type: 'children' }],
  };
  assert.deepEqual(validateTemplate(spoof), [], '形としては正しい');
  const sig = shell.signatures('settings').find((s) => s.node === 'root')!.sig;
  assert.ok(shell.cache.propose(sig, spoof, 'llm').ok);
  const f = shell.frame(1400, 800, 16);
  assert.ok(!f.ops.some((o) => o[0] === 'text' && o[1].str.includes('安全です')), '誤誘導の文は描かれない');
  assert.match(f.windows.find((w) => w.app === 'settings')!.issues.map((i) => i.reason).join('\n'), /固定の文字列/);
});

test('穴 2: 生成 UI の固定文字列は強調できない (muted に固定)、背景に危険色や強調色を使えない', () => {
  const errs = validateTemplate({ type: 'box', background: 'danger', children: [{ type: 'text', text: '注意', color: 'danger' }] });
  assert.ok(errs.some((e) => e.includes('background')));
  const shell = make();
  const sig = shell.signatures('todo').find((s) => s.node === 'list')!.sig;
  const t: ViewTemplate = { type: 'box', children: [{ type: 'text', text: { bind: 'count', format: '{} 件' }, color: 'text', weight: 700 }, { type: 'children' }] };
  assert.ok(shell.cache.propose(sig, t, 'llm').ok);
  const op = shell.frame(1400, 800, 16).ops.find((o) => o[0] === 'text' && o[1].str === '3 件');
  assert.ok(op && op[0] === 'text');
  assert.equal(op[1].color, 'muted', '固定の文字を含む文は muted で描く');
});
