// ネイティブのホストの通し確認: Node のホスト (native/host.ts) + Odin の描画プロセス (wgpu-native)。ブラウザは使わない。
// 窓なし (オフスクリーン) で起動し、--control の標準入力から、描画プロセスが送るのと同じ形の入力を入れる。
// GPU はこの環境では lavapipe (ソフトウェアの Vulkan)。MCP のクライアントから AI としてつなぎ、人の承認はホストのパネルで行う。
// 使い方: native/build.sh で描画プロセスを作ってから  node e2e/native.ts [--shots]
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createInterface } from 'node:readline';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

const root = path.join(import.meta.dirname, '..');
const shots = process.argv.includes('--shots');
const shotsDir = path.join(root, 'screenshots');
const tmp = mkdtempSync(path.join(tmpdir(), 'native-e2e-'));
const kvFile = path.join(tmp, 'kv.json');
const env = { ...process.env, VK_ICD_FILENAMES: process.env.VK_ICD_FILENAMES ?? '/opt/lavapipe/lvp_icd.json' };
if (!existsSync(path.join(root, 'native', 'bin', 'renderer'))) {
  console.log('native/bin/renderer が無い。native/build.sh でビルドする');
  process.exit(1);
}

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

type Rect = { x: number; y: number; w: number; h: number };
interface State {
  workspace: number;
  windows: { id: string; title: string }[];
  focused: string | null;
  ime: string;
  panelFocus: number | null;
  buttons: { action: string; label: string; rect: Rect }[];
  texts: string[];
  ops: number;
  instances: number;
  gpuMs: number;
  proposals: { id: string; status: string }[];
  prompts: { id: string; status: string }[];
}

/** ホストを起こし、--control で話す */
async function startHost(extra: string[] = []) {
  const p = spawn(process.execPath, [path.join(root, 'native', 'host.ts'), '--headless', '1500x860', '--control', '--kv', kvFile, ...extra], { env, stdio: ['pipe', 'pipe', 'pipe'] }) as ChildProcessWithoutNullStreams;
  const stderr: string[] = [];
  p.stderr.on('data', (d) => stderr.push(String(d)));
  const pending = new Map<number, (v: Record<string, unknown>) => void>();
  const events: Record<string, unknown>[] = [];
  let onEvent: ((v: Record<string, unknown>) => void) | null = null;
  createInterface({ input: p.stdout }).on('line', (l) => {
    const v = JSON.parse(l) as Record<string, unknown>;
    if (typeof v.id === 'number' && pending.has(v.id)) {
      pending.get(v.id)!(v);
      pending.delete(v.id);
    } else {
      events.push(v);
      onEvent?.(v);
    }
  });
  let seq = 0;
  const cmd = (c: Record<string, unknown>) =>
    new Promise<Record<string, unknown>>((ok, ng) => {
      const id = ++seq;
      const timer = setTimeout(() => ng(new Error(`応答が無い: ${JSON.stringify(c)}\n${stderr.join('')}`)), 20000);
      pending.set(id, (v) => {
        clearTimeout(timer);
        ok(v);
      });
      p.stdin.write(`${JSON.stringify({ ...c, id })}\n`);
    });
  const event = (t: string) =>
    new Promise<Record<string, unknown>>((ok, ng) => {
      const found = events.find((e) => e.t === t);
      if (found) {
        events.splice(events.indexOf(found), 1);
        return ok(found);
      }
      const timer = setTimeout(() => ng(new Error(`${t} が来ない\n${stderr.join('')}`)), 30000);
      onEvent = (e) => {
        if (e.t !== t) return;
        clearTimeout(timer);
        events.splice(events.indexOf(e), 1);
        onEvent = null;
        ok(e);
      };
    });
  const ready = await event('ready');
  const state = async () => (await cmd({ t: 'state' })) as unknown as State;
  const rects = async () => ((await cmd({ t: 'rects' })) as { rects: Record<string, Rect> }).rects;
  const click = async (x: number, y: number) => {
    await cmd({ t: 'down', x, y, clicks: 1 });
    await cmd({ t: 'up' });
  };
  const clickRect = (r: Rect) => click(r.x + r.w / 2, r.y + r.h / 2);
  const capture = async (file: string) => {
    const c = await cmd({ t: 'capture', path: file });
    assert.equal(c.ok, true, 'PNG を書けた');
  };
  const stop = async () => {
    p.stdin.write(`${JSON.stringify({ t: 'quit' })}\n`);
    await new Promise((r) => p.on('exit', r));
  };
  return { p, ready, cmd, state, rects, click, clickRect, capture, stop, stderr };
}

let host = await startHost(['--fresh']);

await step('起動: 描画プロセスが wgpu-native で準備でき、shell の窓とホストのパネルを 1 回のインスタンス描画で描く', async () => {
  assert.match(String(host.ready.adapter), /Vulkan|Metal|Dx12|Gl/);
  const s = await host.state();
  assert.deepEqual(s.windows.map((w) => w.id), ['todo', 'settings', 'notes']);
  for (const t of ['やること', '設定', 'メモ', 'システム (ホストの UI)']) assert.ok(s.texts.includes(t), t);
  assert.ok(s.instances > 200, `インスタンス ${s.instances}`);
  if (shots) await host.capture(path.join(shotsDir, 'native-start.png'));
});

await step('ポインタ: Todo のチェックを押すと、意味的な操作になり、保存はファイル (KeyValue) に書かれる', async () => {
  const r = await host.rects();
  await host.clickRect(r['todo/i1']);
  const tree = (await host.cmd({ t: 'tree', app: 'todo' })) as { tree: { nodes: { id: string; checked?: boolean }[] }[] };
  assert.equal(tree.tree[0].nodes.find((n) => n.id === 'i1')!.checked, true);
  assert.match(JSON.parse(readFileSync(kvFile, 'utf8'))['app.todo.items'], /"done":true/);
});

await step('文字入力: 入力欄を押すと IME が有効になり、変換中の文字は木に入れず、確定した文字だけが入る。追加で項目が増える', async () => {
  const r = await host.rects();
  await host.clickRect(r['todo/input']);
  assert.equal((await host.state()).ime, 'ime-enable');
  await host.cmd({ t: 'preedit', text: 'たまご' });
  const value = async () => ((await host.cmd({ t: 'tree', app: 'todo' })) as { tree: { nodes: { id: string; value?: string }[] }[] }).tree[0].nodes.find((n) => n.id === 'input')!.value;
  assert.equal(await value(), '', '変換中は木に入れない');
  assert.ok((await host.state()).texts.includes('たまご'), '変換中の文字は描く');
  await host.cmd({ t: 'text', text: '卵を買う' });
  assert.equal(await value(), '卵を買う');
  await host.clickRect((await host.rects())['todo/add']);
  const s = await host.state();
  assert.ok(s.texts.includes('卵を買う'));
  assert.match(JSON.parse(readFileSync(kvFile, 'utf8'))['app.todo.items'], /卵を買う/);
});

await step('キー: Alt+2 で agent のワークスペース (作業場と札)、Alt+1 で戻る', async () => {
  await host.cmd({ t: 'key', code: 'Digit2', key: '', alt: true, shift: false });
  let s = await host.state();
  assert.equal(s.workspace, 2);
  assert.ok(s.texts.includes('agent の作業場') && s.texts.includes('agent のワークスペース'));
  await host.cmd({ t: 'key', code: 'Digit1', key: '', alt: true, shift: false });
  s = await host.state();
  assert.equal(s.workspace, 1);
});

await step('許可の要求はホストのパネルに出る。キーボード (F6 → Tab → Enter) で許可すると、通知が Canvas の上に出る', async () => {
  // 残りを完了にすると Todo が通知を出そうとする (初めては許可の要求)
  const r = await host.rects();
  await host.clickRect(r['todo/i3']);
  const tree = (await host.cmd({ t: 'tree', app: 'todo' })) as { tree: { nodes: { id: string; role: string }[] }[] };
  const added = tree.tree[0].nodes.find((n) => n.role === 'checkbox' && !['i1', 'i2', 'i3'].includes(n.id))!;
  await host.clickRect((await host.rects())[`todo/${added.id}`]);
  let s = await host.state();
  assert.ok(s.prompts.some((p) => p.status === 'pending'));
  assert.ok(s.buttons.some((b) => b.label === '許可'));
  await host.cmd({ t: 'key', code: 'F6', key: 'F6', alt: false, shift: false });
  s = await host.state();
  assert.equal(s.panelFocus, 0);
  const allow = s.buttons.findIndex((b) => b.label === '許可');
  for (let i = 0; i < allow; i++) await host.cmd({ t: 'key', code: 'Tab', key: 'Tab', alt: false, shift: false });
  assert.equal((await host.state()).panelFocus, allow);
  await host.cmd({ t: 'key', code: 'Enter', key: 'Enter', alt: false, shift: false });
  s = await host.state();
  assert.ok(s.prompts.every((p) => p.status !== 'pending'));
  // 許可のあとで、もう一度すべて完了にする
  await host.cmd({ t: 'key', code: 'Escape', key: 'Escape', alt: false, shift: false });
  await host.clickRect((await host.rects())[`todo/${added.id}`]);
  await host.clickRect((await host.rects())[`todo/${added.id}`]);
  s = await host.state();
  assert.ok(s.texts.includes('Todo からの通知'), 'shell が描いた通知');
  if (shots) await host.capture(path.join(shotsDir, 'native-notice.png'));
});

await step('再起動: ファイルからセッションと保存領域が戻る', async () => {
  await host.stop();
  host = await startHost();
  const s = await host.state();
  assert.deepEqual(s.windows.map((w) => w.id), ['todo', 'settings', 'notes']);
  assert.ok(s.texts.includes('卵を買う'));
});

await step('ダークテーマ: 同じ画面を THEMES.dark の色で描く', async () => {
  await host.stop();
  host = await startHost(['--theme', 'dark']);
  const s = await host.state();
  assert.ok(s.texts.includes('ダーク・1x'));
  const file = path.join(shots ? shotsDir : tmp, 'native-dark.png');
  await host.capture(file);
  assert.ok(statSync(file).size > 20000);
});

await step('ホストのシェル: --pty では擬似端末の helper を直接起こす (WebSocket の橋は使わない)。許可はパネルで答える', async () => {
  await host.stop();
  host = await startHost(['--pty']);
  assert.match(JSON.stringify(await host.cmd({ t: 'launch', app: 'terminal' })), /"result":""/);
  // libghostty-vt の wasm を読み終えると、「ホストのシェルにつなぐ」が出る
  const rectOf = async (key: string) => {
    for (let i = 0; i < 50; i++) {
      const r = (await host.rects())[key];
      if (r) return r;
      await new Promise((ok) => setTimeout(ok, 100));
    }
    throw new Error(`${key} が出ない: ${Object.keys(await host.rects()).filter((k) => k.startsWith('terminal')).join(', ')} / ${JSON.stringify((await host.state()).windows)}`);
  };
  await host.clickRect(await rectOf('terminal/host'));
  let s = await host.state();
  const allow = s.buttons.find((b) => b.label === '許可')!;
  assert.ok(allow, 'terminal.pty の許可の要求');
  await host.clickRect(allow.rect);
  await rectOf('terminal/ctrlc'); // 許可されると、端末がそのまま本物のシェルにつなぐ
  const screen = async () => ((await host.cmd({ t: 'tree', app: 'terminal' })) as { tree: { nodes: { id: string; value?: string }[] }[] }).tree[0].nodes.find((n) => n.id === 'screen')!.value ?? '';
  for (let i = 0; i < 50 && !/\$ ?$/m.test(await screen()); i++) await new Promise((r) => setTimeout(r, 100));
  await host.clickRect((await host.rects())['terminal/input']);
  await host.cmd({ t: 'text', text: 'echo $((6*7))' });
  await host.cmd({ t: 'key', code: 'Enter', key: 'Enter', alt: false, shift: false });
  for (let i = 0; i < 50 && !/^42$/m.test(await screen()); i++) await new Promise((r) => setTimeout(r, 100));
  assert.match(await screen(), /^42$/m, await screen());
  s = await host.state();
  if (shots) await host.capture(path.join(shotsDir, 'native-terminal.png'));
});

// ---- MCP: AI のクライアント → MCP サーバ → ネイティブのホスト。承認はホストのパネル (人) ----
await host.stop();
const wsPort = 8300 + Math.floor(Math.random() * 500);
const client = new Client({ name: 'e2e-native', version: '0.0.1' });
await client.connect(new StdioClientTransport({ command: process.execPath, args: [path.join(root, 'mcp', 'server.ts')], env: { ...env, MCP_WS_PORT: String(wsPort) } as Record<string, string> }));
const call = async (name: string, args: Record<string, unknown> = {}) => {
  const r = (await client.callTool({ name, arguments: args })) as { content: { text: string }[]; isError?: boolean };
  let value: Record<string, unknown> = {};
  try {
    value = JSON.parse(r.content[0].text) as Record<string, unknown>;
  } catch {
    /* エラーの文字列 */
  }
  return { isError: !!r.isError, value, text: r.content[0].text };
};
host = await startHost(['--fresh', '--mcp', `ws://127.0.0.1:${wsPort}`]);

await step('MCP: AI はネイティブのホストの画面を読み、提案はホストのパネルで人が承認する', async () => {
  for (let i = 0; i < 50 && (await call('list_apps')).isError; i++) await new Promise((r) => setTimeout(r, 100));
  const apps = await call('list_apps');
  assert.equal(apps.isError, false, apps.text);
  assert.deepEqual((apps.value.shown as { app: string }[]).map((a) => a.app), ['todo', 'settings', 'notes']);
  const p = await call('propose', { intent: 'コンパクト表示にする', steps: [{ app: 'settings', target: 'compact', action: 'click' }] });
  const id = (p.value as { id: string }).id;
  const s = await host.state();
  assert.ok(s.texts.includes(`AI の提案 ${id}`), 'パネルに提案が出る');
  assert.ok(s.texts.includes('MCP: 接続中'));
  if (shots) await host.capture(path.join(shotsDir, 'native-proposal.png'));
  const approve = s.buttons.find((b) => b.action === `prop:${id}:1`)!;
  await host.clickRect(approve.rect);
  const done = await call('wait_proposal', { id, timeout_s: 5 });
  assert.equal(done.value.status, 'done');
});

await step('MCP: agent のワークスペースでは承認なしで動かし、持ち込みはパネルで人が承認する', async () => {
  const l = await call('workspace_launch', { app: 'notes' });
  assert.equal(l.isError, false, l.text);
  const m = await call('propose_merge', { intent: 'メモを足す' });
  assert.equal(m.isError, true, '変更が無いうちは持ち込めない');
  // notes は人の側で開いているので、agent の側は別の窓 (notes-2)。保存は重ねた層に書く
  const notes = (l.value as unknown as { windows: { id: string }[] }).windows.at(-1)!.id;
  assert.notEqual(notes, 'notes');
  assert.equal((await call('workspace_act', { app: notes, target: 'input', action: 'setValue', data: 'agent のメモ' })).isError, false);
  assert.equal((await call('workspace_act', { app: notes, target: 'save', action: 'click' })).isError, false);
  const pm = await call('propose_merge', { intent: 'メモを足す' });
  assert.equal(pm.isError, false, pm.text);
  const mid = (pm.value as { id: string }).id;
  const s = await host.state();
  const write = s.buttons.find((b) => b.action === `merge:${mid}:1`)!;
  assert.ok(write, 'パネルに持ち込みの提案');
  await host.clickRect(write.rect);
  const w = await call('wait_merge', { id: mid, timeout_s: 5 });
  assert.equal(w.value.status, 'applied');
  assert.ok((await host.state()).texts.includes('agent のメモ'), '人のワークスペースの Notes に出る');
});

await host.stop();
await client.close();
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
