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
import { AnimatedRect, PREVIEW_SPRING, Spring, TILE_SPRING } from './anim.ts';
import { paint } from './render.ts';
import type { HitRegion } from './render.ts';
import { signature } from './signature.ts';
import { SemanticTree } from './tree.ts';
import { applyDrop, computeRects, describeDrop, hitDrop, leaf, leaves, setRatio } from './tiling.ts';
import type { Divider, Drop, TileNode } from './tiling.ts';

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
  /** まだ動いている (次のフレームを要求する) */
  animating: boolean;
  cursor: 'default' | 'grab' | 'grabbing' | 'col-resize' | 'row-resize' | 'pointer' | 'not-allowed';
}

/** 窓のドラッグ中の状態 */
export interface WindowDrag {
  app: AppId;
  x: number;
  y: number;
  /** しきい値 (5px) を超えて動いたら true */
  active: boolean;
  drop: Drop | null;
  /** 最小幅を下回る分割は受け付けない */
  valid: boolean;
  reason: string;
  label: string;
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
  /** タイルの分割ツリー */
  tiles: TileNode;
  /** false にするとアニメーションせず、すぐに目標の位置へ (テスト用) */
  animations = true;
  onChange: () => void = () => {};

  private wins: Win[] = [];
  private focusedWin = 0;
  private preedit = '';
  private hits: { app: AppId; hit: HitRegion }[] = [];
  private dividers: Divider[] = [];
  private dividerDrag: Divider | null = null;
  private drag: (WindowDrag & { sx: number; sy: number }) | null = null;
  private hover = { x: -1, y: -1 };
  private titles = new Map<AppId, Rect>();
  private area: Rect = { x: 0, y: 0, w: 0, h: 0 };
  private minWidths = new Map<AppId, number>();
  private anims = new Map<AppId, AnimatedRect>();
  private preview: { rect: AnimatedRect; alpha: Spring; last: Rect | null; valid: boolean; label: string } | null = null;
  private lastNow = 0;
  private lastSize = '';
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
    const ids = apps.map((a) => a.id);
    // 既定: 左に 1 つ目、右を上下に分ける
    this.tiles =
      ids.length >= 3
        ? { kind: 'split', dir: 'row', ratio: 0.5, a: leaf(ids[0]), b: { kind: 'split', dir: 'column', ratio: 0.5, a: leaf(ids[1]), b: leaf(ids[2]) } }
        : ids.length === 2
          ? { kind: 'split', dir: 'row', ratio: 0.5, a: leaf(ids[0]), b: leaf(ids[1]) }
          : leaf(ids[0]);
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
    const div = this.dividers.find((d) => inside(grow(d.rect, 3), x, y));
    if (div) {
      this.dividerDrag = div;
      return;
    }
    // タイトルバーを掴む: 少し動かすまではフォーカスの移動だけ
    for (const [app, r] of this.titles) {
      if (!inside(r, x, y)) continue;
      this.focusedWin = this.wins.findIndex((w) => w.app.id === app);
      this.drag = { app, x, y, sx: x, sy: y, active: false, drop: null, valid: false, reason: '', label: '' };
      this.onChange();
      return;
    }
    const h = [...this.hits].reverse().find((e) => inside(e.hit.rect, x, y));
    if (!h) return;
    this.focusedWin = this.wins.findIndex((w) => w.app.id === h.app);
    this.preedit = '';
    this.dispatch({ app: h.app, target: h.hit.node, action: 'focus', by: 'human' });
    if (h.hit.widget !== 'textField') this.dispatch({ app: h.app, target: h.hit.node, action: 'click', by: 'human' });
  }

  pointerMove(x: number, y: number): void {
    this.hover = { x, y };
    if (this.dividerDrag) {
      const d = this.dividerDrag;
      const r = d.dir === 'row' ? (x - d.area.x) / d.area.w : (y - d.area.y) / d.area.h;
      this.tiles = setRatio(this.tiles, d.path, Math.min(0.85, Math.max(0.15, r)));
      this.snapTiles = true;
      this.onChange();
      return;
    }
    const g = this.drag;
    if (g) {
      g.x = x;
      g.y = y;
      if (!g.active && Math.hypot(x - g.sx, y - g.sy) > 5) g.active = true;
      if (g.active) this.updateDrop();
    }
    this.onChange();
  }

  pointerUp(): void {
    this.dividerDrag = null;
    const g = this.drag;
    this.drag = null;
    if (g?.active && g.drop && g.valid) {
      const next = applyDrop(this.tiles, g.app, g.drop);
      if (next) this.tiles = next;
    }
    if (this.preview) this.preview.alpha.target = 0;
    this.onChange();
  }

  /** ドラッグ中の窓 (テストと UI 用) */
  windowDrag(): WindowDrag | null {
    if (!this.drag) return null;
    const { sx: _sx, sy: _sy, ...rest } = this.drag;
    return rest;
  }

  private snapTiles = false;

  private updateDrop() {
    const g = this.drag!;
    const hit = hitDrop(this.tiles, this.area, GAP, g.app, g.x, g.y);
    g.drop = hit?.drop ?? null;
    g.valid = false;
    g.reason = '';
    g.label = '';
    if (!hit) return;
    g.label = describeDrop(hit.drop);
    const next = applyDrop(this.tiles, g.app, hit.drop);
    if (!next) {
      g.drop = null;
      return;
    }
    // 分割後に、どの窓も検証済みの最小幅を下回らないこと (veriss で証明する範囲と同じ下限)
    const { tiles } = computeRects(next, this.area, GAP);
    for (const [app, r] of tiles) {
      const min = this.minWidths.get(app) ?? 0;
      if (r.w < min) {
        g.reason = `${this.titleOf(app)} が最小幅 ${min}px を下回る`;
        break;
      }
      if (r.h - TITLE_H < 80) {
        g.reason = `${this.titleOf(app)} の高さが足りない`;
        break;
      }
    }
    g.valid = g.reason === '';
    const p = (this.preview ??= { rect: new AnimatedRect(hit.preview, PREVIEW_SPRING), alpha: new Spring(0, PREVIEW_SPRING), last: null, valid: true, label: '' });
    if (p.alpha.x < 0.02) p.rect = new AnimatedRect(shrinkFrom(hit.preview, g.x, g.y), PREVIEW_SPRING); // 現れるときはポインタの位置から広がる
    p.rect.target = hit.preview;
    p.alpha.target = 1;
    p.last = hit.preview;
    p.valid = g.valid;
    p.label = g.valid ? g.label : g.reason;
  }

  private titleOf(app: AppId): string {
    const t = this.win(app).tree;
    return (t.root && t.get(t.root)?.label) || app;
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
        if (this.drag) {
          this.drag = null;
          if (this.preview) this.preview.alpha.target = 0;
        }
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
  frame(width: number, height: number, now: number = Date.now()): Frame {
    const dt = this.lastNow ? Math.min(0.064, Math.max(0, (now - this.lastNow) / 1000)) : 0;
    this.lastNow = now;
    const size = `${width}x${height}`;
    const resized = size !== this.lastSize;
    this.lastSize = size;
    this.area = { x: GAP, y: GAP, w: width - 2 * GAP, h: height - 2 * GAP };

    const ops: DrawOp[] = [['rect', { x: 0, y: 0, w: width, h: height, fill: 'bg' }]];
    this.hits = [];
    this.titles.clear();
    const windows: WindowInfo[] = [];
    let ime: ImeToHost = { t: 'ime-disable' };
    let animating = false;

    const { tiles: targets, dividers } = computeRects(this.tiles, this.area, GAP);
    this.dividers = dividers;
    const snap = !this.animations || resized || this.snapTiles;
    this.snapTiles = false;

    const ghosts = new Map<string, Map<NodeId, number>>();
    for (const p of this.proposals.filter((p) => p.status === 'pending'))
      p.steps.forEach((s, i) => {
        if (!ghosts.has(s.app)) ghosts.set(s.app, new Map());
        ghosts.get(s.app)!.set(s.target, i + 1);
      });

    const dragging = this.drag?.active ? this.drag.app : null;
    const order = leaves(this.tiles);
    // ドラッグ中の窓は最後に描く (下に隠れない)
    const paintOrder = dragging ? [...order.filter((a) => a !== dragging), dragging] : order;
    let dragTitle = '';
    for (const app of paintOrder) {
      const w = this.win(app);
      const target = targets.get(app)!;
      let a = this.anims.get(app);
      if (!a) this.anims.set(app, (a = new AnimatedRect(target, TILE_SPRING)));
      a.target = target;
      if (snap) a.snap();
      const settled = a.step(dt);
      animating ||= !settled;
      const tile = a.value;
      const i = this.wins.indexOf(w);
      const { view, issues } = expandTree(w.tree, this.source);
      const content = { x: tile.x, y: tile.y + TITLE_H, w: tile.w, h: tile.h - TITLE_H };
      const lo = layout(view, content.w, this.measure, content);
      this.minWidths.set(app, lo.minWidth);
      const focused = i === this.focusedWin;
      const pt = paint(lo.root, { focus: focused ? w.tree.focus : null, preedit: focused ? this.preedit : '', ghosts: ghosts.get(app) ?? new Map() }, this.measure);
      const title = this.titleOf(app);
      if (app === dragging) dragTitle = title;

      const dim = app === dragging;
      if (dim) ops.push(['layer', { alpha: 0.4 }]);
      ops.push(['rect', { ...tile, radius: 10, fill: 'surface', stroke: focused ? 'accent' : 'border', lineWidth: focused ? 2 : 1 }]);
      ops.push(['clip', tile]);
      ops.push(['text', { x: tile.x + 28, y: tile.y + TITLE_H / 2, str: title, size: 12, weight: 600, color: 'muted' }]);
      // つまみ (⠿) : タイトルバーを掴んで動かせることを示す
      for (let r = 0; r < 3; r++)
        for (let c = 0; c < 2; c++) ops.push(['rect', { x: tile.x + 12 + c * 4, y: tile.y + 10 + r * 4, w: 2, h: 2, radius: 1, fill: 'muted' }]);
      if (lo.overflow.length && settled) {
        const long = `はみ出し: 最小 ${lo.minWidth}px > 幅 ${Math.floor(content.w)}px`;
        const fits = this.measure(long, 11, 600) + 12 + 80 < tile.w;
        const msg = fits ? long : `${lo.minWidth}px 必要`;
        const mw = this.measure(msg, 11, 600) + 12;
        ops.push(['rect', { x: tile.x + tile.w - mw - 8, y: tile.y + 6, w: mw, h: 18, radius: 9, fill: 'danger' }]);
        ops.push(['text', { x: tile.x + tile.w - mw - 2, y: tile.y + 15, str: msg, size: 11, weight: 600, color: 'accentText' }]);
      }
      ops.push(['pop']);
      ops.push(['clip', content]);
      ops.push(...pt.ops);
      ops.push(['pop']);
      if (dim) ops.push(['pop']);
      this.titles.set(app, { x: tile.x, y: tile.y, w: tile.w, h: TITLE_H });
      if (!dim) for (const hit of pt.hits) this.hits.push({ app, hit });
      if (focused && pt.caret && w.tree.focus) {
        const f = w.tree.get(w.tree.focus);
        ime = { t: 'ime-enable', node: w.tree.focus, surrounding: f?.value ?? '', cursor: [...(f?.value ?? '')].length, caret: pt.caret };
      }
      windows.push({ app, title, tile, minWidth: lo.minWidth, overflow: settled ? lo.overflow : [], issues });
    }
    for (const d of dividers) {
      const c = d.dir === 'row' ? { x: d.rect.x + GAP / 2 - 1, y: d.rect.y + d.rect.h / 2 - 20, w: 2, h: 40 } : { x: d.rect.x + d.rect.w / 2 - 20, y: d.rect.y + GAP / 2 - 1, w: 40, h: 2 };
      ops.push(['rect', { ...c, radius: 1, fill: 'border' }]);
    }

    // ドロップ先のプレビュー (青。最小幅を下回る分割は赤)
    const p = this.preview;
    if (p) {
      if (!this.drag?.active || !this.drag.drop) p.alpha.target = 0;
      if (snap && !this.animations) {
        p.rect.snap();
        p.alpha.snap();
      }
      const s1 = p.rect.step(dt);
      const s2 = p.alpha.step(dt);
      animating ||= !(s1 && s2);
      const alpha = Math.max(0, Math.min(1, p.alpha.x));
      if (alpha > 0.01) {
        const r = p.rect.value;
        const color = p.valid ? 'accent' : 'danger';
        ops.push(['layer', { alpha: 0.22 * alpha }]);
        ops.push(['rect', { ...r, radius: 10, fill: color }]);
        ops.push(['pop']);
        ops.push(['layer', { alpha }]);
        ops.push(['rect', { ...r, radius: 10, stroke: color, lineWidth: 2 }]);
        if (p.label && r.w > 60) {
          const lw = this.measure(p.label, 12, 600) + 20;
          // ラベルは canvas の内側に収める
          const lx = Math.max(this.area.x + 4, Math.min(r.x + r.w / 2 - lw / 2, this.area.x + this.area.w - lw - 4));
          const ly = r.y + r.h / 2 - 13;
          ops.push(['rect', { x: lx, y: ly, w: lw, h: 26, radius: 13, fill: color }]);
          ops.push(['text', { x: lx + 10, y: ly + 13, str: p.label, size: 12, weight: 600, color: 'accentText' }]);
        }
        ops.push(['pop']);
      } else if (!this.drag) this.preview = null;
    }

    // ポインタに付いてくる窓の札
    if (this.drag?.active) {
      const g = this.drag;
      const w = Math.min(220, this.measure(dragTitle, 12, 600) + 24);
      // 札は canvas の内側に収める (右端・下端ではポインタの左上に回す)
      const bx = g.x + 10 + w > width - 4 ? g.x - 10 - w : g.x + 10;
      const by = g.y + 12 + 30 > height - 4 ? g.y - 12 - 30 : g.y + 12;
      ops.push(['layer', { alpha: 0.95 }]);
      ops.push(['rect', { x: bx, y: by, w, h: 30, radius: 8, fill: 'surface', stroke: g.drop && !g.valid ? 'danger' : 'accent', lineWidth: 1.5 }]);
      ops.push(['text', { x: bx + 12, y: by + 15, str: dragTitle, size: 12, weight: 600, color: 'text' }]);
      ops.push(['pop']);
    }

    let cursor: Frame['cursor'] = 'default';
    const hv = this.hover;
    const div = this.dividerDrag ?? this.dividers.find((d) => inside(grow(d.rect, 3), hv.x, hv.y));
    if (this.drag?.active) cursor = this.drag.drop && !this.drag.valid ? 'not-allowed' : 'grabbing';
    else if (div) cursor = div.dir === 'row' ? 'col-resize' : 'row-resize';
    else if ([...this.titles.values()].some((r) => inside(r, hv.x, hv.y))) cursor = 'grab';
    else if (this.hits.some((h) => inside(h.hit.rect, hv.x, hv.y))) cursor = 'pointer';

    return { ops, ime, windows, animating, cursor };
  }
}

function grow(r: Rect, d: number): Rect {
  return { x: r.x - d, y: r.y - d, w: r.w + 2 * d, h: r.h + 2 * d };
}

/** プレビューが現れるとき、ポインタの位置の小さな矩形から広げる */
function shrinkFrom(_r: Rect, x: number, y: number): Rect {
  return { x: x - 20, y: y - 20, w: 40, h: 40 };
}

function inside(r: Rect, x: number, y: number) {
  return x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h;
}
