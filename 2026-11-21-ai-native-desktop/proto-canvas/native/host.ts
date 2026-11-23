// ネイティブのホスト: ブラウザの main.ts の代わりに、Node でプラットフォームを組み立て (createPlatform は DOM に依存しない)、
// 描画は Odin の描画プロセス (native/renderer: SDL3 の窓 + wgpu-native) に任せる。
//
//   node native/host.ts                          # 窓を開く
//   node native/host.ts --headless 1280x800      # 窓なし (オフスクリーン)。--control で標準入力から操作・画面の書き出し
//   オプション: --kv <file> (保存先) / --font <ttf> / --theme light|dark / --pty (ホストの bash) / --mcp ws://127.0.0.1:8130 / --renderer <path>
//             --package <app.json> (パッケージを入れる。次の起動でも読む) / --a11y (AT-SPI にツリーを出す。Linux のスクリーンリーダー)
//
// 役割の分け方
// - Node: プラットフォーム (アプリ・能力・保存・セッション)、shell (意味ツリー → 描画命令)、文字の計測 (mizchi/font)、人だけが答えるパネル
// - 描画プロセス: 描画命令を GPU で描く (ブラウザの WebGPU 版と同じ Odin のコード)、SDL3 の入力と IME
import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
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
import type { AppManifest } from '../src/platform/types.ts';
import { fileKeyValue } from './kv.ts';
import { nativeRunner, readNativePackage } from './packages.ts';
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
/** 何度でも書けるオプション (--package a --package b) */
const opts = (name: string) => argv.flatMap((a, i) => (a === name && argv[i + 1] ? [argv[i + 1]] : []));
const here = (p: string) => fileURLToPath(new URL(p, import.meta.url));
const headless = opt('--headless');
// 窓の最小の幅 (shell + 人が答えるパネル)。窓なしでもこれより狭くはしない
const MIN_W = 720;
const MIN_H = 400;
if (headless) {
  const [w, h] = headless.split('x').map(Number);
  if (!(w >= MIN_W && h >= MIN_H)) {
    console.error(`[host] --headless は ${MIN_W}x${MIN_H} 以上 (パネルを出す幅が要る): ${headless}`);
    process.exit(2);
  }
}
const control = flag('--control');
const kvPath = opt('--kv') ?? join(process.env.XDG_DATA_HOME ?? join(homedir(), '.local', 'share'), 'ai-native-desktop', 'kv.json');
const fontPath = opt('--font') ?? here('../www/fonts/ipaexg.ttf');
const rendererPath = opt('--renderer') ?? here('./bin/renderer');
const log = (...a: unknown[]) => console.error('[host]', ...a);
/** --control のときの応答 (標準出力の JSON Lines) */
const reply = (v: unknown) => control && process.stdout.write(`${JSON.stringify(v)}\n`);

// ---- プラットフォーム ----
let kv: ReturnType<typeof fileKeyValue>;
try {
  kv = fileKeyValue(kvPath);
} catch (e) {
  log(String((e as Error).message ?? e));
  process.exit(1);
}
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
// ---- パッケージ (app.json + WIT のコンポーネント)。コンポーネントは worker_threads で動かし、応答しなければ止める ----
// 入れたパッケージ (app.json のパス) は保存しておき、次の起動でも読む (窓の復元より先に)。--package で足す
const PACKAGES_KEY = 'native.packages';
const savedPackages = (() => {
  try {
    const v: unknown = JSON.parse(kv.get(PACKAGES_KEY) ?? '[]');
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
  } catch {
    return [];
  }
})();
const packagePaths = [...new Set([...savedPackages, ...opts('--package').map((p) => resolve(p))])];
const runner = nativeRunner(() => platform.services, { actionMs: Number(opt('--component-timeout') ?? 2000) });
const packageManifests: AppManifest[] = [];
const takenIds = new Set([...builtinManifests.map((m) => m.id), terminal.id]);
for (const path of packagePaths) {
  const r = await readNativePackage(path, runner);
  if (!r.ok) log(`パッケージ ${path} を読めない: ${r.errors.join(' / ')}`);
  else if (takenIds.has(r.manifest.id)) log(`パッケージ ${path} の id ${r.manifest.id} はすでに使われている。読み込まない`);
  else {
    takenIds.add(r.manifest.id);
    packageManifests.push(r.manifest);
    log(`パッケージ: ${r.manifest.id} (${path})`);
  }
}
kv.set(PACKAGES_KEY, JSON.stringify(packagePaths));

const platform = createPlatform({ apps: [...builtinManifests, terminal, ...packageManifests], initial: ['todo', 'settings', 'notes'], measure, templates, kv, restoreSession: !flag('--fresh') });
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
/**
 * 描画プロセスが表示したフレームのパネルのボタン。押す場所の判定はこれで行う (人が見た画面と同じもの)。
 * 送ったがまだ表示の返事が来ていないフレームのボタンは sentButtons に置き、返事が来たら入れ替える
 */
let presented: PanelButton[] = [];
let sentButtons: PanelButton[] | null = null;
/** ボタンが今の位置に表示された時刻。出たばかりのボタンは押させない (入れ替わった直後の押し間違い・誘導を防ぐ) */
const shownAt = new Map<string, number>();
const PRESS_DELAY_MS = 500;
const buttonKey = (b: PanelButton) => `${b.action}@${b.rect.x},${b.rect.y}`;
/** パネルのキーボードのフォーカス (ボタンの action。null ならデスクトップ)。並びが変わっても同じボタンに留まる */
let panelFocus: string | null = null;
let panelScroll = 0;
const waiters: ((v: unknown) => void)[] = [];
/** 最初のフレームを描き終えた */
let framed = false;
/** 画面の書き出しを頼んだ --control の id (描画プロセスは順に答える) */
const captureIds: (number | undefined)[] = [];

/** パネルはいつも出す (人が答える口を失わない)。窓の最小の幅は描画プロセスが 720px にする */
const panelWidth = () => PANEL_W;

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
  const st = {
    workspace,
    merges: sv.merges.filter((m) => m.status === 'pending'),
    prompts: sv.prompts.filter((p) => p.status === 'pending').map((p) => ({ id: p.id, text: `${p.appTitle} が ${capLabel(p.capability)} の許可を求めている` })),
    proposals: platform.shell.proposals.filter((p) => p.status === 'pending').map((p) => ({ id: p.id, intent: p.intent, steps: p.steps.map(describeStep), sensitive: p.sensitive })),
    status: [mcpStatus, `${theme === 'dark' ? 'ダーク' : 'ライト'}・${size.scale}x`].filter(Boolean),
  };
  let panel = drawPanel(st, sw, size.h, measure, panelFocus, panelScroll);
  // フォーカスしたボタン (答えられて消えたら外す。ほかのボタンへは移さない)
  if (panelFocus !== null && !panel.buttons.some((b) => b.action === panelFocus)) panelFocus = null;
  // スクロール: フォーカスしたボタンを見える位置まで送り、中身の高さで抑える
  let scroll = panelScroll;
  const fb = panel.buttons.find((b) => b.action === panelFocus);
  if (fb && !fb.visible) scroll += fb.rect.y < 0 ? fb.rect.y - 12 : fb.rect.y + fb.rect.h - size.h + 12;
  scroll = Math.max(0, Math.min(scroll, panel.contentH - size.h));
  if (scroll !== panelScroll) {
    panelScroll = scroll;
    panel = drawPanel(st, sw, size.h, measure, panelFocus, panelScroll);
  }
  sentButtons = panel.buttons;
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

// ---- スクリーンリーダー (Linux の AT-SPI)。ブラウザの ARIA のミラーの代わり ----
// native/atspi/server.py (system の Python 3.12 + Gio) に意味ツリーとパネルを渡し、スクリーンリーダーからの操作を人の操作として受ける
let a11y: ReturnType<typeof spawn> | null = null;
let a11ySent = '';
const announced = { proposals: 0, prompts: 0, notes: 0 };
if (flag('--a11y')) {
  a11y = spawn(process.env.ATSPI_PYTHON ?? '/usr/bin/python3.12', [here('./atspi/server.py')], { stdio: ['pipe', 'pipe', 'inherit'] });
  a11y.stdin!.on('error', () => {});
  a11y.on('exit', (code) => {
    log(`AT-SPI のサーバが終わった (${code})`);
    a11y = null;
  });
  createInterface({ input: a11y.stdout! }).on('line', (line) => {
    let m: { t: string; app?: string; node?: string; action?: string; data?: string; message?: string; bus?: string };
    try {
      m = JSON.parse(line);
    } catch {
      return;
    }
    if (m.t === 'ready') {
      log(`AT-SPI: ${m.bus}`);
      a11ySent = '';
      reply({ t: 'a11y-ready', bus: m.bus });
      schedule();
    } else if (m.t === 'error') log('AT-SPI:', m.message);
    else if (m.t === 'panel' && m.action) press(m.action);
    else if (m.t === 'action' && m.app && m.node) {
      // スクリーンリーダーの操作は、人の意味的な操作 (ブラウザの ARIA のミラーと同じ)
      if (m.action === 'click') shell.activate(m.app, m.node);
      else if (m.action === 'focus') shell.dispatch({ app: m.app, target: m.node, action: 'focus', by: 'human' });
      else if (m.action === 'setValue') shell.dispatch({ app: m.app, target: m.node, action: 'setValue', data: m.data ?? '', by: 'human' });
      schedule();
    }
  });
}
function syncA11y() {
  if (!a11y?.stdin?.writable) return;
  const a = shell.accessibility();
  const msg = JSON.stringify({
    t: 'tree',
    windows: a.windows.map((w) => ({ ...w, focus: shell.tree(w.app).focus })),
    rects: Object.fromEntries(a.rects),
    panel: presented.map((b) => ({ action: b.action, label: b.label, rect: b.rect })),
  });
  if (msg !== a11ySent) {
    a11ySent = msg;
    a11y.stdin.write(`${msg}\n`);
  }
  // 提案・許可の要求・通知を読み上げる (ブラウザの aria-live と同じもの)
  const say = (text: string) => a11y!.stdin!.write(`${JSON.stringify({ t: 'announce', text })}\n`);
  const sv = platform.services;
  for (const p of platform.shell.proposals.slice(announced.proposals)) say(`AI の提案 ${p.id}: ${p.intent}。${p.sensitive ? '承認が必要です。' : ''}`);
  announced.proposals = platform.shell.proposals.length;
  for (const p of sv.prompts.slice(announced.prompts)) say(`${p.appTitle} が ${capLabel(p.capability)} の許可を求めています`);
  announced.prompts = sv.prompts.length;
  const last = sv.notifications.at(-1);
  if (last && last.id > announced.notes) {
    say(`通知 ${last.title}: ${last.body ?? ''}`);
    announced.notes = last.id;
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
        const b = presented.find((b) => b.visible && e.x >= b.rect.x && e.x < b.rect.x + b.rect.w && e.y >= b.rect.y && e.y < b.rect.y + b.rect.h);
        if (b && performance.now() - (shownAt.get(buttonKey(b)) ?? Infinity) >= PRESS_DELAY_MS) press(b.action);
        else if (b) log(`出たばかりのボタンは押さない (${b.label})`);
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
      else panelScroll += e.dy;
      break;
    case 'key': {
      if (e.code === 'F6') {
        panelFocus = panelFocus === null && presented.length ? presented[0].action : null;
        break;
      }
      if (panelFocus !== null) {
        const i = presented.findIndex((b) => b.action === panelFocus);
        if (e.code === 'Tab' && presented.length) panelFocus = presented[(i + (e.shift ? presented.length - 1 : 1) + presented.length) % presented.length].action;
        else if (e.code === 'Enter' || e.code === 'Space') {
          // 表示されていて、見える位置にあるボタンだけを押す (スクロールの途中なら、描き直しを待つ)
          if (i >= 0 && presented[i].visible) press(presented[i].action);
        } else if (e.code === 'Escape') panelFocus = null;
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
      // 表示されたフレームのボタンだけを、押せるボタンにする
      if (sentButtons && !e.skipped) {
        const now = performance.now();
        const keys = new Set(sentButtons.map(buttonKey));
        for (const k of shownAt.keys()) if (!keys.has(k)) shownAt.delete(k);
        for (const k of keys) if (!shownAt.has(k)) shownAt.set(k, now);
        presented = sentButtons;
      }
      sentButtons = null;
      if (!dirty) syncA11y();
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
    let c: { t: string; id?: number; path?: string } & Record<string, unknown>;
    try {
      const v: unknown = JSON.parse(line);
      if (!v || typeof v !== 'object' || Array.isArray(v) || typeof (v as { t?: unknown }).t !== 'string') throw new Error('t (文字列) を持つ object ではない');
      c = v as typeof c;
    } catch (err) {
      reply({ t: 'error', message: `読めない命令: ${String((err as Error).message ?? err)}` });
      return;
    }
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
          buttons: presented.map((b) => ({ action: b.action, label: b.label, rect: b.rect, visible: b.visible })),
          panelScroll,
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
      case 'close':
        reply({ t: 'closed', id: c.id, result: platform.close(c.app as string, 'human') });
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
process.on('exit', () => a11y?.kill());
