import { builtinManifests, todoManifest } from './apps/manifests.ts';
import { terminalManifest } from './apps/terminal/terminal.ts';
import { wsPtyConnector } from './apps/terminal/pty-link.ts';
import { createTodoApp } from './apps/todo.ts';
import { createAriaMirror } from './backend/aria.ts';
import { createCanvasBackend } from './backend/canvas.ts';
import type { CacheEntry, Storage } from './cache.ts';
import type { ViewTemplate } from './catalog.ts';
import type { ActionName, Measure } from './protocol.ts';
import { layout } from './layout.ts';
import { LOWER_FONT, lower } from './lower.ts';
import { connectMcp } from './mcp-link.ts';
import { createPlatform } from './platform/platform.ts';
import { memoryKeyValue } from './platform/services.ts';
import type { KeyValue } from './platform/services.ts';
import type { AppManifest } from './platform/types.ts';
import { isLayoutStep } from './shell.ts';
import type { Shell, Step } from './shell.ts';

const KEY = 'ai-native-desktop.templates.v1';
const storage: Storage = {
  load() {
    try {
      return JSON.parse(localStorage.getItem(KEY) ?? '[]') as CacheEntry[];
    } catch {
      return [];
    }
  },
  save(entries) {
    try {
      localStorage.setItem(KEY, JSON.stringify(entries));
    } catch {
      /* 保存できなくても動く */
    }
  },
};

const canvas = document.getElementById('screen') as HTMLCanvasElement;
const sink = document.getElementById('sink') as HTMLTextAreaElement;
const panel = document.getElementById('panel')!;

let shell!: Shell;
let dirty = true;
const schedule = () => {
  if (dirty) return;
  dirty = true;
  requestAnimationFrame(render);
};

let measureImpl: Measure = () => 0;
const backend = createCanvasBackend(canvas, sink, {
  pointerDown: (x, y) => shell.pointerDown(x, y),
  pointerMove: (x, y) => shell.pointerMove(x, y),
  pointerUp: () => shell.pointerUp(),
  doubleClick: (x, y) => shell.doubleClick(x, y),
  wheel: (x, y, dy) => shell.wheel(x, y, dy),
  input: (m) => shell.input(m),
  resize: () => schedule(),
});
measureImpl = backend.measure;
// ?font=fonts/ipaexg.ttf: shell の計測を veriss と同じ mizchi/font にし、Canvas も同じフォントで描く (証明と描画の計測を一致させる)
const fontUrl = new URLSearchParams(location.search).get('font');
let lowerFamily = LOWER_FONT;
if (fontUrl) {
  const { loadFontMeasure } = await import('./backend/font-metrics.ts');
  measureImpl = await loadFontMeasure(fontUrl, 'ShellFont');
  backend.setFontFamily('ShellFont');
  lowerFamily = 'ShellFont';
}
// ?backend=odin-gpu&font=...: 描画と計測を Odin + wgpu (ブラウザでは WebGPU) に差し替える
if (new URLSearchParams(location.search).get('backend') === 'odin-gpu') {
  if (!fontUrl) throw new Error('backend=odin-gpu には font= が要る');
  const { createOdinGpu } = await import('./backend/odin-gpu.ts');
  const gpu = await createOdinGpu('gpu.wasm', fontUrl, { offscreen: new URLSearchParams(location.search).has('gpu-offscreen') });
  measureImpl = gpu.measure;
  const draw2d = backend.draw;
  backend.draw = (ops) => gpu.draw(ops, backend.colors(), backend.canvas);
  /** 検証用: 同じ描画命令を GPU (読み戻し) と Canvas 2D (上の canvas に一時的に描く) で描き、画素を比べる */
  const compareGpu = async () => {
    const c = backend.canvas;
    const { w, h } = backend.size();
    const ops = shell.frame(w, h, performance.now()).ops;
    gpu.draw(ops, backend.colors(), c);
    const g = await gpu.capture();
    draw2d(ops);
    const ctx = c.getContext('2d')!;
    const cpu = ctx.getImageData(0, 0, c.width, c.height);
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, c.width, c.height);
    let n = 0;
    for (let i = 0; i < cpu.data.length; i += 4)
      if (Math.abs(cpu.data[i] - g.data[i]) + Math.abs(cpu.data[i + 1] - g.data[i + 1]) + Math.abs(cpu.data[i + 2] - g.data[i + 2]) > 24) n++;
    return { diff: n / (cpu.data.length / 4), width: g.width, height: g.height, gpuPng: await toPng(g), cpuPng: await toPng(cpu) };
  };
  const toPng = (img: ImageData) => {
    const cv = document.createElement('canvas');
    cv.width = img.width;
    cv.height = img.height;
    cv.getContext('2d')!.putImageData(img, 0, 0);
    return cv.toDataURL('image/png');
  };
  Object.assign(window, { odinMeasure: gpu.measure, gpuInstances: gpu.lastInstances, compareGpu });
}
// ?backend=odin&font=...: 描画と計測を Odin (js_wasm32) に差し替える。入力と IME は canvas backend のまま
if (new URLSearchParams(location.search).get('backend') === 'odin') {
  if (!fontUrl) throw new Error('backend=odin には font= が要る');
  const { createOdinRaster } = await import('./backend/odin.ts');
  const odin = await createOdinRaster('raster.wasm', fontUrl);
  measureImpl = odin.measure;
  backend.draw = (ops) => odin.draw(ops, backend.colors(), backend.canvas);
  Object.assign(window, { odinMeasure: odin.measure });
}
// ?todos=1000&extra=7 で件数と窓の数を増やせる (性能の計測用。このときは保存も復元もしない)
const params = new URLSearchParams(location.search);
const perf = params.has('todos') || params.has('extra');
const perfTodo: AppManifest = { ...todoManifest, create: (id) => createTodoApp({ id, items: id === 'todo' ? Number(params.get('todos') ?? 3) : 20 }) };
/** 能力の許可・アプリの保存領域・セッションは localStorage に置く */
const KV_PREFIX = 'ai-native-desktop.kv.';
const localKeyValue: KeyValue = {
  get: (k) => {
    try {
      return localStorage.getItem(KV_PREFIX + k) ?? undefined;
    } catch {
      return undefined;
    }
  },
  set: (k, v) => {
    try {
      localStorage.setItem(KV_PREFIX + k, v);
    } catch {
      /* 保存できなくても動く */
    }
  },
  keys: () => {
    try {
      return Object.keys(localStorage)
        .filter((k) => k.startsWith(KV_PREFIX))
        .map((k) => k.slice(KV_PREFIX.length));
    } catch {
      return [];
    }
  },
};
// Terminal: libghostty-vt (ghostty-web の ghostty-vt.wasm) は最初の窓を開くときに読み込む
const terminal = terminalManifest(async () => {
  const { loadVtFactory } = await import('./apps/terminal/vt.ts');
  const res = await fetch('ghostty-vt.wasm');
  if (!res.ok) throw new Error(`ghostty-vt.wasm: ${res.status}`);
  return loadVtFactory(await res.arrayBuffer());
});
// 入れたパッケージ (app.json の URL) は、セッションを復元する前に読み込む (窓のアプリが入っていないと復元できない)
const PACKAGES_KEY = 'platform.packages';
// コンポーネントは Worker で動かす (応答しなければ止める)。?component-timeout=ms で操作の期限を変えられる (検証用)
let runner: import('./platform/packages.ts').ComponentRunner | null = null;
async function packageRunner() {
  if (runner) return runner;
  const { workerRunner } = await import('./platform/worker-runner.ts');
  const actionMs = Number(params.get('component-timeout') ?? 2000);
  runner = workerRunner(
    () => new Worker(new URL('platform/component-worker.js', location.href), { type: 'module' }),
    () => ({ grant: (app, cap) => platform.services.grant(app, cap) }),
    { actionMs },
  );
  return runner;
}
async function readPackage(url: string) {
  const { loadPackage } = await import('./platform/packages.ts');
  const { urlPackageSource } = await import('./platform/package-source.ts');
  const { hostPackageTools } = await import('./platform/package-tools.ts');
  const src = urlPackageSource(url);
  if (typeof src === 'string') return { ok: false as const, errors: [src] };
  return loadPackage(src, hostPackageTools, await packageRunner());
}
const packageUrls: string[] = (() => {
  try {
    // 手で書き換えられた・壊れた値でも起動できるように、文字列の配列のときだけ使う
    const v: unknown = perf ? [] : JSON.parse(localKeyValue.get(PACKAGES_KEY) ?? '[]');
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
  } catch {
    return [];
  }
})();
const packageManifests: AppManifest[] = [];
// 同梱のアプリと、先に読んだパッケージの id は使わせない (後から同じ id を返すようになったパッケージで、起動が止まらないように)
const takenIds = new Set([...builtinManifests.map((m) => m.id), terminal.id]);
// 起動の見張り: コンポーネントは UI スレッドで同期に動くので、止まらないコンポーネントは画面ごと固める (期限では止められない)。
// 読み込み中のパッケージを記録し、起動が終わったら消す。前回の起動が終わらなかったら、そのパッケージを隔離して読み込まない。
// (根本の対策は、コンポーネントを Worker で動かして止められるようにすること。PLATFORM.md)
const STARTUP_KEY = 'platform.packages.loading';
const QUARANTINE_KEY = 'platform.packages.quarantine';
const readList = (k: string): string[] => {
  try {
    const v: unknown = JSON.parse(localKeyValue.get(k) ?? '[]');
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
  } catch {
    return [];
  }
};
const quarantined = new Set([...readList(QUARANTINE_KEY), ...readList(STARTUP_KEY)]);
if (quarantined.size) localKeyValue.set(QUARANTINE_KEY, JSON.stringify([...quarantined]));
const packageWarnings: string[] = [];
// ?safe でパッケージを 1 つも読み込まずに起動する
const safeMode = params.has('safe');
for (const url of packageUrls) {
  if (safeMode) {
    packageWarnings.push(`安全モード (?safe): パッケージ ${url} を読み込まなかった`);
    continue;
  }
  if (quarantined.has(url)) {
    packageWarnings.push(`前回の起動が終わらなかったので、パッケージ ${url} を隔離した (読み込まない)`);
    continue;
  }
  localKeyValue.set(STARTUP_KEY, JSON.stringify([url]));
  const r = await readPackage(url);
  if (!r.ok) console.warn(`パッケージ ${url} を読めない: ${r.errors.join(' / ')}`);
  else if (takenIds.has(r.manifest.id)) console.warn(`パッケージ ${url} の id ${r.manifest.id} はすでに使われている。読み込まない`);
  else {
    takenIds.add(r.manifest.id);
    packageManifests.push(r.manifest);
  }
}
// 窓の復元でもコンポーネントを動かすので、読み込んだパッケージ全体を見張りに入れておく
localKeyValue.set(STARTUP_KEY, JSON.stringify(packageUrls.filter((u) => !quarantined.has(u) && !safeMode)));
const platform = createPlatform({
  apps: [...(perf ? builtinManifests.map((m) => (m.id === 'todo' ? perfTodo : m)) : builtinManifests), terminal, ...packageManifests],
  initial: ['todo', 'settings', 'notes', ...Array.from({ length: Number(params.get('extra') ?? 0) }, () => 'todo')],
  measure: (s, z, w) => measureImpl(s, z, w),
  templates: storage,
  kv: perf ? memoryKeyValue() : localKeyValue,
  // ?session=off で前回の窓を復元しない
  restoreSession: !perf && params.get('session') !== 'off',
});
shell = platform.shell;
// 窓・配置が変わったら、すぐにセッションを保存する (変わっていなければ書かない)
shell.onChange = () => {
  schedule();
  if (!perf) platform.saveSession();
};
platform.services.onChange = schedule;
platform.agentShell.onChange = schedule;
// ワークスペース: Alt+1 で人の、Alt+2 で agent のワークスペースを見る (どちらの窓も動き続ける)
let workspace: 1 | 2 = 1;
function switchWorkspace(n: number) {
  workspace = n === 2 ? 2 : 1;
  shell = workspace === 1 ? platform.shell : platform.agentShell;
  Object.assign(window, { shell });
  aria.announce(workspace === 1 ? '人のワークスペース' : 'agent のワークスペース');
  schedule();
}
for (const sh of [platform.shell, platform.agentShell]) sh.onWorkspaceKey = switchWorkspace;
// ?pty=ws://127.0.0.1:8140/?token=... で、Terminal から本物のシェルにつなげる (pty/server.ts)
const ptyUrl = params.get('pty');
if (ptyUrl) platform.services.pty = wsPtyConnector(ptyUrl);

// スクリーンリーダー向けのミラー DOM (意味ツリーから作る)
const aria = createAriaMirror(
  document.getElementById('a11y')!,
  (app, node) => {
    shell.activate(app, node);
    backend.focusInput();
  },
  (app) => shell.restoreApp(app, 'human'),
);
/** 読み上げた数は shell (ワークスペース) ごとに持つ。切り替えで飛ばしたり、読み直したりしないように */
const seen = new Map<Shell, { proposals: number; audit: number }>();
let seenPrompts = 0;
let seenNotes = 0;
/** AI の提案と、拒否された操作を読み上げる (承認の判断を音声でもできるように) */
function announce() {
  const sn = seen.get(shell) ?? seen.set(shell, { proposals: 0, audit: 0 }).get(shell)!;
  for (const p of shell.proposals.slice(sn.proposals)) aria.announce(`AI の提案 ${p.id}: ${p.intent}。${p.sensitive ? '承認が必要です。' : ''}`);
  sn.proposals = shell.proposals.length;
  const denied = shell.audit.slice(sn.audit).filter((a) => a.result === 'denied');
  if (denied.length) aria.announce(`拒否: ${denied.at(-1)!.reason ?? ''}`);
  sn.audit = shell.audit.length;
  const sv = platform.services;
  for (const p of sv.prompts.slice(seenPrompts)) aria.announce(`${p.appTitle} が ${capLabel(p.capability)} の許可を求めています`);
  seenPrompts = sv.prompts.length;
  const last = sv.notifications.at(-1);
  if (last && last.id > seenNotes) {
    aria.announce(`通知 ${last.title}: ${last.body ?? ''}`);
    seenNotes = last.id;
  }
}
const CAP_LABEL: Record<string, string> = {
  storage: '保存領域',
  'clipboard.read': 'クリップボードの読み取り',
  'clipboard.write': 'クリップボードへの書き込み',
  notifications: '通知',
  'apps.launch': 'アプリの起動',
  'terminal.pty': 'ホストのシェル (擬似端末)',
};
/** 能力の名前。ほかのアプリのデータ (data:<topic>) は、提供するアプリと中身を示す */
function capLabel(cap: string): string {
  if (!cap.startsWith('data:')) return CAP_LABEL[cap] ?? cap;
  const topic = cap.slice('data:'.length);
  const m = platform.registry.list({ includeSystem: true }).find((x) => x.provides?.[topic]);
  return m ? `${m.title} の「${m.provides![topic].description}」の読み取り` : `${topic} の読み取り`;
}

// ---- agent API: MCP の tool にそのまま写せる形で window に出す ----
// 人のワークスペースへは提案 (人が承認)、agent のワークスペースでは直接。持ち込みは提案 (人が承認)
const agent = {
  getTree: (app?: string) => platform.shell.getTree(app),
  getLayout: () => platform.shell.getLayout(),
  propose: (intent: string, steps: Step[]) => platform.shell.propose(intent, steps),
  proposals: () => platform.shell.proposals,
  workspace: {
    getTree: (app?: string) => platform.agentShell.getTree(app),
    windows: () => platform.agentShell.windows(),
    launch: (appId: string) => platform.agent.launch(appId),
    act: (app: string, target: string, action: ActionName, data?: string) => platform.agent.act(app, target, action, data),
    proposeMerge: (intent: string) => platform.services.proposeMerge(intent, 'agent'),
    merge: (id: string) => platform.services.merges.find((m) => m.id === id),
  },
};
// ---- veriss への lowering (e2e/veriss-diff.cjs が使う)。計測は lowering 先の CSS と同じ font で行う ----
const lowerCtx = document.createElement('canvas').getContext('2d')!;
const lowerMeasure: Measure = (s, size, weight) => {
  if (fontUrl) return measureImpl(s, size, weight);
  lowerCtx.font = `${weight} ${size}px ${LOWER_FONT}`;
  return lowerCtx.measureText(s).width;
};
const lowerFor = (app: string, pxIntrinsic = false) => {
  const { view } = shell.viewOf(app);
  return { ...lower(view, lowerMeasure, { pxIntrinsic, fontFamily: lowerFamily }), minWidth: layout(view, 1000, lowerMeasure).minWidth };
};
Object.assign(window, { shell, platform, agent, lowerFor, shellMeasure: (s: string, z: number, w: number) => measureImpl(s, z, w) });

// ?mcp=ws://127.0.0.1:8130 で、AI 向けの MCP サーバ (mcp/server.ts) とつなぐ
const mcpUrl = params.get('mcp');
if (mcpUrl)
  connectMcp(
    mcpUrl,
    platform.shell,
    (s) => Object.assign(window, { mcpStatus: s }),
    () => platform.registry.list().map((m) => ({ id: m.id, title: m.title, description: m.description, multiInstance: m.multiInstance, capabilities: m.capabilities })),
    agent.workspace,
  );

// ---- panel ----
const SAMPLE_PROPOSAL = {
  intent: '牛乳を完了にして、卵を追加する',
  steps: [
    { app: 'todo', target: 'i1', action: 'click' },
    { app: 'todo', target: 'input', action: 'setValue', data: '卵を買う' },
    { app: 'todo', target: 'add', action: 'click' },
  ],
};
const SAMPLE_LAYOUT = {
  intent: 'Todo を Notes とタブにまとめ、Settings を最小化して、残りを均等にする',
  steps: [
    { kind: 'layout', op: { type: 'move', app: 'todo', drop: { kind: 'tab', target: 'notes' } } },
    { kind: 'layout', op: { type: 'minimize', app: 'settings' } },
    { kind: 'layout', op: { type: 'equalize' } },
  ],
};
const SAMPLE_CARD: ViewTemplate = {
  type: 'box',
  axis: 'vertical',
  gap: 2,
  padding: 10,
  background: 'bg',
  border: 'border',
  radius: 8,
  children: [
    { type: 'text', text: { bind: 'count', format: '{} 件のタスク' }, size: 12, weight: 700, color: 'muted' },
    { type: 'each', item: { type: 'box', axis: 'horizontal', padding: 4, children: [{ type: 'self' }] } },
  ],
};
const SAMPLE_BAD: ViewTemplate = { type: 'box', axis: 'vertical', children: [{ type: 'text', text: { bind: 'label' }, size: 16 }] };

panel.innerHTML = `
  <div class="row"><label>テーマ <select id="theme"><option value="system">システムに従う</option><option value="light">ライト</option><option value="dark">ダーク</option></select></label></div>
  <div class="row" role="group" aria-label="ワークスペース">ワークスペース <button data-ws="1" id="ws1">人 (Alt+1)</button><button data-ws="2" id="ws2">agent (Alt+2)</button></div>
  <div id="merges"></div>
  <div id="prompts"></div>
  <div id="notices"></div>

  <h2>アプリ</h2>
  <div class="row" id="installed"></div>
  <div class="muted">F6 でデスクトップとこのパネルの間を移る。Alt+L でランチャー (それ自体も意味ツリーを出すアプリ)。窓はタイトルバーの × で閉じる。</div>
  <div id="openWins"></div>
  <div class="row"><label>パッケージ <input id="pkgUrl" value="packages/todo-rs/app.json" size="28" spellcheck="false" aria-label="パッケージ (app.json) の URL"></label><button id="pkgInstall">入れる</button></div>
  <div class="muted">WIT のコンポーネントのアプリ (app.json + .wasm)。glue はホストが .wasm から作り、パッケージの JS は動かさない。能力は import として渡す (宣言していない import を持つものは入らない)。</div>
  <div id="pkgErr" class="err" role="alert"></div>

  <h2>レイアウト</h2>
  <div class="row"><label><input type="checkbox" id="live"> ライブプレビュー (ほかの窓が場所を空ける)</label></div>
  <div class="muted">タイトルバーを掴んで運ぶ: 端で分割 / 中央で入れ替え / タイトルバーでタブ / 境界で差し込み / 外周で全体の端 / 下の受け口で最小化。
  ダブルクリック: タイトルバーでズーム、境界で均等。Alt+方向キーで隣と入れ替え、Alt+Enter ズーム、Alt+M 最小化、Alt+E 均等。</div>

  <h2>Agent の提案</h2>
  <div class="row"><label><input type="checkbox" id="auto"> sensitive でない提案は自動承認</label></div>
  <div id="proposals"></div>
  <textarea class="json" id="proposalJson" spellcheck="false"></textarea>
  <div class="row"><button class="primary" id="propose">提案する (agent として)</button><button id="sampleAction">例: 操作</button><button id="sampleLayout">例: レイアウト</button></div>
  <div id="proposeErr" class="err"></div>

  <h2>View Template (LLM の出力の代わり)</h2>
  <div class="row"><select id="sigSel"></select></div>
  <textarea class="json" id="tplJson" spellcheck="false"></textarea>
  <div class="row">
    <button id="sampleCard">サンプル: カード</button>
    <button id="sampleBad">サンプル: 不正 (操作を落とす)</button>
  </div>
  <div class="row">
    <button class="primary" id="applyTpl">適用 (ephemeral)</button>
    <button id="pinTpl">好み: pin</button>
    <button id="rejectTpl">reject</button>
    <button id="clearTpl">削除</button>
  </div>
  <div id="tplErr" class="err"></div>
  <div id="cache"></div>

  <h2>Window</h2>
  <div id="wins"></div>

  <h2>Semantic Tree (agent から見える形)</h2>
  <div class="row"><select id="treeApp"></select></div>
  <pre id="tree"></pre>

  <h2>監査ログ</h2>
  <pre id="audit"></pre>
`;
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
$<HTMLTextAreaElement>('proposalJson').value = JSON.stringify(SAMPLE_PROPOSAL, null, 2);
$<HTMLTextAreaElement>('tplJson').value = JSON.stringify(SAMPLE_CARD, null, 2);

$<HTMLInputElement>('live').addEventListener('change', (e) => {
  shell.previewMode = (e.target as HTMLInputElement).checked ? 'live' : 'overlay';
  backend.focusInput();
});
$('sampleAction').addEventListener('click', () => ($<HTMLTextAreaElement>('proposalJson').value = JSON.stringify(SAMPLE_PROPOSAL, null, 2)));
$('sampleLayout').addEventListener('click', () => ($<HTMLTextAreaElement>('proposalJson').value = JSON.stringify(SAMPLE_LAYOUT, null, 2)));
$<HTMLInputElement>('auto').addEventListener('change', (e) => {
  shell.policy.autoApproveSafe = (e.target as HTMLInputElement).checked;
});
$('propose').addEventListener('click', () => {
  $('proposeErr').textContent = '';
  try {
    const { intent, steps } = JSON.parse($<HTMLTextAreaElement>('proposalJson').value);
    const r = shell.propose(String(intent ?? ''), steps ?? []);
    if (!r.ok) $('proposeErr').textContent = r.errors.join('\n');
  } catch (e) {
    $('proposeErr').textContent = String(e);
  }
  schedule();
});
$('proposals').addEventListener('click', (e) => {
  const b = (e.target as HTMLElement).closest('button');
  if (!b?.dataset.id) return;
  if (b.dataset.op === 'approve') shell.approve(b.dataset.id);
  else shell.reject(b.dataset.id);
  backend.focusInput();
});

const sigSel = $<HTMLSelectElement>('sigSel');
const fillSigs = () => {
  const prev = sigSel.value;
  sigSel.innerHTML = openApps()
    .flatMap((app) => shell.signatures(app).map((s) => ({ app, ...s })))
    .filter((s) => s.role !== 'paragraph' && s.role !== 'heading')
    .map((s) => `<option value="${escapeHtml(s.sig)}">${escapeHtml(`${s.app}/${s.node}`)} (${s.role}${s.label ? ` "${escapeHtml(s.label)}"` : ''})</option>`)
    .join('');
  if ([...sigSel.options].some((o) => o.value === prev)) sigSel.value = prev;
  else sigSel.value = shell.signatures('todo').find((s) => s.node === 'list')?.sig ?? sigSel.options[0]?.value ?? '';
};
$('sampleCard').addEventListener('click', () => {
  $<HTMLTextAreaElement>('tplJson').value = JSON.stringify(SAMPLE_CARD, null, 2);
});
$('sampleBad').addEventListener('click', () => {
  $<HTMLTextAreaElement>('tplJson').value = JSON.stringify(SAMPLE_BAD, null, 2);
});
$('applyTpl').addEventListener('click', () => {
  $('tplErr').textContent = '';
  try {
    const r = shell.cache.propose(sigSel.value, JSON.parse($<HTMLTextAreaElement>('tplJson').value), 'human');
    if (!r.ok) $('tplErr').textContent = r.errors.join('\n');
  } catch (e) {
    $('tplErr').textContent = String(e);
  }
  schedule();
});
$('pinTpl').addEventListener('click', () => (shell.cache.setState(sigSel.value, 'pinned'), schedule()));
$('rejectTpl').addEventListener('click', () => (shell.cache.setState(sigSel.value, 'rejected'), schedule()));
$('clearTpl').addEventListener('click', () => (shell.cache.remove(sigSel.value), schedule()));
$('treeApp').addEventListener('change', () => schedule());
// 許可の要求・通知・アプリの起動と終了は、アプリから描けない場所 (ホストの UI) に出す
panel.addEventListener('click', (e) => {
  const b = (e.target as HTMLElement).closest('button');
  if (!b) return;
  const d = b.dataset;
  if (d.grant) platform.services.answer(d.grant, d.allow === '1', 'human');
  else if (d.dismiss) platform.services.dismiss(Number(d.dismiss));
  // 開く・閉じるは、今見ているワークスペースで
  else if (d.launch) shell.applyLayout({ type: 'launch', app: d.launch }, 'human');
  else if (d.closeWin) shell.applyLayout({ type: 'close', app: d.closeWin }, 'human');
  else if (d.ws) switchWorkspace(Number(d.ws));
  // 持ち込みに答える (人だけ)
  else if (d.merge) platform.services.answerMerge(d.merge, d.ok === '1', 'human');
  else return;
  schedule();
  backend.focusInput();
});
const openApps = () => shell.windows().map((w) => w.id);
// パッケージを入れる (人の操作だけ。agent / MCP には口が無い)
if (packageWarnings.length) $('pkgErr').textContent = packageWarnings.join('\n');
$('pkgInstall').addEventListener('click', async () => {
  const url = $<HTMLInputElement>('pkgUrl').value.trim();
  $('pkgErr').textContent = '読み込み中…';
  const r = await readPackage(url);
  const why = r.ok ? platform.install(r.manifest) : r.errors.join('\n');
  $('pkgErr').textContent = why;
  if (!why) {
    // 人が入れ直したら隔離を解く
    quarantined.delete(url);
    localKeyValue.set(QUARANTINE_KEY, JSON.stringify([...quarantined]));
    localKeyValue.set(PACKAGES_KEY, JSON.stringify([...new Set([...packageUrls, url])]));
    packageUrls.push(url);
    aria.announce(`${r.ok ? r.manifest.title : ''} を入れた`);
  }
  schedule();
});
// F6: デスクトップ (Canvas) とパネルの間でフォーカスを移す。Canvas の中の Tab はアプリの部品を巡るので、パネルへはこれで行く
window.addEventListener('keydown', (e) => {
  if (e.key !== 'F6') return;
  e.preventDefault();
  if (panel.contains(document.activeElement)) backend.focusInput();
  else panel.querySelector<HTMLElement>('button:not([disabled]), select, input, textarea')?.focus();
});
// テーマ: システムに従う / ライト / ダーク。<html data-theme> を切り替え、Canvas の backend は CSS 変数を読み直す
const THEME_KEY = 'platform.theme';
const applyTheme = (t: string) => {
  if (t === 'light' || t === 'dark') document.documentElement.dataset.theme = t;
  else delete document.documentElement.dataset.theme;
};
const themeSel = $<HTMLSelectElement>('theme');
themeSel.value = (perf ? null : localKeyValue.get(THEME_KEY)) ?? 'system';
applyTheme(themeSel.value);
themeSel.addEventListener('change', () => {
  applyTheme(themeSel.value);
  if (!perf) localKeyValue.set(THEME_KEY, themeSel.value);
  schedule();
  backend.focusInput();
});

function escapeHtml(s: string) {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);
}

function renderPanel(frame: ReturnType<Shell['frame']>) {
  const sv = platform.services;
  $('ws1').setAttribute('aria-pressed', String(workspace === 1));
  $('ws2').setAttribute('aria-pressed', String(workspace === 2));
  $('ws1').className = workspace === 1 ? 'primary' : '';
  $('ws2').className = workspace === 2 ? 'primary' : '';
  const shortV = (v: string | undefined) => escapeHtml(v === undefined ? '(なし)' : v.length > 60 ? `${v.slice(0, 60)}…` : v);
  const mergesHtml = sv.merges
    .filter((m) => m.status === 'pending')
    .map(
      (m) => `<div class="card sensitive" role="alertdialog" aria-label="持ち込みの提案"><div><b>持ち込みの提案 ${m.id}</b> (${m.by === 'agent' ? 'agent' : '人'}): ${escapeHtml(m.intent)}</div>
        <ol>${m.changes
          .map(
            (c) => `<li>${escapeHtml(c.app)} / ${escapeHtml(c.key)}: ${shortV(c.before)} → ${shortV(c.after)}
              <details><summary>全文</summary><div class="muted">前</div><pre>${escapeHtml(c.before ?? '(なし)')}</pre><div class="muted">後</div><pre>${escapeHtml(c.after)}</pre></details></li>`,
          )
          .join('')}</ol>
        <div class="row"><button class="primary" data-merge="${m.id}" data-ok="1">人のワークスペースに書く</button><button data-merge="${m.id}" data-ok="0">却下</button></div></div>`,
    )
    .join('');
  // 変わったときだけ書き換える (開いた「全文」やフォーカスを、ほかの更新で閉じない)
  if ($('merges').dataset.html !== mergesHtml) {
    $('merges').innerHTML = mergesHtml;
    $('merges').dataset.html = mergesHtml;
  }
  $('prompts').innerHTML = sv.prompts
    .filter((p) => p.status === 'pending')
    .map(
      (p) => `<div class="card sensitive" role="alertdialog" aria-label="許可の要求"><div><b>${escapeHtml(p.appTitle)}</b> が <b>${escapeHtml(capLabel(p.capability))}</b> の許可を求めている</div>
        <div class="row"><button class="primary" data-grant="${p.id}" data-allow="1">許可</button><button data-grant="${p.id}" data-allow="0">拒否</button></div></div>`,
    )
    .join('');
  $('notices').innerHTML = sv.notifications
    .slice(-3)
    .reverse()
    .map((n) => `<div class="card"><span class="tag">通知</span> <b>${escapeHtml(n.title)}</b> ${escapeHtml(n.body ?? '')} <span class="muted">(${escapeHtml(n.window)})</span> <button data-dismiss="${n.id}">閉じる</button></div>`)
    .join('');
  const installed = platform.registry.list();
  const html = installed.map((m) => `<button data-launch="${m.id}" title="${escapeHtml(m.description)}" ${platform.registry.has(m.id) ? '' : 'disabled'}>${escapeHtml(m.title)} を開く</button>`).join('');
  if ($('installed').innerHTML !== html) $('installed').innerHTML = html;
  const wins = shell.windows();
  $('openWins').innerHTML = wins
    .map((w) => `<div class="card"><b>${escapeHtml(w.title)}</b> <span class="muted">${w.id} (${platform.registry.appOf(w.id) ?? 'システム'})</span>${wins.length > 1 ? ` <button data-close-win="${w.id}">閉じる</button>` : ''}</div>`)
    .join('');
  const treeSel = $<HTMLSelectElement>('treeApp');
  const opts = wins.map((w) => `<option>${escapeHtml(w.id)}</option>`).join('');
  if (treeSel.dataset.opts !== opts) {
    const prev = treeSel.value;
    treeSel.innerHTML = opts;
    treeSel.dataset.opts = opts;
    treeSel.value = wins.some((w) => w.id === prev) ? prev : wins[0].id;
  }
  $('proposals').innerHTML =
    // 待っている提案はすべて出す (承認・却下の口を失わない)。済んだものは新しい 4 件だけ
    [...shell.proposals.filter((p) => p.status !== 'pending').slice(-4), ...shell.proposals.filter((p) => p.status === 'pending')]
      .sort((a, b) => shell.proposals.indexOf(b) - shell.proposals.indexOf(a))
      .map(
        (p) => `<div class="card ${p.sensitive ? 'sensitive' : ''}">
          <div><b>${p.id}</b> ${escapeHtml(p.intent)} <span class="tag">${p.status}</span>${p.sensitive ? ' <span class="tag">sensitive</span>' : ''}</div>
          <ol>${p.steps
            .map((s) =>
              isLayoutStep(s)
                ? `<li>layout: ${escapeHtml(JSON.stringify(s.op))}</li>`
                : // app・target はアプリが決めた id (信用しない)。承認の面にアプリの HTML を混ぜない
                  `<li>${escapeHtml(`${s.app}/${s.target} ${s.action}`)}${s.data !== undefined ? ` "${escapeHtml(s.data)}"` : ''}</li>`,
            )
            .join('')}</ol>
          ${p.status === 'pending' ? `<div class="row"><button class="primary" data-op="approve" data-id="${p.id}">承認して実行</button><button data-op="reject" data-id="${p.id}">却下</button></div>` : ''}
        </div>`,
      )
      .join('') || '<div class="muted">提案はまだありません</div>';

  fillSigs();
  $('cache').innerHTML = shell.cache
    .list()
    .map((e) => `<div class="card"><span class="tag">${e.state}</span> <span class="tag">veriss: ${e.verification}</span> <span class="muted">${escapeHtml(e.sig)}</span></div>`)
    .join('');
  $('wins').innerHTML = frame.windows
    .map(
      (w) => `<div class="card"><b>${escapeHtml(w.title)}</b> 幅 ${Math.floor(w.tile.w)}px / 最小 ${w.minWidth}px
        ${w.overflow.length ? '<span class="tag">はみ出し</span>' : ''}
        ${w.issues.map((i) => `<div class="err">template を既定に戻した: ${escapeHtml(i.reason)}</div>`).join('')}</div>`,
    )
    .join('');
  $('tree').textContent = JSON.stringify(shell.getTree($<HTMLSelectElement>('treeApp').value)[0], null, 1);
  $('audit').textContent = shell.audit
    .slice(-12)
    .reverse()
    .map((a) => `${a.result === 'ok' ? 'ok    ' : 'DENIED'} ${a.by.padEnd(5)} ${a.app}/${a.target} ${a.action}${a.data !== undefined ? ` "${a.data}"` : ''}${a.proposal ? ` (${a.proposal})` : ''}${a.reason ? ` — ${a.reason}` : ''}`)
    .join('\n');
}

function render() {
  dirty = false;
  const { w, h } = backend.size();
  const t0 = performance.now();
  const frame = shell.frame(w, h, t0);
  // 見えていないワークスペースにも画面の大きさを知らせる (agent がそこで窓を開くときの配置の検査に使う)
  (shell === platform.shell ? platform.agentShell : platform.shell).setSize(w, h);
  const t1 = performance.now();
  backend.draw(frame.ops);
  const t2 = performance.now();
  Object.assign(window, { lastFrame: { windows: frame.windows, animating: frame.animating, cursor: frame.cursor, shellMs: t1 - t0, drawMs: t2 - t1, ops: frame.ops.length } });
  backend.ime(frame.ime);
  backend.setCursor(frame.cursor);
  // パネルの DOM はアニメーション中は更新しない (重い)。止まったら更新する
  if (!frame.animating && !shell.windowDrag()?.active) {
    renderPanel(frame);
    aria.update(shell.accessibility());
    announce();
  }
  if (frame.animating) {
    dirty = true;
    requestAnimationFrame(render);
  } else if (frame.wakeAt !== undefined) {
    // 通知が消え始める時刻に描き直す (それまではフレームを回さない)
    clearTimeout(wakeTimer);
    wakeTimer = setTimeout(schedule, Math.max(0, frame.wakeAt - performance.now()));
  }
}
let wakeTimer: ReturnType<typeof setTimeout> | undefined;
render();
backend.focusInput();
// 最初のフレームまで来たので、起動の見張りを外す (窓の復元でパッケージの窓も作り終えている)
localKeyValue.set(STARTUP_KEY, '[]');
