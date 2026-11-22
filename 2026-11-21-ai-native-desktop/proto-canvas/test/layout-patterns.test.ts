// レイアウトのパターン: タブ、差し込み、均等化、最小化とドック、ズーム、キー操作、ライブプレビュー、agent の提案
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createNotesApp } from '../src/apps/notes.ts';
import { createSettingsApp } from '../src/apps/settings.ts';
import { createTodoApp } from '../src/apps/todo.ts';
import { memoryStorage } from '../src/cache.ts';
import { Shell } from '../src/shell.ts';
import { activateTab, addToLargest, allApps, applyDrop, computeRects, equalize, hitDrop, leaf, leaves, neighbor, removeApp } from '../src/tiling.ts';
import type { TileNode } from '../src/tiling.ts';

const area = { x: 0, y: 0, w: 1200, h: 600 };
const row3: TileNode = { kind: 'split', dir: 'row', ratio: 0.5, a: leaf('a'), b: { kind: 'split', dir: 'row', ratio: 0.5, a: leaf('b'), b: leaf('c') } };
const three: TileNode = { kind: 'split', dir: 'row', ratio: 0.5, a: leaf('a'), b: { kind: 'split', dir: 'column', ratio: 0.5, a: leaf('b'), b: leaf('c') } };

// ---- 純粋な木の操作 ----

test('タブ: タイトルバーに落とすとグループになり、抜けると元に戻る', () => {
  const g = applyDrop(three, 'a', { kind: 'tab', target: 'c' })!;
  assert.deepEqual(g, { kind: 'split', dir: 'column', ratio: 0.5, a: leaf('b'), b: { kind: 'leaf', app: 'a', tabs: ['c', 'a'] } });
  assert.deepEqual(leaves(g), ['b', 'a'], '表に出るのは運んだ窓');
  assert.deepEqual(allApps(g).sort(), ['a', 'b', 'c']);
  assert.deepEqual(leaves(activateTab(g, 'c')), ['b', 'c']);
  assert.deepEqual(removeApp(g, 'a'), { kind: 'split', dir: 'column', ratio: 0.5, a: leaf('b'), b: leaf('c') });
  // グループから端へ引き出すと、グループが分かれる
  const out = applyDrop(g, 'a', { kind: 'split', target: 'c', edge: 'right' })!;
  assert.deepEqual(out, { kind: 'split', dir: 'column', ratio: 0.5, a: leaf('b'), b: { kind: 'split', dir: 'row', ratio: 0.5, a: leaf('c'), b: leaf('a') } });
  assert.equal(applyDrop(g, 'a', { kind: 'tab', target: 'c' }), null, '同じグループへは無効');
});

test('差し込み: 境界に落とすと間に入り、全体の 1/3 を取る', () => {
  const two: TileNode = { kind: 'split', dir: 'row', ratio: 0.5, a: leaf('a'), b: leaf('b') };
  const withC: TileNode = { kind: 'split', dir: 'column', ratio: 0.5, a: two, b: leaf('c') };
  const next = applyDrop(withC, 'c', { kind: 'insert', path: [0] })!;
  const { tiles } = computeRects(next, area, 0);
  assert.deepEqual(leaves(next), ['a', 'c', 'b']);
  for (const app of ['a', 'b', 'c']) assert.ok(Math.abs(tiles.get(app)!.w - 400) < 1e-6, `${app} は 400px`);
  assert.equal(applyDrop(two, 'a', { kind: 'insert', path: [] }), null, '直接の子を自分の境界に差し込むのは無効');
});

test('差し込み: 運ぶ窓を外すと道が変わっても、同じ境界に入る', () => {
  // a | (b / (c | d)) の c|d の境界 [1,1] へ、木の前の方にある a を差し込む
  const t: TileNode = { kind: 'split', dir: 'row', ratio: 0.5, a: leaf('a'), b: { kind: 'split', dir: 'column', ratio: 0.5, a: leaf('b'), b: { kind: 'split', dir: 'row', ratio: 0.5, a: leaf('c'), b: leaf('d') } } };
  const next = applyDrop(t, 'a', { kind: 'insert', path: [1, 1] })!;
  assert.deepEqual(leaves(next), ['b', 'c', 'a', 'd']);
});

test('均等化: どのペーンも同じ幅になる', () => {
  const { tiles } = computeRects(equalize(row3), area, 0);
  for (const app of ['a', 'b', 'c']) assert.ok(Math.abs(tiles.get(app)!.w - 400) < 1e-6);
});

test('一番大きいペーンを長い辺で分ける / 向きで隣を探す', () => {
  const t = addToLargest(three, 'd', area, 0);
  assert.deepEqual(leaves(t), ['a', 'd', 'b', 'c'], 'a (600x600) を左右に分ける');
  assert.equal(neighbor(three, area, 0, 'a', 'right'), 'b');
  assert.equal(neighbor(three, area, 0, 'c', 'top'), 'b');
  assert.equal(neighbor(three, area, 0, 'a', 'left'), null);
});

test('候補の判定: タイトルバー → タブ、境界 → 差し込み、受け口 → 最小化', () => {
  const { tiles, dividers } = computeRects(three, area, 8);
  const c = tiles.get('c')!;
  assert.deepEqual(hitDrop(three, area, 8, 'a', c.x + 100, c.y + 10, { titleH: 30 })?.drop, { kind: 'tab', target: 'c' });
  const bc = dividers.find((d) => d.dir === 'column')!;
  assert.deepEqual(hitDrop(three, area, 8, 'a', bc.rect.x + 200, bc.rect.y + 4)?.drop, { kind: 'insert', path: [1] });
  const dock = { x: 500, y: 500, w: 180, h: 36 };
  assert.deepEqual(hitDrop(three, area, 8, 'a', 510, 510, { dock })?.drop, { kind: 'minimize' });
});

// ---- shell での動き ----

const make = () => {
  const measure = (s: string, size: number) => [...s].length * size * 0.6;
  return new Shell([createTodoApp(), createSettingsApp(), createNotesApp()], measure, memoryStorage());
};
const W = 1400;
const H = 800;
/** タイトルバーを掴んで (x, y) まで運んで離す */
function dragTo(shell: Shell, app: string, x: number, y: number, t: { now: number }) {
  const f = shell.frame(W, H, (t.now += 16));
  const tile = f.windows.find((w) => w.app === app)!.tile;
  shell.pointerDown(tile.x + tile.w - 60, tile.y + 12);
  shell.pointerMove(tile.x + tile.w - 50, tile.y + 20);
  shell.pointerMove(x, y);
  const g = shell.windowDrag();
  shell.frame(W, H, (t.now += 16));
  shell.pointerUp();
  for (let i = 0; i < 200 && shell.frame(W, H, (t.now += 16)).animating; i++);
  return g;
}
const tileOf = (shell: Shell, app: string, t: { now: number }) => shell.frame(W, H, (t.now += 16)).windows.find((w) => w.app === app)?.tile;

test('shell: タイトルバーへ運ぶとタブになり、タブを押すと切り替わる', () => {
  const shell = make();
  const t = { now: 1000 };
  const notes = tileOf(shell, 'notes', t)!;
  const g = dragTo(shell, 'todo', notes.x + notes.w / 2, notes.y + 12, t);
  assert.deepEqual(g!.drop, { kind: 'tab', target: 'notes' });
  assert.deepEqual(leaves(shell.tiles), ['settings', 'todo']);
  const f = shell.frame(W, H, (t.now += 16));
  const tabs = f.ops.filter((o) => o[0] === 'text' && (o[1].str === 'Notes' || o[1].str === 'Todo'));
  assert.equal(tabs.length, 2, 'タイトルバーに 2 つのタブ');
  const notesTab = tabs.find((o) => o[0] === 'text' && o[1].str === 'Notes')!;
  if (notesTab[0] !== 'text') throw new Error();
  shell.pointerDown(notesTab[1].x + 5, notesTab[1].y);
  shell.pointerUp();
  assert.deepEqual(leaves(shell.tiles), ['settings', 'notes']);
});

test('shell: 受け口へ運ぶと最小化してドックに入り、札を押すと札の位置から戻る', () => {
  const shell = make();
  const t = { now: 1000 };
  shell.frame(W, H, t.now);
  const g = dragTo(shell, 'settings', W / 2, H - 8 - 36 - 40 + 18, t);
  assert.deepEqual(g!.drop, { kind: 'minimize' });
  assert.deepEqual(shell.minimized, ['settings']);
  assert.deepEqual(leaves(shell.tiles), ['todo', 'notes']);
  let f = shell.frame(W, H, (t.now += 16));
  const chip = f.ops.find((o) => o[0] === 'text' && o[1].str === 'Settings');
  assert.ok(chip && chip[0] === 'text', 'ドックに札がある');
  shell.pointerDown(chip[1].x + 5, chip[1].y);
  assert.deepEqual(shell.minimized, []);
  f = shell.frame(W, H, (t.now += 16));
  const s = f.windows.find((w) => w.app === 'settings')!.tile;
  assert.ok(s.y > H - 120 && f.animating, '札の位置から広がり始める');
  for (let i = 0; i < 200 && f.animating; i++) f = shell.frame(W, H, (t.now += 16));
  assert.ok(f.windows.find((w) => w.app === 'settings')!.tile.h > 200);
});

test('shell: 最後の窓は最小化できない', () => {
  const shell = make();
  shell.frame(W, H, 0);
  assert.equal(shell.minimizeApp('todo'), '');
  assert.equal(shell.minimizeApp('settings'), '');
  assert.match(shell.minimizeApp('notes'), /最後の窓/);
  assert.equal(shell.audit.at(-1)!.result, 'denied');
});

test('shell: タイトルバーのダブルクリックでズーム、Esc で戻る', () => {
  const shell = make();
  const t = { now: 0 };
  const todo = tileOf(shell, 'todo', t)!;
  shell.doubleClick(todo.x + 100, todo.y + 12);
  assert.equal(shell.zoomed, 'todo');
  let f = shell.frame(W, H, (t.now += 16));
  for (let i = 0; i < 200 && f.animating; i++) f = shell.frame(W, H, (t.now += 16));
  assert.ok(f.windows.find((w) => w.app === 'todo')!.tile.w > W - 20, '全体に広がる');
  shell.input({ t: 'key', key: 'Escape' });
  assert.equal(shell.zoomed, null);
});

test('shell: 境界のダブルクリックで均等にする (アニメーションする)', () => {
  const shell = make();
  const t = { now: 0 };
  shell.tiles = { ...(shell.tiles as Extract<TileNode, { kind: 'split' }>), ratio: 0.3 };
  shell.animations = false;
  let f = shell.frame(W, H, (t.now += 16));
  shell.animations = true;
  const todo = f.windows.find((w) => w.app === 'todo')!.tile;
  shell.doubleClick(todo.x + todo.w + 4, todo.y + 300);
  f = shell.frame(W, H, (t.now += 16));
  assert.ok(f.animating);
  for (let i = 0; i < 200 && f.animating; i++) f = shell.frame(W, H, (t.now += 16));
  const a = f.windows.find((w) => w.app === 'todo')!.tile.w;
  const b = f.windows.find((w) => w.app === 'settings')!.tile.w;
  assert.ok(Math.abs(a - b) < 1, `${a} と ${b}`);
});

test('shell: Alt+方向キーで隣と入れ替える', () => {
  const shell = make();
  shell.frame(W, H, 0);
  shell.pointerDown(...center(shell, 'todo'));
  shell.pointerUp();
  shell.input({ t: 'key', key: 'MoveRight' });
  assert.deepEqual(leaves(shell.tiles), ['settings', 'todo', 'notes']);
  shell.input({ t: 'key', key: 'MoveDown' });
  assert.deepEqual(leaves(shell.tiles), ['settings', 'notes', 'todo']);
});

function center(shell: Shell, app: string): [number, number] {
  const tile = shell.frame(W, H, 0).windows.find((w) => w.app === app)!.tile;
  return [tile.x + tile.w / 2, tile.y + tile.h - 20];
}

test('shell: ライブプレビューでは、離す前からほかの窓が場所を空ける', () => {
  const shell = make();
  shell.previewMode = 'live';
  shell.animations = false;
  let f = shell.frame(W, H, 0);
  const settingsBefore = f.windows.find((w) => w.app === 'settings')!.tile;
  const notes = f.windows.find((w) => w.app === 'notes')!.tile;
  const todo = f.windows.find((w) => w.app === 'todo')!.tile;
  shell.pointerDown(todo.x + 60, todo.y + 12);
  shell.pointerMove(todo.x + 70, todo.y + 20);
  shell.pointerMove(notes.x + notes.w - 50, notes.y + notes.h / 2);
  f = shell.frame(W, H, 16);
  const settingsDuring = f.windows.find((w) => w.app === 'settings')!.tile;
  assert.ok(settingsDuring.w > settingsBefore.w + 100, 'Settings が全幅に広がって見える');
  assert.equal(f.windows.find((w) => w.app === 'todo'), undefined, '運んでいる窓はプレビューが代わりに示す');
  assert.ok(shell.tiles.kind === 'split' && leaves(shell.tiles)[0] === 'todo', '木はまだ変わっていない');
  shell.input({ t: 'key', key: 'Escape' });
  f = shell.frame(W, H, 32);
  assert.deepEqual(f.windows.find((w) => w.app === 'settings')!.tile, settingsBefore, '取り消すと元の配置');
});

test('agent: レイアウトの提案は破線でプレビューされ、承認で実行される', () => {
  const shell = make();
  const t = { now: 0 };
  shell.frame(W, H, t.now);
  const before = JSON.stringify(shell.tiles);
  const r = shell.propose('Todo を Notes のタブに、Settings を最小化、均等に', [
    { kind: 'layout', op: { type: 'move', app: 'todo', drop: { kind: 'tab', target: 'notes' } } },
    { kind: 'layout', op: { type: 'minimize', app: 'settings' } },
    { kind: 'layout', op: { type: 'equalize' } },
  ]);
  assert.ok(r.ok, JSON.stringify(r));
  assert.equal(JSON.stringify(shell.tiles), before, '承認までは何も動かない');
  const f = shell.frame(W, H, (t.now += 16));
  assert.ok(f.ops.some((o) => o[0] === 'text' && o[1].str.includes('最小化される')));
  assert.ok(f.ops.some((o) => o[0] === 'rect' && o[1].dash && o[1].stroke === 'ghost'));
  shell.approve(r.proposal.id);
  assert.deepEqual(shell.minimized, ['settings']);
  assert.deepEqual(leaves(shell.tiles), ['todo']);
  assert.deepEqual(
    shell.audit.filter((a) => a.action === 'layout').map((a) => [a.by, a.target, a.result]),
    [
      ['agent', 'move todo: タブにまとめる', 'ok'],
      ['agent', 'minimize settings', 'ok'],
      ['agent', 'equalize', 'ok'],
    ],
  );
});

test('agent: 最小幅を下回るレイアウトの提案は、提案の時点で拒否する', () => {
  const shell = make();
  shell.frame(700, 800, 0);
  const r = shell.propose('3 列にする', [{ kind: 'layout', op: { type: 'move', app: 'notes', drop: { kind: 'split', target: 'settings', edge: 'right' } } }]);
  assert.equal(r.ok, false);
  if (!r.ok) assert.match(r.errors[0], /最小幅/);
});

test('スクロール: 窓に収まらない中身はホイールで動かせ、画面外は描かない。Tab で移動すると見える位置まで送る', () => {
  const measure = (s: string, size: number) => [...s].length * size * 0.6;
  const shell = new Shell([createTodoApp({ items: 200 }), createSettingsApp(), createNotesApp()], measure, memoryStorage());
  shell.animations = false;
  let f = shell.frame(W, H, 0);
  const has = (s: string) => f.ops.some((o) => o[0] === 'text' && o[1].str === s);
  assert.ok(has('タスク 5') && !has('タスク 150'), '最初は上だけ描く');
  assert.ok(f.ops.length < 400, `画面外を描かない (${f.ops.length} 命令)`);
  const todo = f.windows.find((w) => w.app === 'todo')!.tile;
  shell.wheel(todo.x + 50, todo.y + 100, 4000);
  f = shell.frame(W, H, 16);
  assert.ok(has('タスク 150') && !has('タスク 5'), 'ホイールで下へ動く');
  shell.wheel(todo.x + 50, todo.y + 100, -100000);
  f = shell.frame(W, H, 32);
  assert.ok(has('タスク 5'), '上端で止まる');
  // Tab で一番下の入力欄へ: 見える位置まで送る
  shell.pointerDown(todo.x + 30, todo.y + 80);
  shell.pointerUp();
  for (let i = 0; i < 200; i++) shell.input({ t: 'key', key: 'Tab' }); // i1 → i200 → 入力欄
  f = shell.frame(W, H, 48);
  assert.equal(shell.tree('todo').focus, 'input');
  f = shell.frame(W, H, 64);
  assert.ok(has('新しいタスク'), 'フォーカスした入力欄が見えている');
});
