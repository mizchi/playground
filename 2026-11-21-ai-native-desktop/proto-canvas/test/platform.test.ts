import assert from 'node:assert/strict';
import { test } from 'node:test';
import { builtinManifests } from '../src/apps/manifests.ts';
import { memoryStorage } from '../src/cache.ts';
import { createPlatform } from '../src/platform/platform.ts';
import { memoryKeyValue } from '../src/platform/services.ts';
import type { KeyValue } from '../src/platform/services.ts';

const measure = (s: string, size: number) => [...s].length * size * 0.6;
const make = (kv: KeyValue = memoryKeyValue(), opts: { initial?: string[]; restore?: boolean } = {}) => {
  const p = createPlatform({ apps: builtinManifests, initial: opts.initial ?? ['todo', 'notes'], measure, templates: memoryStorage(), kv, restoreSession: opts.restore });
  p.shell.animations = false;
  p.shell.frame(1200, 700);
  return p;
};
const ids = (p: ReturnType<typeof make>) => p.shell.windows().map((w) => w.id);

test('ランチャーのボタンでアプリを開く (システムのアプリは apps.launch を尋ねない)', () => {
  const p = make();
  assert.equal(p.launch('launcher'), '');
  const labels = [...p.shell.tree('launcher').nodes.values()].filter((n) => n.role === 'button').map((n) => n.label);
  assert.deepEqual(labels, ['Todo を開く', 'Settings を開く', 'Notes を開く', 'Calendar を開く']); // システムのアプリ (ランチャー) は一覧に出ない
  p.shell.dispatch({ app: 'launcher', target: 'open-settings', action: 'click', by: 'human' });
  assert.deepEqual(ids(p), ['todo', 'notes', 'launcher', 'settings']);
  assert.equal(p.registry.appOf('settings'), 'settings');
  assert.equal(p.services.prompts.length, 0);
});

test('複数の窓を開けるアプリは id-2, id-3 になり、1 つだけのアプリは二重に開けない', () => {
  const p = make();
  assert.equal(p.launch('todo'), '');
  assert.equal(p.launch('todo'), '');
  assert.deepEqual(ids(p), ['todo', 'notes', 'todo-2', 'todo-3']);
  assert.equal(p.registry.appOf('todo-3'), 'todo');
  assert.equal(p.launch('settings'), '');
  assert.match(p.launch('settings'), /開けない/);
  assert.match(p.launch('nope'), /開けない/);
});

test('窓を閉じると、その id は次に開く窓で再び使える', () => {
  const p = make();
  p.launch('todo');
  assert.equal(p.close('todo-2'), '');
  assert.equal(p.registry.appOf('todo-2'), undefined);
  assert.deepEqual(ids(p), ['todo', 'notes']);
  p.launch('todo');
  assert.deepEqual(ids(p), ['todo', 'notes', 'todo-2']);
});

test('agent は launch / close を提案でき、close は自動承認されない', () => {
  const p = make();
  p.shell.policy.autoApproveSafe = true;
  const a = p.shell.propose('設定を開く', [{ kind: 'layout', op: { type: 'launch', app: 'settings' } }]);
  assert.ok(a.ok);
  assert.equal(a.proposal.status, 'done');
  assert.ok(ids(p).includes('settings'));
  const c = p.shell.propose('設定を閉じる', [{ kind: 'layout', op: { type: 'close', app: 'settings' } }]);
  assert.ok(c.ok);
  assert.equal(c.proposal.sensitive, true);
  assert.equal(c.proposal.status, 'pending');
  p.shell.approve(c.proposal.id);
  assert.ok(!ids(p).includes('settings'));
});

test('ask の能力は要求を出して pending。答えられるのは人だけで、許可は保存される', () => {
  const kv = memoryKeyValue();
  const p = make(kv);
  const notes = p.shell.tree('notes');
  assert.ok(notes.get('paste'));
  p.shell.dispatch({ app: 'notes', target: 'paste', action: 'click', by: 'human' });
  assert.equal(p.services.prompts.length, 1);
  const pr = p.services.prompts[0];
  assert.equal(pr.capability, 'clipboard.read');
  assert.equal(p.services.answer(pr.id, true, 'agent'), '許可の要求に答えられるのは人だけ');
  assert.equal(pr.status, 'pending');
  // 書き込みは既定で許可。コピーしてから、許可して貼り付ける
  p.shell.dispatch({ app: 'notes', target: 'copy', action: 'click', by: 'human' });
  assert.equal(p.services.answer(pr.id, true, 'human'), '');
  assert.match(p.shell.tree('notes').get('status')!.label!, /許可された/);
  p.shell.dispatch({ app: 'notes', target: 'paste', action: 'click', by: 'human' });
  assert.equal(p.shell.tree('notes').get('input')!.value, '外周なら全体の端へ');
  assert.deepEqual(
    p.services.audit.filter((a) => a.capability.startsWith('clipboard')).map((a) => [a.capability, a.result]),
    [
      ['clipboard.read', 'pending'],
      ['clipboard.write', 'ok'],
      ['clipboard.read', 'ok'],
    ],
  );
  // 次の起動でも許可は残る
  const q = make(kv);
  assert.equal(q.services.grant('notes', 'clipboard.read'), 'granted');
});

test('宣言していない能力は尋ねることもできない', () => {
  const p = make();
  const sv = p.services.servicesFor('settings', 'settings');
  const r = sv.clipboard.write('x');
  assert.deepEqual(r, { ok: false, reason: 'undeclared' });
  assert.equal(p.services.prompts.length, 0);
});

test('保存領域はアプリごとで、同じアプリの窓と次の起動で共有される', () => {
  const kv = memoryKeyValue();
  const p = make(kv, { initial: ['todo'] });
  p.shell.dispatch({ app: 'todo', target: 'input', action: 'setValue', data: '卵', by: 'human' });
  p.shell.dispatch({ app: 'todo', target: 'add', action: 'click', by: 'human' });
  p.launch('todo');
  const labels = (id: string, q = p) => q.shell.tree(id).get('list')!.children!.map((c) => q.shell.tree(id).get(c)!.label);
  assert.ok(labels('todo-2').includes('卵'));
  const q = make(kv, { initial: ['todo'] });
  assert.ok(labels('todo', q).includes('卵'));
  // ほかのアプリからは見えない
  assert.equal(kv.get('app.notes.items'), undefined);
});

test('通知は許可されてから届く', () => {
  const p = make(memoryKeyValue(), { initial: ['todo'] });
  const done = () => {
    for (const id of ['i1', 'i3']) p.shell.dispatch({ app: 'todo', target: id, action: 'click', by: 'human' });
  };
  done();
  assert.equal(p.services.notifications.length, 0);
  assert.equal(p.services.prompts[0]?.capability, 'notifications');
  p.services.answer(p.services.prompts[0].id, true, 'human');
  done(); // 未完了に戻す
  done();
  assert.equal(p.services.notifications.length, 1);
  assert.equal(p.services.notifications[0].body, 'すべて完了した');
  p.services.dismiss(p.services.notifications[0].id);
  assert.equal(p.services.notifications.length, 0);
});

test('セッション (開いている窓と配置) を保存して復元する', () => {
  const kv = memoryKeyValue();
  const p = make(kv);
  p.launch('todo');
  p.launch('settings');
  p.shell.minimizeApp('notes');
  const s = p.saveSession();
  assert.deepEqual(s.windows.map((w) => [w.id, w.app]), [['todo', 'todo'], ['notes', 'notes'], ['todo-2', 'todo'], ['settings', 'settings']]);
  const q = make(kv, { restore: true });
  assert.deepEqual(ids(q), ['todo', 'notes', 'todo-2', 'settings']);
  assert.deepEqual(q.shell.getLayout(), p.shell.getLayout());
  assert.equal(q.registry.appOf('todo-2'), 'todo');
  // 復元した窓の続きの id
  q.launch('todo');
  assert.ok(ids(q).includes('todo-3'));
});

test('壊れた・知らないアプリを含むセッションは捨てて、初期の窓で起動する', () => {
  const kv = memoryKeyValue();
  kv.set('platform.session', JSON.stringify({ v: 1, windows: [{ id: 'ghost', app: 'ghost' }], tiles: { kind: 'leaf', app: 'ghost' }, minimized: [], zoomed: null }));
  assert.deepEqual(ids(make(kv, { restore: true })), ['todo', 'notes']);
  kv.set('platform.session', '{not json');
  assert.deepEqual(ids(make(kv, { restore: true })), ['todo', 'notes']);
});
