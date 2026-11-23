// 通知を Canvas の上に: shell が窓より上の層に描く。出どころの名前はプラットフォームが付け、閉じられるのは人の入力だけ
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { builtinManifests } from '../src/apps/manifests.ts';
import { memoryStorage } from '../src/cache.ts';
import { createPlatform } from '../src/platform/platform.ts';
import { memoryKeyValue } from '../src/platform/services.ts';
import type { DrawOp } from '../src/protocol.ts';
import { NOTICE_MS } from '../src/shell.ts';
import type { Shell } from '../src/shell.ts';

const measure = (s: string, size: number) => [...s].length * size * 0.6;
const W = 1400;
const H = 800;
const make = () => {
  const p = createPlatform({ apps: builtinManifests, initial: ['todo', 'notes'], measure, templates: memoryStorage(), kv: memoryKeyValue() });
  for (const sh of [p.shell, p.agentShell]) {
    sh.animations = false;
    sh.frame(W, H, 1000);
  }
  p.services.setGrant('todo', 'notifications', 'granted');
  return p;
};
type P = ReturnType<typeof make>;
const notify = (p: P, title: string, body?: string, win = 'todo') => p.services.servicesFor('todo', win).notify(title, body);
const texts = (ops: DrawOp[]) => ops.filter((o) => o[0] === 'text').map((o) => (o[1] as { str: string }).str);
/** 通知の枠 (surface を accent で縁取った角丸) */
const boxes = (ops: DrawOp[]) =>
  ops.filter((o) => o[0] === 'rect' && (o[1] as { stroke?: string; radius?: number }).stroke === 'accent' && (o[1] as { radius?: number }).radius === 10 && (o[1] as { w: number }).w === 320).map((o) => o[1] as { x: number; y: number; w: number; h: number });
const click = (sh: Shell, r: { x: number; y: number; w: number; h: number }, dx = 20, dy = 20) => {
  sh.pointerDown(r.x + dx, r.y + dy);
  sh.pointerUp();
};

test('通知は窓より上 (display list の最後の方) に描かれ、出どころはアプリの manifest の名前', () => {
  const p = make();
  assert.ok(notify(p, 'すべて完了した', '3 件').ok);
  const f = p.shell.frame(W, H, 2000);
  const t = texts(f.ops);
  assert.ok(t.includes('Todo からの通知'));
  assert.ok(t.includes('すべて完了した') && t.includes('3 件'));
  // 窓の中身はすべて clip の中で、通知はそのあと (どの clip にも入っていない)
  let depth = 0;
  let lastWindowOp = -1;
  f.ops.forEach((o, i) => {
    if (o[0] === 'clip' || o[0] === 'layer') depth++;
    if (o[0] === 'pop') depth--;
    if (o[0] === 'clip') lastWindowOp = i;
  });
  assert.equal(depth, 0);
  const at = f.ops.findIndex((o) => o[0] === 'text' && (o[1] as { str: string }).str === 'Todo からの通知');
  assert.ok(at > lastWindowOp, '通知は最後の窓の中身より後に描く');
  const box = boxes(f.ops)[0];
  assert.ok(box.x + box.w <= W && box.y + box.h <= H, '画面の内側');
});

test('時間が経つと画面から消える (パネルの一覧には残る)。フェードの間だけ animating、そのあとは wakeAt', () => {
  const p = make();
  notify(p, 'A');
  assert.equal(p.shell.frame(W, H, 2000).animating, true, '出始めはフェード');
  const mid = p.shell.frame(W, H, 2500);
  assert.equal(mid.animating, false);
  assert.equal(mid.wakeAt, 2000 + NOTICE_MS - 300, '消え始める時刻に起こす');
  assert.ok(texts(p.shell.frame(W, H, 2000 + NOTICE_MS - 100).ops).includes('A'));
  assert.ok(!texts(p.shell.frame(W, H, 2000 + NOTICE_MS + 1).ops).includes('A'));
  assert.equal(p.services.notifications.length, 1, 'パネルの一覧には残る');
});

test('新しいものが下。出すのは 3 件まで。長い題名は … で切る', () => {
  const p = make();
  for (const s of ['1', '2', '3', '4']) notify(p, `通知 ${s}`);
  const f = p.shell.frame(W, H, 2500);
  const t = texts(f.ops);
  assert.ok(!t.includes('通知 1') && t.includes('通知 4'));
  const b = boxes(f.ops);
  assert.equal(b.length, 3);
  assert.ok(b[0].y > b[1].y && b[1].y > b[2].y, '最初に描くもの (新しい) が下');
  notify(p, 'と'.repeat(80));
  const long = texts(p.shell.frame(W, H, 3000).ops).find((s) => s.startsWith('とと'))!;
  assert.ok(long.endsWith('…') && measure(long, 14) <= 320 - 50);
});

test('× で閉じる。本文を押すと出した窓へフォーカスして閉じる。Escape は新しいものから閉じる', () => {
  const p = make();
  notify(p, 'A');
  let b = boxes(p.shell.frame(W, H, 2500).ops)[0];
  click(p.shell, b, b.w - 17, 17);
  assert.equal(p.services.notifications.length, 0);

  notify(p, 'B');
  p.shell.activate('notes', p.shell.tree('notes').root!);
  b = boxes(p.shell.frame(W, H, 3000).ops)[0];
  assert.equal(p.shell.accessibility().windows.find((w) => w.focused)?.app, 'notes');
  click(p.shell, b);
  assert.equal(p.services.notifications.length, 0);
  const focused = () => p.shell.accessibility().windows.find((w) => w.focused)?.app;
  assert.equal(focused(), 'todo');

  notify(p, 'C');
  notify(p, 'D');
  p.shell.frame(W, H, 3500);
  p.shell.input({ t: 'key', key: 'Escape' });
  assert.deepEqual(p.services.notifications.map((n) => n.title), ['C']);
});

test('通知の上を押しても、下の窓には届かない', () => {
  const p = make();
  notify(p, 'A');
  const b = boxes(p.shell.frame(W, H, 2500).ops)[0];
  const before = p.shell.audit.length;
  click(p.shell, b, 40, 40);
  assert.equal(p.shell.audit.length, before, '窓への操作 (focus など) は起きない');
});

test('どちらのワークスペースでも出る。切り替えても出し直さない', () => {
  const p = make();
  notify(p, 'A');
  p.shell.frame(W, H, 2000);
  assert.ok(texts(p.agentShell.frame(W, H, 2500).ops).includes('A'));
  assert.ok(!texts(p.agentShell.frame(W, H, 2000 + NOTICE_MS + 1).ops).includes('A'), '人の側で最初に出た時刻から数える');
});

test('本文を押すと、裏のタブにある窓やズームで隠れた窓も表に出してから移る', () => {
  const p = make();
  // Notes を Todo のタブに入れる (入れた Notes が表、Todo は裏)
  assert.equal(p.shell.applyLayout({ type: 'move', app: 'notes', drop: { kind: 'tab', target: 'todo' } }, 'human'), '');
  p.shell.frame(W, H, 1500);
  assert.ok(!p.shell.accessibility().windows.some((w) => w.app === 'todo'), 'Todo は裏のタブ');
  notify(p, 'A');
  let b = boxes(p.shell.frame(W, H, 2500).ops)[0];
  click(p.shell, b);
  p.shell.frame(W, H, 2600);
  assert.equal(p.shell.accessibility().windows.find((w) => w.focused)?.app, 'todo');

  assert.equal(p.launch('settings'), '');
  assert.equal(p.shell.applyLayout({ type: 'zoom', app: 'settings' }, 'human'), '');
  notify(p, 'B');
  b = boxes(p.shell.frame(W, H, 3000).ops)[0];
  click(p.shell, b);
  p.shell.frame(W, H, 3100);
  assert.equal(p.shell.getLayout().zoomed, null, 'ほかの窓のズームを解く');
  assert.equal(p.shell.accessibility().windows.find((w) => w.focused)?.app, 'todo');
});
