import { createSettingsApp } from './apps/settings.ts';
import { createNotesApp } from './apps/notes.ts';
import { createTodoApp } from './apps/todo.ts';
import { createCanvasBackend } from './backend/canvas.ts';
import type { CacheEntry, Storage } from './cache.ts';
import type { ViewTemplate } from './catalog.ts';
import type { Measure } from './protocol.ts';
import { layout } from './layout.ts';
import { LOWER_FONT, lower } from './lower.ts';
import { Shell, isLayoutStep } from './shell.ts';
import type { Step } from './shell.ts';

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
  input: (m) => shell.input(m),
  resize: () => schedule(),
});
measureImpl = backend.measure;
shell = new Shell([createTodoApp(), createSettingsApp(), createNotesApp()], (s, z, w) => measureImpl(s, z, w), storage);
shell.onChange = schedule;

// ---- agent API: MCP の tool にそのまま写せる形で window に出す ----
const agent = {
  getTree: (app?: string) => shell.getTree(app),
  getLayout: () => shell.getLayout(),
  propose: (intent: string, steps: Step[]) => shell.propose(intent, steps),
  proposals: () => shell.proposals,
};
// ---- veriss への lowering (e2e/veriss-diff.cjs が使う)。計測は lowering 先の CSS と同じ font で行う ----
const lowerCtx = document.createElement('canvas').getContext('2d')!;
const lowerMeasure: Measure = (s, size, weight) => {
  lowerCtx.font = `${weight} ${size}px ${LOWER_FONT}`;
  return lowerCtx.measureText(s).width;
};
const lowerFor = (app: string, pxIntrinsic = false) => {
  const { view } = shell.viewOf(app);
  return { ...lower(view, lowerMeasure, { pxIntrinsic }), minWidth: layout(view, 1000, lowerMeasure).minWidth };
};
Object.assign(window, { shell, agent, lowerFor });

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
  <div class="row"><select id="treeApp"><option>todo</option><option>settings</option><option>notes</option></select></div>
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
  sigSel.innerHTML = ['todo', 'settings', 'notes']
    .flatMap((app) => shell.signatures(app).map((s) => ({ app, ...s })))
    .filter((s) => s.role !== 'paragraph' && s.role !== 'heading')
    .map((s) => `<option value="${escapeHtml(s.sig)}">${s.app}/${s.node} (${s.role}${s.label ? ` "${escapeHtml(s.label)}"` : ''})</option>`)
    .join('');
  if ([...sigSel.options].some((o) => o.value === prev)) sigSel.value = prev;
  else sigSel.value = shell.signatures('todo').find((s) => s.node === 'list')!.sig;
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

function escapeHtml(s: string) {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);
}

function renderPanel(frame: ReturnType<Shell['frame']>) {
  $('proposals').innerHTML =
    shell.proposals
      .slice(-4)
      .reverse()
      .map(
        (p) => `<div class="card ${p.sensitive ? 'sensitive' : ''}">
          <div><b>${p.id}</b> ${escapeHtml(p.intent)} <span class="tag">${p.status}</span>${p.sensitive ? ' <span class="tag">sensitive</span>' : ''}</div>
          <ol>${p.steps
            .map((s) =>
              isLayoutStep(s)
                ? `<li>layout: ${escapeHtml(JSON.stringify(s.op))}</li>`
                : `<li>${s.app}/${s.target} ${s.action}${s.data !== undefined ? ` "${escapeHtml(s.data)}"` : ''}</li>`,
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
  const frame = shell.frame(w, h, performance.now());
  Object.assign(window, { lastFrame: { windows: frame.windows, animating: frame.animating, cursor: frame.cursor } });
  backend.draw(frame.ops);
  backend.ime(frame.ime);
  backend.setCursor(frame.cursor);
  // パネルの DOM はアニメーション中は更新しない (重い)。止まったら更新する
  if (!frame.animating && !shell.windowDrag()?.active) renderPanel(frame);
  if (frame.animating) {
    dirty = true;
    requestAnimationFrame(render);
  }
}
render();
backend.focusInput();
