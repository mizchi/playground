// アプリのパッケージ (app.json + WIT のコンポーネント): 能力 = import、宣言していない import を求めるものは入れない
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { test } from 'node:test';
import { pathToFileURL } from 'node:url';
import * as shim from '@bytecodealliance/preview2-shim';
import { builtinManifests } from '../src/apps/manifests.ts';
import { memoryStorage } from '../src/cache.ts';
import { loadPackage, parsePackage } from '../src/platform/packages.ts';
import type { ComponentModule, PackageSource } from '../src/platform/packages.ts';
import { createPlatform } from '../src/platform/platform.ts';
import { memoryKeyValue } from '../src/platform/services.ts';
import type { KeyValue } from '../src/platform/services.ts';

const root = path.join(import.meta.dirname, '..');
const gen = path.join(root, 'component', 'gen');
const wasi = shim as unknown as Record<string, Record<string, unknown>>;
const measure = (s: string, size: number) => [...s].length * size * 0.6;

/** テストでは、app.json は packages/、生成物は component/gen/ から読む */
const fsSource = (patch: (m: Record<string, unknown>) => Record<string, unknown> = (m) => m): PackageSource => ({
  manifest: async () => patch(JSON.parse(await readFile(path.join(root, 'packages', 'todo-rs', 'app.json'), 'utf8'))),
  module: async (p) => (await import(pathToFileURL(path.join(gen, p)).href)) as ComponentModule,
  bytes: (p) => readFile(path.join(gen, p)),
});

const make = async (kv: KeyValue = memoryKeyValue()) => {
  const r = await loadPackage(fsSource(), wasi);
  assert.ok(r.ok, JSON.stringify(!r.ok && r.errors));
  const p = createPlatform({ apps: builtinManifests, initial: ['todo'], measure, templates: memoryStorage(), kv });
  assert.equal(p.install(r.manifest), '');
  p.shell.animations = false;
  p.shell.frame(1400, 800);
  return p;
};

test('パッケージを入れると、ランチャーから開ける WIT のコンポーネントのアプリになる', async () => {
  const p = await make();
  assert.ok(p.registry.list().some((m) => m.id === 'todo-rs' && m.kind === 'component'));
  assert.equal(p.launch('todo-rs'), '');
  assert.equal(p.shell.tree('todo-rs').get('root')!.label, 'Todo (wasm)');
});

test('能力 storage = import gui:app/storage: 項目はアプリの保存領域に残り、次の起動でも読める', async () => {
  const kv = memoryKeyValue();
  const p = await make(kv);
  p.launch('todo-rs');
  p.shell.dispatch({ app: 'todo-rs', target: 'input', action: 'setValue', data: '卵', by: 'human' });
  p.shell.dispatch({ app: 'todo-rs', target: 'add', action: 'click', by: 'human' });
  assert.match(kv.get('app.todo-rs.items')!, /\t0\t卵$/m);
  assert.ok(p.services.audit.some((a) => a.app === 'todo-rs' && a.capability === 'storage' && a.op === 'set items' && a.result === 'ok'));
  const q = await make(kv);
  q.launch('todo-rs');
  const labels = q.shell.tree('todo-rs').get('list')!.children!.map((c) => q.shell.tree('todo-rs').get(c)!.label);
  assert.deepEqual(labels, ['牛乳を買う', '請求書を送る', 'PR をレビューする', '卵']);
});

test('人が保存領域を拒否していれば、読めないので書かない (既定の項目で保存済みの項目を上書きしない)', async () => {
  const kv = memoryKeyValue();
  kv.set('app.todo-rs.items', 'i7\t0\t保存済みの項目');
  const p = await make(kv);
  p.services.setGrant('todo-rs', 'storage', 'denied');
  p.launch('todo-rs');
  // 読めないので既定の項目で始まる。操作しても保存済みの項目は消えない
  p.shell.dispatch({ app: 'todo-rs', target: 'i1', action: 'click', by: 'human' });
  assert.equal(p.shell.tree('todo-rs').get('status')!.label, '保存領域を読めないので、保存しない');
  assert.equal(kv.get('app.todo-rs.items'), 'i7\t0\t保存済みの項目');
  // あとで許可されたら、書く前に読み直して保存済みの項目に戻る
  p.services.setGrant('todo-rs', 'storage', 'granted');
  p.shell.dispatch({ app: 'todo-rs', target: 'i1', action: 'click', by: 'human' });
  assert.match(p.shell.tree('todo-rs').get('status')!.label!, /保存済みの項目を読み直した/);
  const t = p.shell.tree('todo-rs');
  assert.deepEqual(t.get('list')!.children!.map((c) => t.get(c)!.label), ['保存済みの項目']);
  assert.equal(kv.get('app.todo-rs.items'), 'i7\t0\t保存済みの項目');
});

test('項目の文字のタブ・改行・バックスラッシュは、保存して読み直しても変わらない', async () => {
  const kv = memoryKeyValue();
  const p = await make(kv);
  p.launch('todo-rs');
  const text = 'タブ\tと\n改行と \\ と \\t';
  p.shell.dispatch({ app: 'todo-rs', target: 'input', action: 'setValue', data: text, by: 'human' });
  p.shell.dispatch({ app: 'todo-rs', target: 'add', action: 'click', by: 'human' });
  assert.equal(kv.get('app.todo-rs.items')!.split('\n').length, 4, '1 項目 1 行のまま');
  const q = await make(kv);
  q.launch('todo-rs');
  const t = q.shell.tree('todo-rs');
  assert.equal(t.get(t.get('list')!.children!.at(-1)!)!.label, text);
});

test('world gui-app の app (start / on-action) を export しないコンポーネントは入れない', async () => {
  const src = fsSource();
  const r = await loadPackage({ ...src, module: async () => ({ instantiate: () => ({}) }) }, wasi);
  assert.ok(!r.ok);
  assert.deepEqual(r.errors, ['gui:app/app (start と on-action) を export していない']);
});

test('宣言していない能力の import を求めるコンポーネントは入れない (どの能力が足りないかを言う)', async () => {
  const r = await loadPackage(fsSource((m) => ({ ...m, capabilities: [] })), wasi);
  assert.ok(!r.ok);
  assert.deepEqual(r.errors, ['gui:app/storage を import している (能力 storage を宣言していない)']);
});

test('app.json の検査: パッケージの外のパス、知らない能力、形の崩れを断る', () => {
  const base = { id: 'x', title: 'X', description: 'x', kind: 'component', capabilities: [], multiInstance: false, entry: 'x.js', core: ['x.wasm'] };
  assert.ok(parsePackage(base).ok);
  const bad = (patch: Record<string, unknown>) => {
    const r = parsePackage({ ...base, ...patch });
    return r.ok ? [] : r.errors;
  };
  assert.match(bad({ entry: '../../etc/passwd' }).join(), /相対パス/);
  assert.match(bad({ core: ['/abs.wasm'] }).join(), /相対パス/);
  assert.match(bad({ capabilities: ['root'] }).join(), /capabilities/);
  assert.match(bad({ id: 'Bad Id' }).join(), /id/);
  assert.match(bad({ kind: 'js' }).join(), /kind/);
  assert.ok(!parsePackage(null).ok);
});

test('入っているアプリ (同梱のアプリを含む) と同じ id のパッケージは入れない (なりすまし)', async () => {
  const r = await loadPackage(fsSource((m) => ({ ...m, id: 'settings', title: 'Settings' })), wasi);
  assert.ok(r.ok);
  const p = createPlatform({ apps: builtinManifests, initial: ['todo'], measure, templates: memoryStorage(), kv: memoryKeyValue() });
  assert.match(p.install(r.manifest), /すでに入っている/);
  assert.equal(p.registry.manifest('settings')!.kind, 'js');
});
