// LayoutOp を実際の sway (headless) に送り、読み戻した木を、試作のモデル (tiling.ts) の結果と比べる。
// 使い方: (sway を headless で起動し、app_id が todo / settings / notes の窓を 3 つ開いてから)
//   SWAYSOCK=... node e2e/sway-bridge.ts
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import type { LayoutOp } from '../src/shell.ts';
import { readSway, realizeCommands, shape, sizeCommands, toSwayCommands } from '../src/sway.ts';
import type { SwayState } from '../src/sway.ts';
import { addToLargest, applyDrop, computeRects, equalize, leaves, removeApp } from '../src/tiling.ts';
import type { TileNode } from '../src/tiling.ts';

const sway = (...args: string[]) => execFileSync('swaymsg', args, { encoding: 'utf8' });
const read = (): SwayState => readSway(JSON.parse(sway('-t', 'get_tree')));
const sleep = (ms: number) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

interface Model {
  tiles: TileNode | null;
  minimized: string[];
  zoomed: string | null;
}

/** 試作のモデルでの結果 (shell.ts の simulate と同じ規則。最小幅の検査は除く) */
function simulate(m: Model, op: LayoutOp, area: SwayState['area']): Model | string {
  switch (op.type) {
    case 'move': {
      const next = m.tiles && applyDrop(m.tiles, op.app, op.drop);
      return next ? { ...m, tiles: next } : 'モデルでも無効';
    }
    case 'zoom':
      return { ...m, zoomed: op.app };
    case 'minimize':
      return { ...m, tiles: m.tiles && removeApp(m.tiles, op.app), minimized: [...m.minimized, op.app] };
    case 'restore':
      return { ...m, tiles: addToLargest(m.tiles, op.app, area, 0), minimized: m.minimized.filter((a) => a !== op.app) };
    case 'equalize':
      return { ...m, tiles: m.tiles && equalize(m.tiles) };
    case 'close':
      return { ...m, tiles: m.tiles && removeApp(m.tiles, op.app), minimized: m.minimized.filter((a) => a !== op.app) };
    case 'launch':
      return 'モデルでは扱わない (プラットフォームの操作)';
  }
}

/** 境界 (左の部分木の最後の窓が x、右の最初の窓が y) の道を探す */
function dividerBetween(t: TileNode, x: string, y: string, p: number[] = []): number[] | null {
  if (t.kind !== 'split') return null;
  if (leaves(t.a).at(-1) === x && leaves(t.b)[0] === y) return p;
  return dividerBetween(t.a, x, y, [...p, 0]) ?? dividerBetween(t.b, x, y, [...p, 1]);
}

type Step = { name: string; op: (m: Model) => LayoutOp };
const scenario: Step[] = [
  { name: '端で分割 (下): notes を todo の下へ', op: () => ({ type: 'move', app: 'notes', drop: { kind: 'split', target: 'todo', edge: 'bottom' } }) },
  { name: '端で分割 (右): settings を notes の右へ', op: () => ({ type: 'move', app: 'settings', drop: { kind: 'split', target: 'notes', edge: 'right' } }) },
  { name: '端で分割 (左): settings を todo の左へ', op: () => ({ type: 'move', app: 'settings', drop: { kind: 'split', target: 'todo', edge: 'left' } }) },
  { name: '端で分割 (上): todo を notes の上へ', op: () => ({ type: 'move', app: 'todo', drop: { kind: 'split', target: 'notes', edge: 'top' } }) },
  { name: '入れ替え: todo と settings', op: () => ({ type: 'move', app: 'todo', drop: { kind: 'swap', target: 'settings' } }) },
  {
    name: '差し込み: notes を左右の境界へ',
    op: (m) => {
      const ls = leaves(m.tiles!);
      const at = dividerBetween(m.tiles!, ls[0], ls[1]) ?? [];
      return { type: 'move', app: 'notes', drop: { kind: 'insert', path: at } };
    },
  },
  { name: 'タブ: todo を settings のタブへ', op: () => ({ type: 'move', app: 'todo', drop: { kind: 'tab', target: 'settings' } }) },
  { name: 'タブから引き出す: todo を notes の右へ', op: () => ({ type: 'move', app: 'todo', drop: { kind: 'split', target: 'notes', edge: 'right' } }) },
  { name: '全体の端: settings を全体の上へ', op: () => ({ type: 'move', app: 'settings', drop: { kind: 'split', target: null, edge: 'top' } }) },
  { name: 'ズーム: notes', op: () => ({ type: 'zoom', app: 'notes' }) },
  { name: 'ズーム解除', op: () => ({ type: 'zoom', app: null }) },
  { name: '最小化: todo', op: () => ({ type: 'minimize', app: 'todo' }) },
  { name: '元に戻す: todo', op: () => ({ type: 'restore', app: 'todo' }) },
  { name: '均等化', op: () => ({ type: 'equalize' }) },
];

const run = (cmds: string[], row: Record<string, unknown>) => {
  for (const c of cmds) {
    try {
      sway(c);
    } catch (e) {
      row.error = `${c}: ${String((e as { stdout?: string }).stdout ?? e).slice(0, 200)}`;
    }
  }
  sleep(120);
};

/** モデルの大きさを sway に送る (2 回: 入れ子の容器は 1 回では揃わないことがある) */
const syncSizes = (m: Model, row: Record<string, unknown>) => {
  if (!m.tiles || m.zoomed) return;
  for (let i = 0; i < 2; i++) {
    const st = read();
    run(sizeCommands(computeRects(m.tiles, st.area, 0).tiles, st), row);
  }
};

function compare(want: Model | string, got: SwayState): string[] {
  const diffs: string[] = [];
  if (typeof want === 'string') return [want];
  const ws = JSON.stringify(shape(want.tiles));
  const gs = JSON.stringify(shape(got.tiles));
  if (ws !== gs) diffs.push(`構造: モデル ${ws} / sway ${gs}`);
  if (want.zoomed !== got.zoomed) diffs.push(`ズーム: モデル ${want.zoomed} / sway ${got.zoomed}`);
  if (JSON.stringify([...want.minimized].sort()) !== JSON.stringify([...got.minimized].sort())) diffs.push(`最小化: モデル ${want.minimized} / sway ${got.minimized}`);
  if (ws === gs && want.tiles && !want.zoomed) {
    const { tiles } = computeRects(want.tiles, got.area, 0);
    for (const [app, r] of tiles) {
      const g = got.rects.get(app);
      const tabbed = ws.includes(`"shown":"${app}"`);
      if (!g || tabbed) continue;
      const d = Math.max(Math.abs(g.x - r.x), Math.abs(g.y - r.y), Math.abs(g.w - r.w), Math.abs(g.h - r.h));
      if (d > 2) diffs.push(`矩形 ${app}: モデル ${[r.x, r.y, r.w, r.h].map(Math.round)} / sway ${[g.x, g.y, g.w, g.h]}`);
    }
  }
  return diffs;
}

const third = (a: string, b: string, c: string): TileNode => ({ kind: 'split', dir: 'row', ratio: 1 / 3, a: { kind: 'leaf', app: a }, b: { kind: 'split', dir: 'row', ratio: 0.5, a: { kind: 'leaf', app: b }, b: { kind: 'leaf', app: c } } });

function runMode(mode: 'incremental' | 'declarative') {
  // どちらの方式も同じ初期状態から始める (宣言的方式で作る)
  const init: Model = { tiles: third('settings', 'todo', 'notes'), minimized: [], zoomed: null };
  const r0: Record<string, unknown> = {};
  run(realizeCommands(init, read()), r0);
  syncSizes(init, r0);
  let st = read();
  let model: Model = { tiles: st.tiles, minimized: st.minimized, zoomed: st.zoomed };
  const results: Record<string, unknown>[] = [];
  console.log(`\n## ${mode === 'incremental' ? '逐次方式 (操作を 1 つずつ写す + 大きさを送る)' : '宣言的方式 (モデルの状態を作り直す)'}\n初期: ${JSON.stringify(shape(st.tiles))}`);
  for (const step of scenario) {
    const op = step.op(model);
    const want = simulate(model, op, st.area);
    const row: Record<string, unknown> = { mode, step: step.name, op };
    if (mode === 'incremental') {
      const mapped = toSwayCommands(op, st);
      if ('unsupported' in mapped) {
        row.result = 'unsupported';
        row.reason = mapped.unsupported;
      } else {
        row.commands = mapped.commands;
        run(mapped.commands, row);
        if (typeof want !== 'string') syncSizes(want, row);
      }
    } else if (typeof want !== 'string') {
      const cmds = realizeCommands({ ...want, zoomed: null }, st);
      row.commands = cmds.length;
      run(cmds, row);
      syncSizes({ ...want, zoomed: null }, row);
      if (want.zoomed) run([`[con_id=${read().ids.get(want.zoomed)}] fullscreen enable`], row);
    }
    if (!row.result) {
      const got = read();
      const diffs = compare(want, got);
      row.result = diffs.length ? 'mismatch' : 'ok';
      if (diffs.length) row.diffs = diffs;
    }
    results.push(row);
    console.log(`${String(row.result).padEnd(11)} ${step.name}${row.reason ? `\n            ${row.reason}` : ''}${row.diffs ? `\n            ${(row.diffs as string[]).join('\n            ')}` : ''}${row.error ? `\n            error: ${row.error}` : ''}`);
    // 次の手順は sway の実際の状態から始める (食い違いを持ち越さない)
    st = read();
    model = { tiles: st.tiles, minimized: st.minimized, zoomed: st.zoomed };
  }
  const n = (r: string) => results.filter((x) => x.result === r).length;
  console.log(`ok ${n('ok')} / mismatch ${n('mismatch')} / unsupported ${n('unsupported')}`);
  return results;
}

const all = [...runMode('incremental'), ...runMode('declarative')];
const out = path.join(import.meta.dirname, '..', 'sway-bridge.json');
fs.writeFileSync(out, JSON.stringify(all, null, 2));
console.log(`\n${out}`);
