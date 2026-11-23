// ネイティブのホスト: ブラウザの main.ts の代わりに、Node でプラットフォームを組み立て (createPlatform は DOM に依存しない)、
// 描画は Odin の描画プロセス (native/renderer: SDL3 の窓 + wgpu-native) に任せる。
//
//   node native/host.ts                          # 窓を開く
//   node native/host.ts --headless 1280x800      # 窓なし (オフスクリーン)。--control で標準入力から操作・画面の書き出し
//   オプション: --kv <file> (保存先) / --font <ttf> / --theme light|dark / --pty (ホストの bash) / --mcp ws://127.0.0.1:8130 / --renderer <path>
//
// 役割の分け方
// - Node: プラットフォーム (アプリ・能力・保存・セッション)、shell (意味ツリー → 描画命令)、文字の計測 (mizchi/font)、人だけが答えるパネル
// - 描画プロセス: 描画命令を GPU で描く (ブラウザの WebGPU 版と同じ Odin のコード)、SDL3 の入力と IME
import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import { builtinManifests } from '../src/apps/manifests.ts';
import { terminalManifest } from '../src/apps/terminal/terminal.ts';
import { createVt, instantiateGhostty } from '../src/apps/terminal/vt.ts';
import { createFontMeasure } from '../src/backend/font-metrics.ts';
import { mapKey } from '../src/backend/keys.ts';
import type { CacheEntry, Storage } from '../src/cache.ts';
import { connectMcp } from '../src/mcp-link.ts';
import { createPlatform } from '../src/platform/platform.ts';
import type { DrawOp, ImeToHost } from '../src/protocol.ts';
import type { Shell, Step } from '../src/shell.ts';
import { THEMES } from '../src/theme.ts';
import type { ThemeName } from '../src/theme.ts';
import { fileKeyValue } from './kv.ts';
import { PANEL_W, drawPanel } from './panel.ts';
import type { PanelButton } from './panel.ts';
import { directPtyConnector } from './pty.ts';
import { CURSOR, MSG, encodeFrame, encodeIme, message } from './wire.ts';
import type { RendererEvent } from './wire.ts';

// ---- 引数 ----
const argv = process.argv.slice(2);
const opt = (name: string) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : undefined;
};
const flag = (name: string) => argv.includes(name);
const here = (p: string) => fileURLToPath(new URL(p, import.meta.url));
const headless = opt('--headless');
const control = flag('--control');
const kvPath = opt('--kv') ?? join(process.env.XDG_DATA_HOME ?? join(homedir(), '.local', 'share'), 'ai-native-desktop', 'kv.json');
const fontPath = opt('--font') ?? here('../www/fonts/ipaexg.ttf');
const rendererPath = opt('--renderer') ?? here('./bin/renderer');
const log = (...a: unknown[]) => console.error('[host]', ...a);
/** --control のときの応答 (標準出力の JSON Lines) */
const reply = (v: unknown) => control && process.stdout.write(`${JSON.stringify(v)}\n`);

// ---- プラットフォーム ----
const kv = fileKeyValue(kvPath);
const TEMPLATES_KEY = 'shell.templates';
const templates: Storage = {
  load: () => {
    try {
      return JSON.parse(kv.get(TEMPLATES_KEY) ?? '[]') as CacheEntry[];
    } catch {
      return [];
    }
  },
  save: (e) => kv.set(TEMPLATES_KEY, JSON.stringify(e)),
};
const fontData = new Uint8Array(readFileSync(fontPath));
const measure = createFontMeasure(fontData, 'ShellFont');
const terminal = terminalManifest(async () => {
  const g = await instantiateGhostty(await readFile(here('../node_modules/ghostty-web/ghostty-vt.wasm')));
  return (c, r) => createVt(g, c, r);
});
const platform = createPlatform({ apps: [...builtinManifests, terminal], initial: ['todo', 'settings', 'notes'], measure, templates, kv, restoreSession: !flag('--fresh') });
if (flag('--pty')) platform.services.pty = directPtyConnector();
const theme: ThemeName = (opt('--theme') as ThemeName | undefined) ?? (kv.get('platform.theme') === 'dark' ? 'dark' : 'light');
const colors = THEMES[theme];

let shell: Shell = platform.shell;
let workspace: 1 | 2 = 1;
const switchWorkspace = (n: number) => {
  workspace = n === 2 ? 2 : 1;
  shell = workspace === 1 ? platform.shell : platform.agentShell;
  schedule();
};
for (const sh of [platform.shell, platform.agentShell]) sh.onWorkspaceKey = switchWorkspace;
platform.shell.onChange = () => {
  schedule();
  platform.saveSession();
};
platform.agentShell.onChange = schedule;
platform.services.onChange = schedule;

// ---- MCP (AI の口。ブラウザと同じ connectMcp。Node 24 の WebSocket を使う) ----
let mcpStatus = '';
const mcpUrl = opt('--mcp');
if (mcpUrl)
  connectMcp(
    mcpUrl,
    platform.shell,
    (s) => {
      mcpStatus = s;
      schedule();
    },
    () => platform.registry.list().map((m) => ({ id: m.id, title: m.title, description: m.description, multiInstance: m.multiInstance, capabilities: m.capabilities })),
    {
      getTree: (app) => platform.agentShell.getTree(app),
      windows: () => platform.agentShell.windows(),
      launch: (a) => platform.agent.launch(a),
      act: (app, target, action, data) => platform.agent.act(app, target, action, data),
      proposeMerge: (intent) => platform.services.proposeMerge(intent, 'agent'),
      merge: (id) => platform.services.merges.find((m) => m.id === id),
    },
  );

// ---- 描画プロセス ----
const rendererArgs = headless ? ['--headless', headless, ...(opt('--scale') ? ['--scale', opt('--scale')!] : [])] : [];
const renderer = spawn(rendererPath, rendererArgs, { stdio: ['pipe', 'pipe', 'inherit'] });
renderer.on('error', (e) => {
  log(`描画プロセスを起こせない (${rendererPath}): ${e.message}。native/build.sh でビルドする`);
  process.exit(1);
});
renderer.on('exit', (code) => {
  log(`描画プロセスが終わった (${code})`);
  process.exit(code ?? 0);
});
// 描画プロセスが先に終わった (窓を閉じたなど) あとの書き込みは捨てる
renderer.stdin!.on('error', () => {});
const send = (type: number, body?: Uint8Array) => renderer.stdin!.writable && renderer.stdin!.write(message(type, body));

let size = { w: 0, h: 0, scale: 1 };
let ready = false;
let inflight = false;
let dirty = false;
let wakeTimer: ReturnType<typeof setTimeout> | undefined;
let lastOps: DrawOp[] = [];
let lastInstances = 0;
let lastMs = 0;
/** 直近のフレームの、ホスト側の時間 (shell.frame + パネル + 命令の符号化) */
let hostMs = 0;
let ime: ImeToHost = { t: 'ime-disable' };
let cursor = 'default';
let buttons: PanelButton[] = [];
/** パネルのボタンのキーボードのフォーカス (null ならデスクトップ) */
let panelFocus: number | null = null;
const waiters: ((v: unknown) => void)[] = [];
/** 最初のフレームを描き終えた */
let framed = false;
/** 画面の書き出しを頼んだ --control の id (描画プロセスは順に答える) */
const captureIds: (number | undefined)[] = [];

const panelWidth = () => (size.w >= PANEL_W + 400 ? PANEL_W : 0);

function schedule() {
  dirty = true;
  if (ready && !inflight) setImmediate(render);
}

const describeStep = (s: Step) =>
  'kind' in s && s.kind === 'layout' ? `レイアウト: ${s.op.type}${'app' in s.op && s.op.app ? ` ${s.op.app}` : ''}` : `${s.app} / ${s.target} を ${s.action}${s.data !== undefined ? ` "${s.data}"` : ''}`;
const CAP: Record<string, string> = {
  storage: '保存領域',
  'clipboard.read': 'クリップボードの読み取り',
  'clipboard.write': 'クリップボードへの書き込み',
  notifications: '通知',
  'apps.launch': 'アプリの起動',
  'terminal.pty': 'ホストのシェル (擬似端末)',
};
const capLabel = (cap: string) => {
  if (!cap.startsWith('data:')) return CAP[cap] ?? cap;
  const topic = cap.slice(5);
  const m = platform.registry.list({ includeSystem: true }).find((x) => x.provides?.[topic]);
  return m ? `${m.title} の「${m.provides![topic].description}」の読み取り` : `${topic} の読み取り`;
};

function render() {
  if (!ready || inflight || !dirty) return;
  dirty = false;
  const pw = panelWidth();
  const sw = size.w - pw;
  const now = performance.now();
  const f = shell.frame(sw, size.h, now);
  (shell === platform.shell ? platform.agentShell : platform.shell).setSize(sw, size.h);
  const sv = platform.services;
  const panel = pw
    ? drawPanel(
        {
          workspace,
          merges: sv.merges.filter((m) => m.status === 'pending'),
          prompts: sv.prompts.filter((p) => p.status === 'pending').map((p) => ({ id: p.id, text: `${p.appTitle} が ${capLabel(p.capability)} の許可を求めている` })),
          proposals: platform.shell.proposals.filter((p) => p.status === 'pending').map((p) => ({ id: p.id, intent: p.intent, steps: p.steps.map(describeStep), sensitive: p.sensitive })),
          status: [mcpStatus, `${theme === 'dark' ? 'ダーク' : 'ライト'}・${size.scale}x`].filter(Boolean),
        },
        sw,
        size.h,
        measure,
        panelFocus,
      )
    : { ops: [], buttons: [] };
  buttons = panel.buttons;
  if (panelFocus !== null && panelFocus >= buttons.length) panelFocus = buttons.length ? buttons.length - 1 : null;
  lastOps = [...f.ops, ...panel.ops];
  inflight = true;
  const body = encodeFrame(lastOps, colors);
  hostMs = performance.now() - now;
  send(MSG.frame, body);
  // IME とカーソルは変わったときだけ送る
  const imeKey = (i: ImeToHost) => (i.t === 'ime-enable' ? `${i.node}/${i.caret.x}/${i.caret.y}/${i.caret.h}` : '');
  if (imeKey(f.ime) !== imeKey(ime)) send(MSG.ime, f.ime.t === 'ime-enable' ? encodeIme(true, { ...f.ime.caret, w: f.ime.caret.w ?? 2 }) : encodeIme(false));
  ime = f.ime;
  if (f.cursor !== cursor) send(MSG.cursor, new Uint8Array([CURSOR[f.cursor] ?? 0]));
  cursor = f.cursor;
  if (f.animating) dirty = true;
  else if (f.wakeAt !== undefined) {
    clearTimeout(wakeTimer);
    wakeTimer = setTimeout(schedule, Math.max(0, f.wakeAt - performance.now()));
  }
}

// ---- 入力 ----
let pointerInPanel = false;
let pendingDouble: { x: number; y: number } | null = null;

function press(action: string) {
  const [kind, id, ok] = action.split(':');
  if (kind === 'ws') switchWorkspace(Number(id));
  else if (kind === 'grant') platform.services.answer(id, ok === '1', 'human');
  else if (kind === 'merge') platform.services.answerMerge(id, ok === '1', 'human');
  else if (kind === 'prop') ok === '1' ? platform.shell.approve(id) : platform.shell.reject(id);
  schedule();
}

function onInput(e: RendererEvent) {
  const pw = panelWidth();
  const inPanel = (x: number) => pw > 0 && x >= size.w - pw;
  switch (e.t) {
    case 'down':
      pointerInPanel = inPanel(e.x);
      if (pointerInPanel) {
        const b = buttons.find((b) => e.x >= b.rect.x && e.x < b.rect.x + b.rect.w && e.y >= b.rect.y && e.y < b.rect.y + b.rect.h);
        if (b) press(b.action);
        break;
      }
      panelFocus = null;
      shell.pointerDown(e.x, e.y);
      if (e.clicks === 2) pendingDouble = { x: e.x, y: e.y };
      break;
    case 'move':
      if (!pointerInPanel) shell.pointerMove(Math.min(e.x, size.w - pw - 1), e.y);
      break;
    case 'up':
      if (!pointerInPanel) {
        shell.pointerUp();
        if (pendingDouble) shell.doubleClick(pendingDouble.x, pendingDouble.y);
      }
      pendingDouble = null;
      pointerInPanel = false;
      break;
    case 'wheel':
      if (!inPanel(e.x)) shell.wheel(e.x, e.y, e.dy);
      break;
    case 'key': {
      if (e.code === 'F6') {
        panelFocus = panelFocus === null && buttons.length ? 0 : null;
        break;
      }
      if (panelFocus !== null) {
        if (e.code === 'Tab') panelFocus = (panelFocus + (e.shift ? buttons.length - 1 : 1)) % buttons.length;
        else if (e.code === 'Enter' || e.code === 'Space') press(buttons[panelFocus].action);
        else if (e.code === 'Escape') panelFocus = null;
        break;
      }
      const m = mapKey(e, ime.t === 'ime-enable');
      if (m) shell.input(m);
      break;
    }
    case 'text':
      if (panelFocus === null) shell.input({ t: 'ime-commit', text: e.text });
      break;
    case 'preedit':
      if (panelFocus === null) shell.input({ t: 'ime-preedit', text: e.text });
      break;
  }
  schedule();
}

createInterface({ input: renderer.stdout! }).on('line', (line) => {
  let e: RendererEvent;
  try {
    e = JSON.parse(line) as RendererEvent;
  } catch {
    log('描画プロセスの出力を読めない:', line);
    return;
  }
  switch (e.t) {
    case 'ready':
      size = { w: e.w, h: e.h, scale: e.scale };
      log(`描画: ${e.adapter}, ${e.format}, ${e.w}x${e.h} @${e.scale}x${e.headless ? ' (窓なし)' : ''}`);
      send(MSG.font, fontData);
      ready = true;
      reply({ t: 'ready', w: e.w, h: e.h, scale: e.scale, adapter: e.adapter });
      schedule();
      break;
    case 'size':
      size = { w: e.w, h: e.h, scale: e.scale };
      schedule();
      break;
    case 'frame':
      inflight = false;
      framed = true;
      if (!e.skipped) {
        lastInstances = e.n;
        lastMs = e.ms;
      }
      if (dirty) setImmediate(render);
      else for (const w of waiters.splice(0)) w(null);
      break;
    case 'captured':
      reply({ ...e, id: captureIds.shift() });
      break;
    case 'close':
      send(MSG.quit);
      break;
    case 'error':
      log('描画プロセス:', e.message);
      reply(e);
      break;
    default:
      onInput(e);
  }
});

// ---- --control: 標準入力の JSON Lines で操作する (テスト・自動化用。窓なしでは入力の代わり) ----
/** 描き終わって落ち着くまで待つ (アニメーションも済ませる) */
const settle = () =>
  new Promise<unknown>((r) => {
    if (framed && !inflight && !dirty) r(null);
    else waiters.push(r);
  });
if (control)
  createInterface({ input: process.stdin }).on('line', async (line) => {
    const c = JSON.parse(line) as { t: string; id?: number; path?: string } & Record<string, unknown>;
    await settle();
    switch (c.t) {
      case 'state': {
        const texts = lastOps.filter((o) => o[0] === 'text').map((o) => (o[1] as { str: string }).str);
        reply({
          t: 'state',
          id: c.id,
          workspace,
          size,
          panelFocus,
          windows: shell.windows().map((w) => ({ id: w.id, title: w.title })),
          focused: shell.accessibility().windows.find((w) => w.focused)?.app ?? null,
          ime: ime.t,
          buttons: buttons.map((b) => ({ action: b.action, label: b.label, rect: b.rect })),
          texts,
          ops: lastOps.length,
          instances: lastInstances,
          gpuMs: lastMs,
          hostMs,
          proposals: platform.shell.proposals.map((p) => ({ id: p.id, status: p.status })),
          prompts: platform.services.prompts.map((p) => ({ id: p.id, status: p.status })),
          kv: kvPath,
        });
        break;
      }
      case 'rects':
        // 操作できる widget の位置 (shell の hit test と同じもの)。テストが押す場所を知るため
        reply({ t: 'rects', id: c.id, rects: Object.fromEntries(shell.accessibility().rects) });
        break;
      case 'tree':
        reply({ t: 'tree', id: c.id, tree: shell.getTree(c.app as string | undefined) });
        break;
      case 'capture':
        captureIds.push(c.id);
        send(MSG.capture, new TextEncoder().encode(c.path!));
        break;
      case 'launch':
        // 人がランチャーから開くのと同じ (テストで使う)
        reply({ t: 'launched', id: c.id, result: platform.launch(c.app as string, 'human') });
        break;
      case 'propose':
        reply({ t: 'proposed', id: c.id, result: platform.shell.propose(c.intent as string, c.steps as Step[]) });
        break;
      case 'quit':
        send(MSG.quit);
        break;
      default:
        // down / move / up / wheel / key / text / preedit: 描画プロセスから来る入力と同じ形
        onInput(c as unknown as RendererEvent);
        await settle();
        reply({ t: 'done', id: c.id });
    }
  });

process.on('SIGINT', () => send(MSG.quit));
