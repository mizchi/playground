// sway への橋渡しの純粋な部分 (実物の sway は e2e/sway-bridge.ts で動かす)
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';
import { readSway, realizeCommands, shape, sizeCommands, toSwayCommands } from '../src/sway.ts';
import type { SwayNode } from '../src/sway.ts';
import { leaf } from '../src/tiling.ts';

// 実際の sway 1.9 (headless) から取った木: column[row[settings, todo], notes]
const fixture = JSON.parse(fs.readFileSync(path.join(import.meta.dirname, 'fixtures', 'sway-tree.json'), 'utf8')) as SwayNode;

test('sway の木を読む: 構造・矩形・con_id', () => {
  const st = readSway(fixture);
  assert.deepEqual(shape(st.tiles), { dir: 'column', children: [{ dir: 'row', children: ['settings', 'todo'] }, 'notes'] });
  assert.deepEqual(st.rects.get('notes'), { x: 0, y: 400, w: 1400, h: 400 });
  assert.equal(st.ids.get('settings'), 5);
  assert.deepEqual(st.minimized, []);
});

const con = (id: number, extra: Partial<SwayNode>): SwayNode => ({ id, type: 'con', rect: { x: 0, y: 0, width: 100, height: 100 }, nodes: [], ...extra });
const ws = (nodes: SwayNode[], layout = 'splith'): SwayNode => ({
  id: 1,
  type: 'root',
  rect: { x: 0, y: 0, width: 1000, height: 600 },
  nodes: [{ id: 2, type: 'output', rect: { x: 0, y: 0, width: 1000, height: 600 }, nodes: [{ id: 3, type: 'workspace', name: '1', layout, rect: { x: 0, y: 0, width: 1000, height: 600 }, nodes }] }],
});

test('タブの容器はタブの葉に、全画面の子の percent (2.0) は残りで読む', () => {
  const tree = ws([
    con(10, { layout: 'tabbed', percent: 0.5, focus: [12, 11], nodes: [con(11, { app_id: 'a' }), con(12, { app_id: 'b' })] }),
    con(13, { app_id: 'c', percent: 2.0, fullscreen_mode: 1 }),
  ]);
  const st = readSway(tree);
  assert.deepEqual(shape(st.tiles), { dir: 'row', children: [{ tabs: ['a', 'b'], shown: 'b' }, 'c'] });
  assert.equal(st.zoomed, 'c');
  assert.equal(st.tiles!.kind === 'split' && st.tiles!.ratio, 0.5, '全画面でも比率は 0.5');
});

test('操作の写し方: 入れ替え・左への分割・全体の端 (未対応)・均等化 (大きさだけ)', () => {
  const st = readSway(fixture);
  assert.deepEqual(toSwayCommands({ type: 'move', app: 'todo', drop: { kind: 'swap', target: 'notes' } }, st), { commands: ['[con_id=6] swap container with con_id 7'] });
  const left = toSwayCommands({ type: 'move', app: 'notes', drop: { kind: 'split', target: 'settings', edge: 'left' } }, st);
  assert.ok('commands' in left && left.commands.some((c) => c.includes('swap container')), '左・上は隣に置いてから入れ替える');
  assert.ok('unsupported' in toSwayCommands({ type: 'move', app: 'notes', drop: { kind: 'split', target: null, edge: 'top' } }, st));
  assert.deepEqual(toSwayCommands({ type: 'equalize' }, st), { commands: [] });
  assert.deepEqual(sizeCommands(new Map([['notes', { x: 0, y: 0, w: 700.4, h: 400 }]]), st), ['[con_id=7] resize set width 700 px height 400 px']);
});

test('宣言的方式: 全てを scratchpad に入れてから、木の順に組み直す', () => {
  const st = readSway(fixture);
  const cmds = realizeCommands({ tiles: { kind: 'split', dir: 'column', ratio: 0.5, a: leaf('todo'), b: { kind: 'leaf', app: 'notes', tabs: ['notes', 'settings'] } }, minimized: [], zoomed: null }, st);
  assert.deepEqual(cmds.slice(0, 3).sort(), ['[con_id=5] move scratchpad', '[con_id=6] move scratchpad', '[con_id=7] move scratchpad']);
  assert.deepEqual(cmds.slice(3), [
    '[con_id=6] scratchpad show',
    '[con_id=6] floating disable',
    '[con_id=6] splitv',
    '[con_id=6] focus',
    '[con_id=7] scratchpad show',
    '[con_id=7] floating disable',
    '[con_id=7] splith',
    '[con_id=7] layout tabbed',
    '[con_id=7] focus',
    '[con_id=5] scratchpad show',
    '[con_id=5] floating disable',
    '[con_id=7] focus',
  ]);
});
