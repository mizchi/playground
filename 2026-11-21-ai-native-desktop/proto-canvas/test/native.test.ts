// ネイティブのホストの部品: キーの対応 (ブラウザと共通)、描画プロセスへの命令の形、ファイルの KeyValue、パネル
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { mapKey } from '../src/backend/keys.ts';
import { THEMES } from '../src/theme.ts';
import { fileKeyValue } from '../native/kv.ts';
import { PANEL_W, drawPanel, wrap } from '../native/panel.ts';
import { MSG, encodeFrame, message, rgba } from '../native/wire.ts';

const measure = (s: string, size: number) => [...s].length * size * 0.6;
const k = (code: string, key = '', alt = false, shift = false) => ({ code, key, alt, shift });

test('キーの対応: レイアウトは Alt + code、Tab / Enter / Escape はそのまま、Space と Backspace は IME の状態で変わる', () => {
  assert.deepEqual(mapKey(k('Digit2', '™', true), false), { t: 'key', key: 'Workspace2' }, 'mac の Option で key が変わっても code で見る');
  assert.deepEqual(mapKey(k('ArrowLeft', 'ArrowLeft', true), false), { t: 'key', key: 'MoveLeft' });
  assert.deepEqual(mapKey(k('Tab', 'Tab', false, true), false), { t: 'key', key: 'ShiftTab' });
  assert.deepEqual(mapKey(k('Space', ' '), false), { t: 'key', key: 'Space' });
  assert.equal(mapKey(k('Space', ' '), true), null, '入力中の空白は文字入力として届く');
  assert.equal(mapKey(k('Backspace', 'Backspace'), false), null);
  assert.deepEqual(mapKey(k('Backspace', 'Backspace'), true), { t: 'key', key: 'Backspace' });
  assert.equal(mapKey(k('KeyA', 'a'), false), null, '文字は文字入力として届く');
});

test('描画プロセスへの命令: [u32 長さ][種類][中身]。色 token は RGBA (0xAABBGGRR) にする', () => {
  assert.equal(rgba('#3557d4'), 0xffd45735);
  const body = encodeFrame(
    [
      ['rect', { x: 1, y: 2, w: 3, h: 4, radius: 5, fill: 'accent' }],
      ['clip', { x: 0, y: 0, w: 10, h: 10 }],
      ['text', { x: 1, y: 2, str: 'あa', size: 14, color: 'text' }],
      ['pop'],
    ],
    THEMES.light,
  );
  const v = new DataView(body.buffer, body.byteOffset, body.byteLength);
  assert.equal(v.getUint32(0, true), rgba(THEMES.light.bg), '背景色');
  assert.equal(body[4], 1, 'rect');
  assert.equal(v.getUint32(4 + 1 + 20, true), rgba(THEMES.light.accent));
  // rect (1+20+4+4+4+1) + clip (1+16) + text (1+12+4+1+4+4 バイトの文字) + pop (1)
  assert.equal(body.length, 4 + 34 + 17 + 26 + 1);
  const m = message(MSG.frame, body);
  assert.equal(m.readUInt32LE(0), body.length + 1);
  assert.equal(m[4], MSG.frame);
});

test('ファイルの KeyValue: 書くたびにファイルに残り、開き直すと読める。壊れたファイルは空から', () => {
  const dir = mkdtempSync(join(tmpdir(), 'kv-'));
  const path = join(dir, 'sub', 'kv.json');
  const a = fileKeyValue(path);
  a.set('app.todo.items', '[1]');
  assert.deepEqual(JSON.parse(readFileSync(path, 'utf8')), { 'app.todo.items': '[1]' });
  const b = fileKeyValue(path);
  assert.equal(b.get('app.todo.items'), '[1]');
  assert.deepEqual(b.keys(), ['app.todo.items']);
  assert.equal(b.get('toString'), undefined, 'Object の組み込みのキーは無い');
  writeFileSync(path, '{壊れている');
  assert.deepEqual(fileKeyValue(path).keys(), []);
});

test('ファイルの KeyValue: 生きている別のプロセスが使っているファイルは開かない。書けなかった値は、もう一度書ける', () => {
  const dir = mkdtempSync(join(tmpdir(), 'kv-'));
  const path = join(dir, 'kv.json');
  writeFileSync(`${path}.lock`, String(process.ppid)); // 親のプロセス (生きている)
  assert.throws(() => fileKeyValue(path), /別のホスト/);
  writeFileSync(`${path}.lock`, '999999999'); // 落ちたプロセスの残り
  const kv = fileKeyValue(path);
  kv.set('a', '1');
  // rename の先をディレクトリにして、書き込みを失敗させる
  rmSync(path);
  mkdirSync(path);
  assert.throws(() => kv.set('a', '2'));
  assert.equal(kv.get('a'), '1', '書けなかった値は手元にも入れない');
  rmSync(path, { recursive: true });
  kv.set('a', '2');
  assert.equal(JSON.parse(readFileSync(path, 'utf8')).a, '2', '同じ値でもう一度書ける');
});

test('ホストのパネル: 待っているものごとに答えるボタンを出す。長い文は折り返して切る', () => {
  const f = drawPanel(
    {
      workspace: 1,
      merges: [{ id: 'm1', by: 'agent', intent: 'メモを足す', changes: [{ app: 'notes', key: 'notes' }] }],
      intents: [{ id: 'i9', text: 'Terminal が Calendar に頼んでいる: 2026-11-24 に予定「会議」を追加する' }],
      prompts: [{ id: 'g1', text: 'Todo が 通知 の許可を求めている' }],
      proposals: [{ id: 'p1', intent: 'あ'.repeat(200), steps: ['todo / i1 を click'], sensitive: true }],
      status: [],
    },
    1000,
    800,
    measure,
    'merge:m1:1',
  );
  assert.deepEqual(f.buttons.map((b) => b.action), ['ws:1', 'ws:2', 'merge:m1:1', 'merge:m1:0', 'intent:i9:1', 'intent:i9:0', 'grant:g1:1', 'grant:g1:0', 'prop:p1:1', 'prop:p1:0']);
  for (const b of f.buttons) assert.ok(b.rect.x >= 1000 && b.rect.x + b.rect.w <= 1000 + PANEL_W, `${b.label} はパネルの中`);
  // フォーカスの輪は 3 番目のボタン (merge:m1:1) の周り
  const ring = f.ops.find((o) => o[0] === 'rect' && (o[1] as { lineWidth?: number }).lineWidth === 2 && (o[1] as { stroke?: string }).stroke === 'accent');
  assert.ok(ring && Math.abs((ring[1] as { x: number }).x - (f.buttons[2].rect.x - 3)) < 0.01);
  // 手順はすべて出す (承認の前に全部見られる)
  const many = drawPanel({ workspace: 1, merges: [], intents: [], prompts: [], proposals: [{ id: 'p2', intent: '多い', steps: ['a', 'b', 'c', 'd', 'e', 'f', 'g'].map((x) => `todo / ${x} を click`), sensitive: false }], status: [] }, 0, 800, measure, null);
  const texts = many.ops.filter((o) => o[0] === 'text').map((o) => (o[1] as { str: string }).str);
  assert.ok(texts.includes('7. todo / g を click'));
  // 縦に収まらないボタンは visible でない。スクロールすると見える
  const prompts = Array.from({ length: 6 }, (_, i) => ({ id: `g${i}`, text: `要求 ${i}` }));
  const small = drawPanel({ workspace: 1, merges: [], intents: [], prompts, proposals: [], status: [] }, 0, 300, measure, null);
  const last = small.buttons.at(-1)!;
  assert.equal(last.visible, false);
  assert.ok(small.contentH > 300);
  const scrolled = drawPanel({ workspace: 1, merges: [], intents: [], prompts, proposals: [], status: [] }, 0, 300, measure, null, small.contentH - 300);
  assert.equal(scrolled.buttons.at(-1)!.visible, true);
  const lines = wrap('あ'.repeat(200), 100, 10, measure, 2);
  assert.equal(lines.length, 2);
  assert.ok(lines[1].endsWith('…') && measure(lines[1], 10) <= 100);
});
