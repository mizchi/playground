// ネイティブのホストのスクリーンリーダー対応 (Linux の AT-SPI) の通し確認。
// D-Bus のセッションと AT-SPI のバスを立て、ホスト (--a11y) の意味ツリーを AT-SPI のクライアント (pyatspi) から読み、操作する。
// クライアントは既存アプリの取り込みに使った legacy/atspi_bridge.py (dump / action / set-text) と、イベントを聞く e2e/atspi-listen.py。
// 使い方: node e2e/native-a11y.ts   (dbus-run-session・at-spi2-core・python3-pyatspi (Python 3.12) と、native/build.sh の描画プロセスが要る)
import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createInterface } from 'node:readline';

const root = path.join(import.meta.dirname, '..');
// 自分を D-Bus のセッションの中で起こし直す (ホストもクライアントも、同じセッションの AT-SPI のバスを使う)
if (!process.env.NATIVE_A11Y_INNER) {
  const r = spawnSync('dbus-run-session', ['--', process.execPath, import.meta.filename, ...process.argv.slice(2)], { stdio: 'inherit', env: { ...process.env, NATIVE_A11Y_INNER: '1' } });
  process.exit(r.status ?? 1);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const env = { ...process.env, VK_ICD_FILENAMES: process.env.VK_ICD_FILENAMES ?? '/opt/lavapipe/lvp_icd.json' };
let failures = 0;
async function step(name: string, fn: () => Promise<void>) {
  try {
    await fn();
    console.log(`ok   ${name}`);
  } catch (e) {
    failures++;
    console.log(`FAIL ${name}\n     ${String((e as Error).message).split('\n').join('\n     ')}`);
  }
}

const bus = spawn('/usr/libexec/at-spi-bus-launcher', ['--launch-immediately'], { stdio: 'ignore' });
await sleep(1000);

// AT-SPI のイベントを聞く (スクリーンリーダーの代わり)
const events: { type: string; name: string; text: string | null }[] = [];
const listener = spawn('/usr/bin/python3.12', [path.join(root, 'e2e', 'atspi-listen.py')], { stdio: ['ignore', 'pipe', 'ignore'] });
let listening: () => void;
const listenReady = new Promise<void>((r) => (listening = r));
createInterface({ input: listener.stdout }).on('line', (l) => {
  const e = JSON.parse(l);
  if (e.type === 'listening') listening();
  events.push(e);
});
// イベントを聞き始めてからホストを起こす (先に出たイベントを取りこぼさないように)
await Promise.race([listenReady, sleep(10000).then(() => Promise.reject(new Error('AT-SPI のイベントを聞き始められない')))]);

const host = spawn(process.execPath, [path.join(root, 'native', 'host.ts'), '--headless', '1500x860', '--control', '--a11y', '--fresh', '--kv', path.join(mkdtempSync(path.join(tmpdir(), 'a11y-')), 'kv.json')], { env, stdio: ['pipe', 'pipe', 'pipe'] }) as ChildProcessWithoutNullStreams;
const stderr: string[] = [];
host.stderr.on('data', (d) => stderr.push(String(d)));
const replies = new Map<number, (v: Record<string, unknown>) => void>();
let a11yReady: () => void;
const ready = new Promise<void>((r) => (a11yReady = r));
createInterface({ input: host.stdout }).on('line', (l) => {
  const v = JSON.parse(l) as Record<string, unknown>;
  if (v.t === 'a11y-ready') a11yReady();
  if (typeof v.id === 'number') replies.get(v.id)?.(v);
});
let seq = 0;
const cmd = (c: Record<string, unknown>) =>
  new Promise<Record<string, unknown>>((ok, ng) => {
    const id = ++seq;
    const t = setTimeout(() => ng(new Error(`応答が無い ${JSON.stringify(c)}\n${stderr.join('')}`)), 20000);
    replies.set(id, (v) => {
      clearTimeout(t);
      ok(v);
    });
    host.stdin.write(`${JSON.stringify({ ...c, id })}\n`);
  });
const tree = async (app: string) => ((await cmd({ t: 'tree', app })) as { tree: { nodes: { id: string; checked?: boolean; value?: string }[] }[] }).tree[0].nodes;

interface Acc {
  path: string;
  role: string;
  name: string;
  states: string[];
  actions: string[];
  text: string | null;
  editable: boolean;
  children: Acc[];
}
const bridge = (...args: string[]) => JSON.parse(execFileSync('/usr/bin/python3.12', [path.join(root, 'legacy', 'atspi_bridge.py'), ...args], { encoding: 'utf8', timeout: 15000 }));
const dump = () => bridge('dump', 'AI-native desktop') as Acc;
const find = (a: Acc, f: (x: Acc) => boolean): Acc | undefined => (f(a) ? a : a.children.map((c) => find(c, f)).find(Boolean));
const until = async (f: () => Promise<boolean> | boolean, what: string) => {
  for (let i = 0; i < 50; i++) {
    if (await f()) return;
    await sleep(100);
  }
  throw new Error(`${what} にならない`);
};

await step('AT-SPI に「AI-native desktop」が出る。窓ごとの frame と、人が答えるパネルの frame', async () => {
  await Promise.race([ready, sleep(20000).then(() => Promise.reject(new Error(`a11y-ready が来ない\n${stderr.join('')}`)))]);
  await cmd({ t: 'state' });
  const app = dump();
  assert.equal(app.role, 'application');
  assert.deepEqual(app.children.map((c) => [c.role, c.name]), [
    ['frame', 'Todo'],
    ['frame', 'Settings'],
    ['frame', 'Notes'],
    ['frame', 'システム (ホストの UI)'],
  ]);
  const i1 = find(app, (x) => x.name === '牛乳を買う')!;
  assert.equal(i1.role, 'check box');
  assert.ok(i1.states.includes('checkable') && !i1.states.includes('checked'));
  assert.deepEqual(i1.actions, ['click']);
  const input = find(app, (x) => x.name === '新しいタスク')!;
  assert.equal(input.role, 'entry');
  assert.ok(input.editable);
  assert.equal(find(app, (x) => x.name === '利用状況を送信')!.role, 'toggle button');
});

await step('スクリーンリーダーの click は人の操作として shell に届く (Todo のチェック)', async () => {
  const i1 = find(dump(), (x) => x.name === '牛乳を買う')!;
  assert.deepEqual(bridge('action', 'AI-native desktop', i1.path, 'click'), { ok: true });
  await until(async () => (await tree('todo')).find((n) => n.id === 'i1')!.checked === true, 'i1 が完了');
  await until(() => find(dump(), (x) => x.name === '牛乳を買う')!.states.includes('checked'), 'AT-SPI でも checked');
});

await step('文字の入力 (EditableText) と、フォーカスの移動がスクリーンリーダーに知らされる (focused のイベント)', async () => {
  const input = find(dump(), (x) => x.name === '新しいタスク')!;
  assert.deepEqual(bridge('set-text', 'AI-native desktop', input.path, '卵を買う'), { ok: true });
  await until(async () => (await tree('todo')).find((n) => n.id === 'input')!.value === '卵を買う', '入力欄が 卵を買う');
  assert.equal(find(dump(), (x) => x.name === '新しいタスク')!.text, '卵を買う');
  const add = find(dump(), (x) => x.name === '追加' && x.role === 'push button')!;
  bridge('action', 'AI-native desktop', add.path, 'click');
  await until(() => events.some((e) => e.type === 'object:state-changed:focused' && e.name === '追加'), 'focused のイベント');
  await until(async () => (await tree('todo')).some((n) => (n as { label?: string }).label === '卵を買う'), '項目が増える');
});

await step('文字の境界 (文字・単語・行)、位置からの検索、位置と大きさ、長さ付きの挿入を AT-SPI の形で答える', async () => {
  const probe = (...args: string[]) => JSON.parse(execFileSync('/usr/bin/python3.12', ['-W', 'ignore::DeprecationWarning', path.join(root, 'e2e', 'atspi-probe.py'), ...args], { encoding: 'utf8', timeout: 15000 }));
  // 入力欄に「buy 卵」と入れて、境界を問い合わせる
  let input = find(dump(), (x) => x.name === '新しいタスク')!;
  bridge('set-text', 'AI-native desktop', input.path, 'buy 卵');
  await until(async () => (await tree('todo')).find((n) => n.id === 'input')!.value === 'buy 卵', '入力欄');
  assert.deepEqual(probe('text-at', input.path, '1', '0'), { text: 'u', start: 1, end: 2 });
  assert.deepEqual(probe('text-at', input.path, '5', '1'), { text: '卵', start: 4, end: 5 });
  assert.deepEqual(probe('string-at', input.path, '0', '1'), { text: 'buy', start: 0, end: 3 });
  assert.deepEqual(probe('text-at', input.path, '2', '5'), { text: 'buy 卵', start: 0, end: 5 });
  // 挿入の長さは UTF-8 のバイト数。「目玉」(6 バイト) の 3 バイトだけなら「目」
  assert.deepEqual(probe('insert', input.path, '4', '目玉', '3'), { ok: true });
  await until(async () => (await tree('todo')).find((n) => n.id === 'input')!.value === 'buy 目卵', '3 バイトだけ入る');
  input = find(dump(), (x) => x.name === '新しいタスク')!;
  const r = probe('size', input.path);
  assert.ok(r.w > 100 && r.h > 10, JSON.stringify(r));
  // 入力欄の真ん中の点は、入力欄
  assert.deepEqual(probe('at-point', '0', String(r.x + r.w / 2), String(r.y + r.h / 2)), { name: '新しいタスク' });
});

await step('状態と構造の変化はイベントで届く: checked の変化、子 (項目) の追加は親のリストに位置と子を付けて', async () => {
  const before = events.length;
  const i2 = find(dump(), (x) => x.name === '請求書を送る')!;
  bridge('action', 'AI-native desktop', i2.path, 'click');
  await until(() => events.slice(before).some((e) => e.type === 'object:state-changed:checked'), 'checked のイベント');
  const add = find(dump(), (x) => x.name === '追加' && x.role === 'push button')!;
  bridge('action', 'AI-native desktop', add.path, 'click');
  await until(() => events.slice(before).some((e) => e.type === 'object:children-changed:add' && e.name === 'タスク'), 'リスト「タスク」の children-changed:add');
});

await step('AI の提案は announcement のイベントで届き、パネルの「承認」をスクリーンリーダーから押せる', async () => {
  const p = (await cmd({ t: 'propose', intent: 'コンパクト表示にする', steps: [{ app: 'settings', target: 'compact', action: 'click' }] })) as { result: { proposal: { id: string } } };
  const id = p.result.proposal.id;
  await until(() => events.some((e) => e.type === 'object:announcement' && (e.text ?? '').includes(`AI の提案 ${id}`)), '提案の読み上げ');
  let approve: Acc | undefined;
  await until(() => !!(approve = find(dump(), (x) => x.role === 'push button' && x.name === '承認')), 'パネルの承認ボタン');
  // 「承認」だけでは何に答えるか分からないので、説明に提案の全文が入る
  const d = JSON.parse(execFileSync('/usr/bin/python3.12', ['-W', 'ignore::DeprecationWarning', path.join(root, 'e2e', 'atspi-probe.py'), 'desc', approve!.path], { encoding: 'utf8', timeout: 15000 }));
  assert.match(d.description, new RegExp(`AI の提案 ${id}: コンパクト表示にする。1\\. settings / compact を click`));
  bridge('action', 'AI-native desktop', approve!.path, 'click');
  await until(async () => ((await cmd({ t: 'state' })) as { proposals: { id: string; status: string }[] }).proposals.find((x) => x.id === id)?.status === 'done', '提案が実行される');
});

host.stdin.write(`${JSON.stringify({ t: 'quit' })}\n`);
await Promise.race([new Promise((r) => host.on('exit', r)), sleep(5000)]);
listener.kill();
bus.kill();
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
