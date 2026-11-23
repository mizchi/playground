// ネイティブのホストを本物の窓で確かめる: Xvfb (X11) の上に SDL3 の窓を開き、wgpu-native の表面に描く。
// 入力は xdotool で X11 のマウス・キーボードとして入れる (SDL3 のイベント → 描画プロセスの JSON → ホストの shell)。
// 画面は ImageMagick の import で X のルート窓から取り、表示された画素を確かめる。
// 使い方: native/build.sh のあとで  node e2e/native-window.ts [--shots]   (Xvfb・xdotool・ImageMagick が要る)
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createInterface } from 'node:readline';
import { THEMES } from '../src/theme.ts';

const root = path.join(import.meta.dirname, '..');
const shots = process.argv.includes('--shots');
const tmp = mkdtempSync(path.join(tmpdir(), 'native-win-'));
const display = `:${90 + Math.floor(Math.random() * 9)}`;
const env = { ...process.env, DISPLAY: display, VK_ICD_FILENAMES: process.env.VK_ICD_FILENAMES ?? '/opt/lavapipe/lvp_icd.json', SDL_VIDEO_DRIVER: 'x11' };
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const x = (...args: string[]) => execFileSync('xdotool', args, { env, encoding: 'utf8' }).trim();

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

const xvfb = spawn('Xvfb', [display, '-screen', '0', '1600x900x24', '-nolisten', 'tcp'], { stdio: 'ignore' });
await sleep(800);

const host = spawn(process.execPath, [path.join(root, 'native', 'host.ts'), '--control', '--fresh', '--kv', path.join(tmp, 'kv.json')], { env, stdio: ['pipe', 'pipe', 'pipe'] }) as ChildProcessWithoutNullStreams;
const stderr: string[] = [];
host.stderr.on('data', (d) => stderr.push(String(d)));
const replies = new Map<number, (v: Record<string, unknown>) => void>();
let readyResolve: (v: Record<string, unknown>) => void;
const ready = new Promise<Record<string, unknown>>((r) => (readyResolve = r));
createInterface({ input: host.stdout }).on('line', (l) => {
  const v = JSON.parse(l) as Record<string, unknown>;
  if (v.t === 'ready') readyResolve(v);
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
const state = () => cmd({ t: 'state' }) as Promise<{ workspace: number; size: { w: number; h: number }; texts: string[]; ime: string }>;
const rects = async () => ((await cmd({ t: 'rects' })) as { rects: Record<string, { x: number; y: number; w: number; h: number }> }).rects;
const tree = async (app: string) => ((await cmd({ t: 'tree', app })) as { tree: { nodes: { id: string; checked?: boolean; value?: string }[] }[] }).tree[0].nodes;
/** 入力が描画プロセス → ホストへ届き、描き終わるまで待つ */
const until = async (f: () => Promise<boolean>, what: string) => {
  for (let i = 0; i < 50; i++) {
    if (await f()) return;
    await sleep(100);
  }
  throw new Error(`${what} にならない`);
};
const screen = (file: string) => execFileSync('import', ['-window', 'root', file], { env });
const pixel = (file: string, px: number, py: number) => {
  const s = execFileSync('convert', [file, '-format', '%[fx:int(255*p{' + px + ',' + py + '}.r)],%[fx:int(255*p{' + px + ',' + py + '}.g)],%[fx:int(255*p{' + px + ',' + py + '}.b)]', 'info:'], { encoding: 'utf8' });
  return s.split(',').map(Number);
};
const near = (a: number[], hex: string) => {
  const n = parseInt(hex.slice(1), 16);
  const b = [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  return a.every((v, i) => Math.abs(v - b[i]) <= 6);
};

let win = '';
let origin = { x: 0, y: 0 };
const at = (r: { x: number; y: number; w: number; h: number }) => [String(Math.round(origin.x + r.x + r.w / 2)), String(Math.round(origin.y + r.y + r.h / 2))];

await step('X11 の窓を開き、wgpu-native の表面に描く (Vulkan / lavapipe)', async () => {
  const r = await Promise.race([ready, sleep(30000).then(() => null)]);
  assert.ok(r, `ready が来ない\n${stderr.join('')}`);
  win = x('search', '--sync', '--name', 'AI-native desktop').split('\n')[0];
  const g = Object.fromEntries(x('getwindowgeometry', '--shell', win).split('\n').map((l) => l.split('=')));
  origin = { x: Number(g.X), y: Number(g.Y) };
  await state(); // 最初のフレームまで待つ
  await sleep(300);
  const file = path.join(shots ? path.join(root, 'screenshots') : tmp, 'native-window.png');
  screen(file);
  // 表示された画素: 追加ボタンの塗り (accent) と、パネルの地 (surface)
  const add = (await rects())['todo/add'];
  assert.ok(near(pixel(file, Math.round(origin.x + add.x + 4), Math.round(origin.y + add.y + add.h / 2)), THEMES.light.accent), `追加ボタンの色 ${pixel(file, Math.round(origin.x + add.x + 4), Math.round(origin.y + add.y + add.h / 2))}`);
  const s = await state();
  assert.ok(near(pixel(file, origin.x + s.size.w - 6, origin.y + s.size.h - 6), THEMES.light.surface), 'パネルの地');
});

await step('マウス: X11 のクリックが SDL3 → ホスト → shell の意味的な操作になる', async () => {
  const r = await rects();
  x('mousemove', ...at(r['todo/i1']), 'click', '1');
  await until(async () => (await tree('todo')).find((n) => n.id === 'i1')!.checked === true, 'i1 が完了');
});

await step('キーボード: 入力欄を押すと SDL3 の文字入力が始まり、打った文字が入る', async () => {
  const r = await rects();
  x('mousemove', ...at(r['todo/input']), 'click', '1');
  await until(async () => (await state()).ime === 'ime-enable', 'IME が有効');
  x('type', '--delay', '30', 'buy eggs');
  await until(async () => (await tree('todo')).find((n) => n.id === 'input')!.value === 'buy eggs', '入力欄が buy eggs');
  x('mousemove', ...at(r['todo/add']), 'click', '1');
  await until(async () => (await state()).texts.includes('buy eggs'), '項目が増える');
});

await step('キー: Alt+2 / Alt+1 でワークスペースを切り替える (code で見るので配列に依らない)', async () => {
  x('key', 'alt+2');
  await until(async () => (await state()).workspace === 2, 'agent のワークスペース');
  x('key', 'alt+1');
  await until(async () => (await state()).workspace === 1, '人のワークスペース');
});

await step('1 フレームの時間 (ホスト: shell.frame + 符号化 / 描画プロセス: 命令の解読 + GPU への提出と表示)', async () => {
  const host: number[] = [];
  const gpu: number[] = [];
  for (let i = 0; i < 20; i++) {
    x('mousemove', String(origin.x + 100 + i * 10), String(origin.y + 300)); // ホバーで描き直させる
    await sleep(60);
    const s = (await cmd({ t: 'state' })) as { hostMs: number; gpuMs: number; instances: number };
    host.push(s.hostMs);
    gpu.push(s.gpuMs);
  }
  const avg = (a: number[]) => (a.reduce((p, c) => p + c, 0) / a.length).toFixed(2);
  const s = (await cmd({ t: 'state' })) as { instances: number };
  console.log(`     ホスト: 平均 ${avg(host)}ms / 描画プロセス: 平均 ${avg(gpu)}ms (インスタンス ${s.instances} 個、lavapipe)`);
  assert.ok(Number(avg(host)) < 16);
});

await step('窓の大きさを変えると、ホストが新しい大きさで描き直す', async () => {
  x('windowsize', win, '1100', '700');
  await until(async () => {
    const s = await state();
    return s.size.w === 1100 && s.size.h === 700;
  }, '1100x700');
});

// 窓の × (WM_DELETE_WINDOW) は窓のマネージャが送るので、窓のマネージャの無い Xvfb では試せない (xdotool windowclose は窓を壊すだけ)
await step('ホストを止める (Ctrl+C) と、描画プロセスに終わりを送り、両方が 0 で終わる', async () => {
  const exited = new Promise((r) => host.on('exit', r));
  host.kill('SIGINT');
  const code = await Promise.race([exited, sleep(5000).then(() => 'timeout')]);
  assert.equal(code, 0, stderr.join(''));
});

host.kill();
xvfb.kill();
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
