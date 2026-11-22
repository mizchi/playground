import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { builtinManifests } from '../src/apps/manifests.ts';
import { terminalManifest } from '../src/apps/terminal/terminal.ts';
import { createVt, instantiateGhostty } from '../src/apps/terminal/vt.ts';
import type { VtFactory } from '../src/apps/terminal/vt.ts';
import { memoryStorage } from '../src/cache.ts';
import { createPlatform } from '../src/platform/platform.ts';
import { memoryKeyValue } from '../src/platform/services.ts';
import type { PtyConnector } from '../src/platform/types.ts';

const wasm = () => readFile(new URL('../node_modules/ghostty-web/ghostty-vt.wasm', import.meta.url));
const ghostty = instantiateGhostty(await wasm());
const measure = (s: string, size: number) => [...s].length * size * 0.6;
const tick = () => new Promise((r) => setTimeout(r, 0));

const make = async (pty?: PtyConnector) => {
  const p = createPlatform({ apps: [...builtinManifests, terminalManifest(async (): Promise<VtFactory> => {
    const g = await ghostty;
    return (c, r) => createVt(g, c, r);
  })], initial: ['terminal', 'notes'], measure, templates: memoryStorage(), kv: memoryKeyValue() });
  p.services.pty = pty ?? null;
  p.shell.animations = false;
  await tick();
  p.shell.frame(1400, 800);
  return p;
};
type P = Awaited<ReturnType<typeof make>>;
const screen = (p: P, id = 'terminal') => p.shell.tree(id).get('screen')!;
const run = (p: P, line: string, id = 'terminal') => {
  p.shell.dispatch({ app: id, target: 'input', action: 'setValue', data: line, by: 'human' });
  p.shell.dispatch({ app: id, target: 'input', action: 'click', by: 'human' });
};

test('libghostty-vt: 色・太字・反転・全角を区間にまとめる', async () => {
  const vt = createVt(await ghostty, 20, 3);
  vt.write('ab \x1b[31mred\x1b[0m \x1b[1mB\x1b[0m\r\n日本\x1b[7mX\x1b[0m');
  const { screen: s, text } = vt.snapshot();
  assert.equal(text, 'ab red B\n日本X');
  assert.deepEqual(s.lines[0].map((r) => [r.col, r.text, !!r.fg, !!r.bold]), [
    [0, 'ab', false, false],
    [3, 'red', true, false],
    [7, 'B', false, true],
  ]);
  assert.deepEqual(s.lines[1].map((r) => [r.col, r.text, !!r.wide, r.bg === s.fg]), [
    [0, '日', true, false],
    [2, '本', true, false],
    [4, 'X', false, true], // 反転: 背景が既定の文字色
  ]);
  assert.deepEqual(s.cursor, { x: 5, y: 1 });
  vt.free();
});

test('libghostty-vt: 別の端末を free したあとに作った端末にも、ごみの文字が出ない', async () => {
  const g = await ghostty;
  for (let k = 0; k < 3; k++) {
    const a = createVt(g, 80, 30);
    for (let i = 0; i < 500; i++) a.write(`${'x'.repeat(i % 70)}\r\n`);
    a.free();
  }
  const b = createVt(g, 72, 18);
  b.write('hello\r\nworld');
  assert.equal(b.snapshot().text, 'hello\nworld');
  b.free();
});

test('Terminal は意味ツリーの log として画面を出し、Enter (click) でコマンドを実行する', async () => {
  const p = await make();
  assert.match(screen(p).value!, /Terminal — libghostty-vt/);
  run(p, 'echo こんにちは');
  assert.match(screen(p).value!, /\$ echo こんにちは\nこんにちは\n.*\$$/);
  run(p, 'help');
  assert.match(screen(p).value!, /colors/);
  run(p, 'cal');
  assert.match(screen(p).value!, /日 月 火 水 木 金 土\n/);
  run(p, 'nope');
  assert.match(screen(p).value!, /nope: コマンドが見つかりません/);
  run(p, 'clear');
  assert.equal(screen(p).value, 'you@ai-desktop:~$');
});

test('入力中の文字は、プロンプトの行にそのまま出る', async () => {
  const p = await make();
  p.shell.dispatch({ app: 'terminal', target: 'input', action: 'replaceSelectedText', data: 'ec', by: 'human' });
  p.shell.dispatch({ app: 'terminal', target: 'input', action: 'replaceSelectedText', data: 'ho', by: 'human' });
  assert.match(screen(p).value!.split('\n').at(-1)!, /\$ echo$/);
});

test('描画: 格子に 1 桁ずつ置き、色はアプリの中身だけに rgb で付く。窓の最小幅は押し上げない', async () => {
  const p = await make();
  run(p, 'colors');
  const f = p.shell.frame(1400, 800);
  const mono = f.ops.filter((o) => o[0] === 'text' && o[1].mono);
  assert.ok(mono.length > 50);
  assert.ok(mono.every((o) => o[0] === 'text' && [...o[1].str].length === 1));
  const bgs = f.ops.filter((o) => o[0] === 'rect' && o[1].rgb);
  assert.ok(bgs.length > 40, '色の見本の背景');
  const term = f.windows.find((w) => w.app === 'terminal')!;
  assert.ok(term.minWidth < 400, `最小幅 ${term.minWidth}`);
});

test('agent には描画用の格子を渡さず、文字だけを見せる。コマンド行への入力は承認が要る', async () => {
  const p = await make();
  const t = p.shell.getTree('terminal')[0].nodes.find((n) => n.id === 'screen')!;
  assert.equal(t.screen, undefined);
  assert.match(t.value!, /help/);
  p.shell.policy.autoApproveSafe = true;
  const r = p.shell.propose('whoami を実行する', [
    { app: 'terminal', target: 'input', action: 'setValue', data: 'whoami' },
    { app: 'terminal', target: 'input', action: 'click' },
  ]);
  assert.ok(r.ok);
  assert.equal(r.proposal.sensitive, true);
  assert.equal(r.proposal.status, 'pending');
  p.shell.approve(r.proposal.id);
  assert.match(screen(p).value!, /agent \(人が承認した提案で入力された\)/);
});

test('ファイルは保存領域、copy/paste はクリップボード、open はアプリの起動 (能力の確認を通る)', async () => {
  const p = await make();
  run(p, 'write memo.txt 牛乳を買う');
  run(p, 'ls');
  assert.match(screen(p).value!, /memo\.txt {2}readme\.txt/);
  run(p, 'cat memo.txt');
  assert.match(screen(p).value!, /\n牛乳を買う\n/);
  run(p, 'copy hello');
  run(p, 'paste');
  assert.match(screen(p).value!, /許可を待っている/);
  const pr = p.services.prompts.find((x) => x.capability === 'clipboard.read')!;
  p.services.answer(pr.id, true, 'human');
  run(p, 'paste');
  assert.match(screen(p).value!, /\$ paste\nhello\n/);
  run(p, 'open settings');
  assert.ok(!p.shell.windows().some((w) => w.id === 'settings'));
  p.services.answer(p.services.prompts.find((x) => x.capability === 'apps.launch')!.id, true, 'human');
  run(p, 'open settings');
  assert.ok(p.shell.windows().some((w) => w.id === 'settings'));
});

test('ホストの擬似端末: terminal.pty を人が許可するとつながり、出力は libghostty-vt を通る', async () => {
  const written: string[] = [];
  let out: (d: string) => void = () => {};
  const fake: PtyConnector = (cols, rows, onData) => {
    out = onData;
    written.push(`open ${cols}x${rows}`);
    return { write: (d) => written.push(d), close: () => written.push('close') };
  };
  const p = await make(fake);
  p.shell.dispatch({ app: 'terminal', target: 'host', action: 'click', by: 'human' });
  assert.equal(written.length, 0);
  p.services.answer(p.services.prompts.find((x) => x.capability === 'terminal.pty')!.id, true, 'human');
  assert.deepEqual(written, ['open 72x18']);
  out('\x1b[32mbash$\x1b[0m ');
  assert.equal(screen(p).value, 'bash$');
  run(p, 'ls -la');
  assert.equal(written.at(-1), 'ls -la\r');
  p.shell.dispatch({ app: 'terminal', target: 'ctrlc', action: 'click', by: 'human' });
  assert.equal(written.at(-1), '\x03');
  p.close('terminal');
  assert.equal(written.at(-1), 'close');
});

test('ターミナルは複数の窓で開け、それぞれ別の端末の状態を持つ', async () => {
  const p = await make();
  p.launch('terminal');
  await tick();
  run(p, 'echo one');
  run(p, 'echo two', 'terminal-2');
  assert.match(screen(p).value!, /one/);
  assert.doesNotMatch(screen(p).value!, /two/);
  assert.match(screen(p, 'terminal-2').value!, /two/);
});
