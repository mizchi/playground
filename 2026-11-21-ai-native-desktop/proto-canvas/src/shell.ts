// Shell: 入力の振り分け (actor 付き)、ポリシー、提案と承認、タイル配置、描画のまとめ。DOM に依存しない。
import type { App } from './apps/app.ts';
import { TemplateCache } from './cache.ts';
import type { Storage } from './cache.ts';
import { defaultTemplate } from './catalog.ts';
import { expandTree } from './expand.ts';
import type { ExpandIssue, TemplateSource, ViewNode } from './expand.ts';
import { layout } from './layout.ts';
import type { Overflow } from './layout.ts';
import type { ActionName, ActionRequest, Actor, AppId, DrawOp, ImeFromHost, ImeToHost, Measure, NodeId, Rect, SemanticNode, TreeUpdate } from './protocol.ts';
import { paint } from './render.ts';
import type { HitRegion } from './render.ts';
import { signature } from './signature.ts';
import { SemanticTree } from './tree.ts';

export interface Step {
  app: AppId;
  target: NodeId;
  action: ActionName;
  data?: string;
}

export interface Proposal {
  id: string;
  intent: string;
  steps: Step[];
  status: 'pending' | 'approved' | 'rejected' | 'done';
  /** sensitive なノードに触れる (自動承認しない) */
  sensitive: boolean;
}

export interface AuditEntry {
  at: number;
  by: Actor;
  app: AppId;
  target: NodeId;
  action: ActionName;
  data?: string;
  proposal?: string;
  result: 'ok' | 'denied';
  reason?: string;
}

export interface WindowInfo {
  app: AppId;
  title: string;
  tile: Rect;
  minWidth: number;
  overflow: Overflow[];
  issues: ExpandIssue[];
}

export interface Frame {
  ops: DrawOp[];
  ime: ImeToHost;
  windows: WindowInfo[];
}

interface Win {
  app: App;
  tree: SemanticTree;
}

const TITLE_H = 30;
const GAP = 8;

export class Shell {
  readonly cache: TemplateCache;
  readonly audit: AuditEntry[] = [];
  readonly proposals: Proposal[] = [];
  policy = { autoApproveSafe: false };
  /** 2 つのタイルの左の割合 */
  split = 0.5;
  onChange: () => void = () => {};

  private wins: Win[] = [];
  private focusedWin = 0;
  private preedit = '';
  private hits: { app: AppId; hit: HitRegion }[] = [];
  private divider: Rect | null = null;
  private dragging = false;
  private seq = 1;
  private measure: Measure;

  constructor(apps: App[], measure: Measure, storage: Storage) {
    this.measure = measure;
    this.cache = new TemplateCache(storage);
    for (const app of apps) {
      const w: Win = { app, tree: new SemanticTree() };
      this.wins.push(w);
      app.start((u) => this.receive(w, u));
    }
  }

  // ---- P1 ----
  private receive(w: Win, u: TreeUpdate) {
    if (u.app !== w.app.id) throw new Error(`app ${w.app.id} sent update as ${u.app}`);
    w.tree.apply(u);
    this.onChange();
  }

  tree(app: AppId): SemanticTree {
    return this.win(app).tree;
  }

  private win(app: AppId): Win {
    const w = this.wins.find((x) => x.app.id === app);
    if (!w) throw new Error(`no app ${app}`);
    return w;
  }

  // ---- P2: すべての操作はここを通る ----
  dispatch(req: Omit<ActionRequest, 't'>): AuditEntry {
    const entry: AuditEntry = { at: Date.now(), by: req.by, app: req.app, target: req.target, action: req.action, data: req.data, proposal: req.proposal, result: 'ok' };
    const deny = (reason: string) => {
      entry.result = 'denied';
      entry.reason = reason;
      this.audit.push(entry);
      this.onChange();
      return entry;
    };
    const w = this.wins.find((x) => x.app.id === req.app);
    if (!w) return deny('no such app');
    const node = w.tree.get(req.target);
    if (!node) return deny('no such node');
    if (node.disabled) return deny('disabled');
    // view や agent が何を言っても、アプリが公開した action 以外は起こせない
    if (!node.actions?.includes(req.action)) return deny(`${node.role} は ${req.action} を公開していない`);
    if (req.by === 'agent') {
      const p = this.proposals.find((x) => x.id === req.proposal);
      if (!p || p.status !== 'approved') return deny('承認された提案がない');
      if (!p.steps.some((s) => s.app === req.app && s.target === req.target && s.action === req.action && s.data === req.data))
        return deny('提案に含まれない操作');
    }
    this.audit.push(entry);
    w.app.onAction(structuredClone({ t: 'action', ...req }));
    return entry;
  }

  // ---- 人間の入力 (意味的な入力に変換する) ----
  pointerDown(x: number, y: number): void {
    if (this.divider && inside(this.divider, x, y)) {
      this.dragging = true;
      return;
    }
    const h = [...this.hits].reverse().find((e) => inside(e.hit.rect, x, y));
    if (!h) return;
    this.focusedWin = this.wins.findIndex((w) => w.app.id === h.app);
    this.preedit = '';
    this.dispatch({ app: h.app, target: h.hit.node, action: 'focus', by: 'human' });
    if (h.hit.widget !== 'textField') this.dispatch({ app: h.app, target: h.hit.node, action: 'click', by: 'human' });
  }

  pointerMove(x: number, width: number): void {
    if (!this.dragging) return;
    this.split = Math.min(0.85, Math.max(0.15, x / width));
    this.onChange();
  }

  pointerUp(): void {
    this.dragging = false;
  }

  input(msg: ImeFromHost): void {
    const w = this.wins[this.focusedWin];
    const focus = w.tree.focus ? w.tree.get(w.tree.focus) : undefined;
    const app = w.app.id;
    if (msg.t === 'ime-preedit') {
      if (focus?.role === 'textbox') this.preedit = msg.text;
      this.onChange();
      return;
    }
    if (msg.t === 'ime-commit') {
      this.preedit = '';
      // 確定した文字列だけを action にする
      if (focus?.role === 'textbox' && msg.text) this.dispatch({ app, target: focus.id, action: 'replaceSelectedText', data: msg.text, by: 'human' });
      else this.onChange();
      return;
    }
    switch (msg.key) {
      case 'Tab':
      case 'ShiftTab':
        this.moveFocus(msg.key === 'Tab' ? 1 : -1);
        break;
      case 'Space':
      case 'Enter':
        if (focus && focus.role !== 'textbox') this.dispatch({ app, target: focus.id, action: 'click', by: 'human' });
        break;
      case 'Backspace':
        if (focus?.role === 'textbox' && !this.preedit)
          this.dispatch({ app, target: focus.id, action: 'setValue', data: [...(focus.value ?? '')].slice(0, -1).join(''), by: 'human' });
        break;
      case 'Escape':
        this.preedit = '';
        this.onChange();
        break;
    }
  }

  /** フォーカス順は木の順。窓をまたいで巡回する */
  private moveFocus(dir: 1 | -1) {
    const order: { app: AppId; node: SemanticNode }[] = this.wins.flatMap((w) =>
      w.tree.actionable().filter((n) => n.actions?.includes('focus')).map((node) => ({ app: w.app.id, node })),
    );
    if (order.length === 0) return;
    const w = this.wins[this.focusedWin];
    const cur = order.findIndex((o) => o.app === w.app.id && o.node.id === w.tree.focus);
    const next = order[(cur + dir + order.length) % order.length];
    this.focusedWin = this.wins.findIndex((x) => x.app.id === next.app);
    this.preedit = '';
    this.dispatch({ app: next.app, target: next.node.id, action: 'focus', by: 'human' });
  }

  // ---- agent API (MCP の tool にそのまま写す) ----
  /** sensitive なノードの value は伏せる */
  getTree(app?: AppId): { app: AppId; root: NodeId | null; nodes: SemanticNode[] }[] {
    return this.wins
      .filter((w) => !app || w.app.id === app)
      .map((w) => ({
        app: w.app.id,
        root: w.tree.root,
        nodes: w.tree.walk().map((n) => (n.sensitive && n.value !== undefined ? { ...n, value: '[redacted]' } : structuredClone(n))),
      }));
  }

  propose(intent: string, steps: Step[]): { ok: true; proposal: Proposal } | { ok: false; errors: string[] } {
    const errors: string[] = [];
    let sensitive = false;
    steps.forEach((s, i) => {
      const w = this.wins.find((x) => x.app.id === s.app);
      const n = w?.tree.get(s.target);
      if (!n) return errors.push(`steps[${i}]: ${s.app}/${s.target} がない`);
      if (s.action === 'focus') return errors.push(`steps[${i}]: agent は focus を使わない (人間のフォーカスを奪わない)`);
      if (!n.actions?.includes(s.action)) return errors.push(`steps[${i}]: ${n.role} "${n.label}" は ${s.action} を公開していない`);
      if (n.sensitive) sensitive = true;
    });
    if (steps.length === 0) errors.push('steps が空');
    if (errors.length) return { ok: false, errors };
    const p: Proposal = { id: `p${this.seq++}`, intent, steps: structuredClone(steps), status: 'pending', sensitive };
    this.proposals.push(p);
    if (this.policy.autoApproveSafe && !sensitive) this.approve(p.id);
    else this.onChange();
    return { ok: true, proposal: p };
  }

  approve(id: string): void {
    const p = this.proposals.find((x) => x.id === id);
    if (!p || p.status !== 'pending') return;
    p.status = 'approved';
    for (const s of p.steps) this.dispatch({ ...s, by: 'agent', proposal: p.id });
    p.status = 'done';
    this.onChange();
  }

  reject(id: string): void {
    const p = this.proposals.find((x) => x.id === id);
    if (!p || p.status !== 'pending') return;
    p.status = 'rejected';
    this.onChange();
  }

  /** template を作る対象: 各ノードの構造シグネチャ */
  signatures(app: AppId): { node: NodeId; role: string; label?: string; sig: string }[] {
    const t = this.tree(app);
    return t.walk().map((n) => ({ node: n.id, role: n.role, label: n.label, sig: signature(t, n) }));
  }

  /** 展開済みの View Tree (veriss への lowering や検査用) */
  viewOf(app: AppId): { view: ViewNode; issues: ExpandIssue[] } {
    return expandTree(this.tree(app), this.source);
  }

  private source: TemplateSource = (node, sig) => {
    const e = this.cache.lookup(sig);
    return e ? { template: e.template, from: 'cache' } : { template: defaultTemplate(node), from: 'default' };
  };

  // ---- 描画 ----
  frame(width: number, height: number): Frame {
    const ops: DrawOp[] = [['rect', { x: 0, y: 0, w: width, h: height, fill: 'bg' }]];
    this.hits = [];
    const windows: WindowInfo[] = [];
    let ime: ImeToHost = { t: 'ime-disable' };
    const n = this.wins.length;
    const ratios = n === 2 ? [this.split, 1 - this.split] : this.wins.map(() => 1 / n);
    const avail = width - GAP * (n + 1);
    let x = GAP;
    const ghosts = new Map<string, Map<NodeId, number>>();
    for (const p of this.proposals.filter((p) => p.status === 'pending'))
      p.steps.forEach((s, i) => {
        if (!ghosts.has(s.app)) ghosts.set(s.app, new Map());
        ghosts.get(s.app)!.set(s.target, i + 1);
      });

    this.wins.forEach((w, i) => {
      const tile: Rect = { x, y: GAP, w: avail * ratios[i], h: height - 2 * GAP };
      x += tile.w + GAP;
      const { view, issues } = expandTree(w.tree, this.source);
      const content = { x: tile.x, y: tile.y + TITLE_H, w: tile.w, h: tile.h - TITLE_H };
      const lo = layout(view, content.w, this.measure, content);
      const focused = i === this.focusedWin;
      const pt = paint(lo.root, { focus: focused ? w.tree.focus : null, preedit: focused ? this.preedit : '', ghosts: ghosts.get(w.app.id) ?? new Map() }, this.measure);
      const root = w.tree.root ? w.tree.get(w.tree.root) : undefined;
      const title = root?.label ?? w.app.id;

      ops.push(['rect', { ...tile, radius: 10, fill: 'surface', stroke: focused ? 'accent' : 'border', lineWidth: focused ? 2 : 1 }]);
      ops.push(['text', { x: tile.x + 12, y: tile.y + TITLE_H / 2, str: title, size: 12, weight: 600, color: 'muted' }]);
      if (lo.overflow.length) {
        const long = `はみ出し: 最小 ${lo.minWidth}px > 幅 ${Math.floor(content.w)}px`;
        const fits = this.measure(long, 11, 600) + 12 + 80 < tile.w;
        const msg = fits ? long : `${lo.minWidth}px 必要`;
        const mw = this.measure(msg, 11, 600) + 12;
        ops.push(['rect', { x: tile.x + tile.w - mw - 8, y: tile.y + 6, w: mw, h: 18, radius: 9, fill: 'danger' }]);
        ops.push(['text', { x: tile.x + tile.w - mw - 2, y: tile.y + 15, str: msg, size: 11, weight: 600, color: 'accentText' }]);
      }
      ops.push(['clip', content]);
      ops.push(...pt.ops);
      ops.push(['pop']);
      for (const hit of pt.hits) this.hits.push({ app: w.app.id, hit });
      if (focused && pt.caret && w.tree.focus) {
        const f = w.tree.get(w.tree.focus);
        ime = { t: 'ime-enable', node: w.tree.focus, surrounding: f?.value ?? '', cursor: [...(f?.value ?? '')].length, caret: pt.caret };
      }
      windows.push({ app: w.app.id, title, tile, minWidth: lo.minWidth, overflow: lo.overflow, issues });
    });
    this.divider = n === 2 ? { x: GAP + avail * ratios[0], y: GAP, w: GAP, h: height - 2 * GAP } : null;
    if (this.divider) ops.push(['rect', { x: this.divider.x + GAP / 2 - 1, y: height / 2 - 20, w: 2, h: 40, radius: 1, fill: 'border' }]);
    return { ops, ime, windows };
  }
}

function inside(r: Rect, x: number, y: number) {
  return x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h;
}
