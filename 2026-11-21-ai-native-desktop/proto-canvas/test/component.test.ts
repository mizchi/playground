// WIT のコンポーネント (Rust の Todo、component/todo.wasm) を shell に入れる。能力の境界と、境界のコスト
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';
import * as shim from '@bytecodealliance/preview2-shim';
import { instantiate } from '../component/gen/todo.js';
import { createComponentApp, hostImports, wasiImports } from '../src/apps/component-app.ts';
import type { ComponentExports } from '../src/apps/component-app.ts';
import { createTodoApp } from '../src/apps/todo.ts';
import { memoryStorage } from '../src/cache.ts';
import { Shell } from '../src/shell.ts';

const gen = path.join(import.meta.dirname, '..', 'component', 'gen');
const modules = new Map<string, WebAssembly.Module>();
const core = (p: string) => modules.get(p) ?? modules.set(p, new WebAssembly.Module(fs.readFileSync(path.join(gen, p)))).get(p)!;
const measure = (s: string, size: number) => [...s].length * size * 0.6;

const wasmApp = (preopens: 'none' | 'root') =>
  createComponentApp('wasm', (host) => {
    const imports = { ...wasiImports(shim as unknown as Record<string, Record<string, unknown>>, { preopens, version: '0.2.9' }), ...hostImports(host) };
    return instantiate(core, imports as never) as unknown as ComponentExports;
  });
const node = (shell: Shell, label: string) => shell.tree('wasm').walk().find((n) => n.label === label);

test('コンポーネントのアプリが、意味ツリーを出して shell に入る', () => {
  const shell = new Shell([wasmApp('none')], measure, memoryStorage());
  const roles = shell.tree('wasm').walk().map((n) => `${n.role}:${n.label}`);
  assert.deepEqual(roles.slice(0, 3), ['x-window:Todo (wasm)', 'heading:やること', 'list:タスク']);
  const f = shell.frame(1000, 700, 0);
  assert.ok(f.ops.some((o) => o[0] === 'text' && o[1].str === '牛乳を買う'), '試作の描画で描ける');
});

test('人の操作も agent の提案も、JS のアプリと同じ権限モデルで通る', () => {
  const shell = new Shell([wasmApp('none')], measure, memoryStorage());
  shell.dispatch({ app: 'wasm', target: 'i1', action: 'click', by: 'human' });
  assert.equal(node(shell, '牛乳を買う')!.checked, true);
  assert.equal(shell.dispatch({ app: 'wasm', target: 'i3', action: 'click', by: 'agent' }).result, 'denied');
  const r = shell.propose('卵を追加', [
    { app: 'wasm', target: 'input', action: 'setValue', data: '卵を買う' },
    { app: 'wasm', target: 'add', action: 'click' },
  ]);
  assert.ok(r.ok);
  shell.approve(r.proposal.id);
  assert.equal(node(shell, '卵を買う')?.role, 'checkbox');
  shell.dispatch({ app: 'wasm', target: 'input', action: 'replaceSelectedText', data: 'あ', by: 'human' });
  assert.equal(node(shell, '新しいタスク')!.value, 'あ', 'replaceSelectedText は WIT の replace-selected-text として届く');
});

test('能力はホストが決める: preopens を渡さなければファイルは読めず、渡せば読める', () => {
  const denied = new Shell([wasmApp('none')], measure, memoryStorage());
  denied.dispatch({ app: 'wasm', target: 'probe', action: 'click', by: 'human' });
  const st = denied.tree('wasm').get('status')!.label!;
  assert.match(st, /ファイルは読めない/);

  const granted = new Shell([wasmApp('root')], measure, memoryStorage());
  granted.dispatch({ app: 'wasm', target: 'probe', action: 'click', by: 'human' });
  assert.match(granted.tree('wasm').get('status')!.label!, /読めた/, 'ホストがルートを開いて渡せば読める');
});

test('境界のコスト: 1 操作 (on-action → tree-update の emit → 木の更新) を JS のアプリと比べる', () => {
  const run = (shell: Shell, app: string) => {
    for (let i = 0; i < 50; i++) shell.dispatch({ app, target: 'i1', action: 'click', by: 'human' });
    const n = 1000;
    const t = performance.now();
    for (let i = 0; i < n; i++) shell.dispatch({ app, target: 'i1', action: 'click', by: 'human' });
    return ((performance.now() - t) / n) * 1000;
  };
  const js = run(new Shell([createTodoApp()], measure, memoryStorage()), 'todo');
  const wasm = run(new Shell([wasmApp('none')], measure, memoryStorage()), 'wasm');
  console.log(`     1 操作あたり: JS ${js.toFixed(1)}µs / wasm コンポーネント ${wasm.toFixed(1)}µs`);
  assert.ok(wasm < 1000, '1 操作 1ms 未満');
});

test('読み込み時の検査: コンポーネントの import を一覧できる (WIT では host だけだが、Rust の std が WASI を持ち込む)', async () => {
  const { execFileSync } = await import('node:child_process');
  const jco = path.join(import.meta.dirname, '..', 'node_modules', '.bin', 'jco');
  const wit = execFileSync(jco, ['wit', path.join(import.meta.dirname, '..', 'component', 'todo.wasm')], { encoding: 'utf8' });
  const imports = [...wit.matchAll(/^\s*import ([\w:/.-]+)@/gm)].map((m) => m[1]);
  assert.ok(imports.includes('gui:app/host'));
  assert.ok(imports.includes('wasi:filesystem/preopens'), 'std がファイルシステムの import を持ち込む');
  // ホストの方針: 許可した import 以外を持つコンポーネントは読み込まない、という検査ができる
  const allow = new Set(['gui:app/host', 'gui:app/types']);
  const extra = imports.filter((i) => !allow.has(i));
  assert.ok(extra.length > 0);
  console.log(`     import ${imports.length} 個 (WIT の world 以外: ${extra.length} 個、例: ${extra.slice(0, 3).join(', ')})`);
});
