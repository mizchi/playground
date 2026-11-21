import assert from 'node:assert/strict';
import { test } from 'node:test';
import { AnimatedRect, Spring, TILE_SPRING } from '../src/anim.ts';
import { createNotesApp } from '../src/apps/notes.ts';
import { createSettingsApp } from '../src/apps/settings.ts';
import { createTodoApp } from '../src/apps/todo.ts';
import { memoryStorage } from '../src/cache.ts';
import { Shell } from '../src/shell.ts';
import { applyDrop, computeRects, hitDrop, leaf, leaves, removeLeaf } from '../src/tiling.ts';
import type { TileNode } from '../src/tiling.ts';

const area = { x: 0, y: 0, w: 1000, h: 600 };
const three: TileNode = { kind: 'split', dir: 'row', ratio: 0.5, a: leaf('a'), b: { kind: 'split', dir: 'column', ratio: 0.5, a: leaf('b'), b: leaf('c') } };

test('tiling: 葉を取り除くと兄弟が場所を引き継ぐ', () => {
  assert.deepEqual(removeLeaf(three, 'b'), { kind: 'split', dir: 'row', ratio: 0.5, a: leaf('a'), b: leaf('c') });
  assert.deepEqual(removeLeaf(leaf('a'), 'a'), null);
});

test('tiling: 別のペーンの右端に落とすと、そのペーンを左右に分割する', () => {
  const next = applyDrop(three, 'a', { kind: 'split', target: 'c', edge: 'right' })!;
  assert.deepEqual(next, { kind: 'split', dir: 'column', ratio: 0.5, a: leaf('b'), b: { kind: 'split', dir: 'row', ratio: 0.5, a: leaf('c'), b: leaf('a') } });
});

test('tiling: 全体の上端に落とすと、全体を上下に分けて上に置く', () => {
  const next = applyDrop(three, 'c', { kind: 'split', target: null, edge: 'top' })!;
  assert.equal(next.kind, 'split');
  assert.deepEqual(leaves(next), ['c', 'a', 'b']);
  assert.equal(next.kind === 'split' && next.dir, 'column');
});

test('tiling: 中央は入れ替え、自分自身へのドロップは無効', () => {
  assert.deepEqual(leaves(applyDrop(three, 'a', { kind: 'swap', target: 'c' })!), ['c', 'b', 'a']);
  assert.equal(applyDrop(three, 'a', { kind: 'swap', target: 'a' }), null);
  assert.equal(applyDrop(three, 'a', { kind: 'split', target: 'a', edge: 'left' }), null);
});

test('tiling: ポインタの位置で候補が決まる (外周の帯 > ペーンの端 > 中央)', () => {
  const { tiles } = computeRects(three, area, 8);
  const c = tiles.get('c')!;
  assert.deepEqual(hitDrop(three, area, 8, 'a', 5, 300)?.drop, { kind: 'split', target: null, edge: 'left' });
  assert.deepEqual(hitDrop(three, area, 8, 'a', c.x + c.w - 40, c.y + c.h / 2)?.drop, { kind: 'split', target: 'c', edge: 'right' });
  assert.deepEqual(hitDrop(three, area, 8, 'a', c.x + c.w / 2, c.y + c.h / 2)?.drop, { kind: 'swap', target: 'c' });
  const prev = hitDrop(three, area, 8, 'a', c.x + c.w / 2, c.y + c.h - 40)!;
  assert.deepEqual(prev.drop, { kind: 'split', target: 'c', edge: 'bottom' });
  assert.ok(Math.abs(prev.preview.h - (c.h - 8) / 2) < 1e-9, 'プレビューは対象の下半分');
  assert.equal(hitDrop(three, area, 8, 'a', 200, 300), null, '自分の上では何もしない');
});

test('anim: ばねは目標に収束し、臨界減衰なら行き過ぎない', () => {
  const s = new Spring(0, TILE_SPRING);
  s.target = 100;
  let max = 0;
  for (let i = 0; i < 120; i++) {
    s.step(1 / 60);
    max = Math.max(max, s.x);
  }
  assert.equal(s.x, 100);
  assert.ok(max <= 100.0001);
  const r = new AnimatedRect({ x: 0, y: 0, w: 10, h: 10 }, TILE_SPRING);
  r.target = { x: 50, y: 0, w: 10, h: 10 };
  assert.equal(r.step(1 / 60), false);
  assert.ok(r.value.x > 0 && r.value.x < 50, '途中の値');
});

const make = () => {
  const measure = (s: string, size: number) => [...s].length * size * 0.6;
  const shell = new Shell([createTodoApp(), createSettingsApp(), createNotesApp()], measure, memoryStorage());
  return shell;
};

test('shell: タイトルバーを掴んで別のペーンの端へ運ぶと分割し、アニメーションして収まる', () => {
  const shell = make();
  let t = 1000;
  let f = shell.frame(1200, 700, t);
  const todo = f.windows.find((w) => w.app === 'todo')!.tile;
  const notes = f.windows.find((w) => w.app === 'notes')!.tile;
  shell.pointerDown(todo.x + 60, todo.y + 12);
  shell.pointerMove(todo.x + 62, todo.y + 13);
  assert.equal(shell.windowDrag()!.active, false, '5px まではドラッグにしない');
  shell.pointerMove(notes.x + notes.w - 40, notes.y + notes.h / 2);
  const g = shell.windowDrag()!;
  assert.deepEqual(g.drop, { kind: 'split', target: 'notes', edge: 'right' });
  assert.equal(g.valid, true);
  f = shell.frame(1200, 700, (t += 16));
  assert.ok(f.animating, 'プレビューが動いている');
  assert.ok(f.ops.some((o) => o[0] === 'text' && o[1].str === '右に分割'));
  shell.pointerUp();
  f = shell.frame(1200, 700, (t += 16));
  assert.ok(f.animating, '分割後のタイルが動いている');
  for (let i = 0; i < 120 && f.animating; i++) f = shell.frame(1200, 700, (t += 16));
  assert.equal(f.animating, false);
  const after = Object.fromEntries(f.windows.map((w) => [w.app, w.tile]));
  assert.ok(after.todo.x > after.notes.x, 'todo は notes の右');
  assert.ok(Math.abs(after.todo.y - after.notes.y) < 1e-6);
  assert.ok(after.settings.w > 1000, 'settings は上の段を全幅で使う');
});

test('shell: 最小幅を下回る分割は赤く表示して受け付けない', () => {
  const shell = make();
  shell.animations = false;
  let f = shell.frame(480, 700, 0);
  const todo = f.windows.find((w) => w.app === 'todo')!;
  const settings = f.windows.find((w) => w.app === 'settings')!.tile;
  shell.pointerDown(todo.tile.x + 60, todo.tile.y + 12);
  shell.pointerMove(settings.x + settings.w - 40, settings.y + settings.h / 2);
  const g = shell.windowDrag()!;
  assert.equal(g.valid, false);
  assert.match(g.reason, /最小幅/);
  const before = JSON.stringify(shell.tiles);
  shell.pointerUp();
  f = shell.frame(480, 700, 16);
  assert.equal(JSON.stringify(shell.tiles), before, '木は変わらない');
});

test('shell: Escape でドラッグを取り消す', () => {
  const shell = make();
  const f = shell.frame(1200, 700, 0);
  const todo = f.windows.find((w) => w.app === 'todo')!.tile;
  const before = JSON.stringify(shell.tiles);
  shell.pointerDown(todo.x + 60, todo.y + 12);
  shell.pointerMove(1150, 650);
  shell.input({ t: 'key', key: 'Escape' });
  shell.pointerUp();
  assert.equal(JSON.stringify(shell.tiles), before);
});
