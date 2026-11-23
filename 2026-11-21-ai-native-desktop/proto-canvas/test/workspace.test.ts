// 人と agent のワークスペース: agent は自分のワークスペースで直接動かせる。保存領域への書き込みは重ねた層に溜まり、
// 人が持ち込みを承認したときだけ本物に書く。
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { builtinManifests } from '../src/apps/manifests.ts';
import { memoryStorage } from '../src/cache.ts';
import { createPlatform } from '../src/platform/platform.ts';
import { memoryKeyValue } from '../src/platform/services.ts';
import type { KeyValue } from '../src/platform/services.ts';

const measure = (s: string, size: number) => [...s].length * size * 0.6;
const make = (kv: KeyValue = memoryKeyValue()) => {
  const p = createPlatform({ apps: builtinManifests, initial: ['todo', 'settings'], measure, templates: memoryStorage(), kv });
  for (const sh of [p.shell, p.agentShell]) {
    sh.animations = false;
    sh.frame(1400, 800);
  }
  return p;
};
type P = ReturnType<typeof make>;
const labels = (p: P, shell: 'human' | 'agent', id: string) => {
  const t = (shell === 'human' ? p.shell : p.agentShell).tree(id);
  return t.get('list')!.children!.map((c) => t.get(c)!.label);
};
const addTodo = (p: P, id: string, text: string) => {
  assert.equal(p.agent.act(id, 'input', 'setValue', text), '');
  assert.equal(p.agent.act(id, 'add', 'click'), '');
};

test('agent のワークスペースには作業場があり、agent は提案なしで窓を開ける (窓の id は人の側と重ならない)', () => {
  const p = make();
  assert.deepEqual(p.agentShell.windows().map((w) => w.id), ['agent-desk']);
  assert.equal(p.agent.launch('todo'), '');
  assert.deepEqual(p.agentShell.windows().map((w) => w.id), ['agent-desk', 'todo-2']);
  assert.ok(!p.shell.windows().some((w) => w.id === 'todo-2'), '人のワークスペースには出ない');
  assert.match(p.agent.launch('settings'), /開けない/, '1 つだけのアプリは、人の側で開いていれば開けない');
});

test('agent は自分のワークスペースでは直接動かせる。人のワークスペースでは今まで通り提案が要る', () => {
  const p = make();
  p.agent.launch('todo');
  assert.equal(p.agent.act('todo-2', 'i1', 'click'), '');
  assert.equal(p.agentShell.tree('todo-2').get('i1')!.checked, true);
  assert.equal(p.shell.dispatch({ app: 'todo', target: 'i1', action: 'click', by: 'agent' }).result, 'denied');
  assert.match(p.agent.act('todo-2', 'input', 'focus'), /focus/);
});

test('agent のワークスペースでも、sensitive なノードは承認が要る', () => {
  const p = make();
  // 作業場の「変更を捨てる」は sensitive
  p.agent.launch('todo');
  addTodo(p, 'todo-2', '卵');
  assert.match(p.agent.act('agent-desk', 'discard', 'click'), /承認された提案がない/);
});

test('保存領域への書き込みは重ねた層に溜まり、本物と人の窓は変わらない。作業場に差分が出る', () => {
  const kv = memoryKeyValue();
  const p = make(kv);
  p.agent.launch('todo');
  addTodo(p, 'todo-2', '卵を買う');
  assert.ok(labels(p, 'agent', 'todo-2').includes('卵を買う'));
  assert.ok(!labels(p, 'human', 'todo').includes('卵を買う'), '人の窓は変わらない');
  assert.equal(kv.get('app.todo.items'), undefined, '本物には書かない');
  const changes = p.services.pendingChanges();
  assert.deepEqual(changes.map((c) => [c.app, c.key]), [['todo', 'items']]);
  const desk = p.agentShell.tree('agent-desk');
  assert.match(desk.get('c0')!.label!, /^todo \/ items: /);
  assert.ok(!desk.get('merge')!.disabled, '持ち込みを提案できる');
});

test('agent のワークスペースでは、外へ出る能力 (通知・クリップボード) は使えない', () => {
  const p = make();
  p.agent.launch('notes');
  const id = p.agentShell.windows().at(-1)!.id;
  p.agent.act(id, 'copy', 'click');
  assert.match(p.agentShell.tree(id).get('status')!.label!, /コピーできない \(denied\)/);
  assert.ok(p.services.audit.some((a) => a.app === 'notes' && a.op === 'write (agent のワークスペース)' && a.result === 'denied'));
});

test('持ち込み: agent が提案し、人が承認すると本物に書き、人の窓が読み直す。agent は答えられない', () => {
  const kv = memoryKeyValue();
  const p = make(kv);
  p.agent.launch('todo');
  addTodo(p, 'todo-2', '牛乳を 2 本');
  assert.equal(p.agent.act('agent-desk', 'merge', 'click'), '');
  const m = p.services.merges[0];
  assert.equal(m.status, 'pending');
  assert.equal(m.by, 'agent');
  assert.equal(p.agentShell.tree('agent-desk').get('merge')!.disabled, true, '答えを待つ間は二重に提案しない');
  assert.equal(p.services.answerMerge(m.id, true, 'agent'), '持ち込みに答えられるのは人だけ');
  assert.equal(p.services.answerMerge(m.id, true, 'human'), '');
  assert.equal(m.status, 'applied');
  assert.match(kv.get('app.todo.items')!, /牛乳を 2 本/);
  assert.ok(labels(p, 'human', 'todo').includes('牛乳を 2 本'), '人の窓が読み直した');
  assert.equal(p.services.pendingChanges().length, 0, '重ねた層は空になる');
  assert.ok(p.services.audit.some((a) => a.op === `merge items (${m.id})`));
});

test('持ち込みの提案のあとに人が同じ値を変えたら、承認しても書かない (stale)', () => {
  const kv = memoryKeyValue();
  const p = make(kv);
  p.agent.launch('todo');
  addTodo(p, 'todo-2', 'agent の項目');
  const r = p.services.proposeMerge('持ち込む', 'agent');
  assert.ok(r.ok);
  // 人が自分のワークスペースで Todo を変える
  p.shell.dispatch({ app: 'todo', target: 'i1', action: 'click', by: 'human' });
  assert.match(p.services.answerMerge(r.merge.id, true, 'human'), /書かなかった/);
  assert.equal(r.merge.status, 'stale');
  assert.doesNotMatch(kv.get('app.todo.items')!, /agent の項目/);
});

test('人は変更を捨てられる。agent は捨てられない', () => {
  const p = make();
  p.agent.launch('todo');
  addTodo(p, 'todo-2', '捨てる');
  assert.equal(p.services.discardOverlay('agent'), '変更を捨てられるのは人だけ');
  assert.equal(p.services.discardOverlay('human'), '');
  assert.equal(p.services.pendingChanges().length, 0);
  // agent の窓も読み直す (捨てた項目を手元に残したまま、次の保存で層に戻さない)
  assert.ok(!labels(p, 'agent', 'todo-2').includes('捨てる'));
  p.agent.act('todo-2', 'i1', 'click');
  assert.doesNotMatch(p.services.pendingChanges()[0].after, /捨てる/);
});

test('agent が書き始めたあと、提案の前に人が同じ値を変えていても stale になる (人の変更を上書きしない)', () => {
  const kv = memoryKeyValue();
  const p = make(kv);
  p.agent.launch('todo');
  addTodo(p, 'todo-2', 'agent の項目');
  p.shell.dispatch({ app: 'todo', target: 'i1', action: 'click', by: 'human' });
  const human = kv.get('app.todo.items');
  const r = p.services.proposeMerge('持ち込む', 'agent');
  assert.ok(r.ok);
  assert.equal(r.merge.changes[0].before, undefined, 'before は agent が書き始めた時点の値');
  assert.match(p.services.answerMerge(r.merge.id, true, 'human'), /書かなかった/);
  assert.equal(kv.get('app.todo.items'), human);
});

test('提案のあとに agent がさらに書いた分は、承認しても消えずに次の持ち込みに残る', () => {
  const kv = memoryKeyValue();
  const p = make(kv);
  p.agent.launch('todo');
  addTodo(p, 'todo-2', '一つ目');
  const r = p.services.proposeMerge('一つ目', 'agent');
  assert.ok(r.ok);
  addTodo(p, 'todo-2', '二つ目');
  assert.equal(p.services.answerMerge(r.merge.id, true, 'human'), '');
  assert.match(kv.get('app.todo.items')!, /一つ目/);
  assert.doesNotMatch(kv.get('app.todo.items')!, /二つ目/);
  const rest = p.services.pendingChanges();
  assert.equal(rest.length, 1);
  assert.match(rest[0].after, /二つ目/);
  assert.equal(rest[0].before, kv.get('app.todo.items'), '基準は持ち込んだ値');
  const r2 = p.services.proposeMerge('二つ目', 'agent');
  assert.ok(r2.ok);
  assert.equal(p.services.answerMerge(r2.merge.id, true, 'human'), '');
  assert.match(kv.get('app.todo.items')!, /二つ目/);
});

test('作業場のボタンへのフォーカスは、木を出し直しても保たれる (Enter で押せる)', () => {
  const p = make();
  p.agent.launch('todo');
  addTodo(p, 'todo-2', '卵');
  p.agentShell.dispatch({ app: 'agent-desk', target: 'merge', action: 'focus', by: 'human' });
  assert.equal(p.agentShell.tree('agent-desk').focus, 'merge');
  addTodo(p, 'todo-2', 'ハム'); // 作業場の木が出し直される
  assert.equal(p.agentShell.tree('agent-desk').focus, 'merge');
});
