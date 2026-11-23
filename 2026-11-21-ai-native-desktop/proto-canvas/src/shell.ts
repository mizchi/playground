// Shell: 入力の振り分け (actor 付き)、ポリシー、提案と承認、タイル配置、描画のまとめ。DOM に依存しない。
import type { App } from './apps/app.ts';
import { TemplateCache } from './cache.ts';
import type { Storage } from './cache.ts';
import { defaultTemplate } from './catalog.ts';
import { expandTree } from './expand.ts';
import type { ExpandIssue, TemplateSource, ViewNode } from './expand.ts';
import { M, layout } from './layout.ts';
import type { LaidOut, Overflow } from './layout.ts';
import type { ActionName, ActionRequest, Actor, AppId, DrawOp, ImeFromHost, ImeToHost, Measure, NodeId, Rect, SemanticNode, TreeUpdate } from './protocol.ts';
import { AnimatedRect, PREVIEW_SPRING, Spring, TILE_SPRING } from './anim.ts';
import { paint } from './render.ts';
import type { HitRegion } from './render.ts';
import { signature } from './signature.ts';
import { SemanticTree } from './tree.ts';
import { activateTab, addToLargest, allApps, applyDrop, computeRects, describeDrop, equalize, groupOf, hitDrop, leaf, leaves, neighbor, removeApp, setRatio } from './tiling.ts';
import type { Divider, Drop, Edge, TileNode } from './tiling.ts';

/** アプリへの操作 */
export interface ActionStep {
  kind?: 'action';
  app: AppId;
  target: NodeId;
  action: ActionName;
  data?: string;
}

/** レイアウトの操作 (agent は人のドラッグを真似ず、意味のある操作として提案する) */
export type LayoutOp =
  | { type: 'move'; app: AppId; drop: Exclude<Drop, { kind: 'minimize' }> }
  | { type: 'zoom'; app: AppId | null }
  | { type: 'minimize'; app: AppId }
  | { type: 'restore'; app: AppId }
  | { type: 'equalize' }
  /** インストール済みのアプリを新しい窓で開く (窓の工場が要る) */
  | { type: 'launch'; app: string }
  /** 窓を閉じる */
  | { type: 'close'; app: AppId };

/** 窓を作る工場。shell はインストール済みのアプリを知らないので、プラットフォームが差し込む */
export interface WindowFactory {
  /** 次に開く窓の id (提案のプレビューにも使う) */
  nextId(appId: string): AppId;
  create(appId: string, windowId: AppId): App;
  has(appId: string): boolean;
  title(appId: string): string;
}

export interface LayoutStep {
  kind: 'layout';
  op: LayoutOp;
}

export type Step = ActionStep | LayoutStep;
export const isLayoutStep = (s: Step): s is LayoutStep => s.kind === 'layout';

interface LayoutState {
  tiles: TileNode;
  minimized: AppId[];
  zoomed: AppId | null;
}

export interface Proposal {
  id: string;
  intent: string;
  steps: Step[];
  /** stale: 提案のあとに前提が変わったので実行しなかった */
  status: 'pending' | 'approved' | 'rejected' | 'done' | 'stale';
  /** sensitive なノードに触れる (自動承認しない) */
  sensitive: boolean;
  /** 提案した時点の前提。承認するときに 1 つでも変わっていたら実行しない */
  preconditions: { app: AppId; target: NodeId; state: NodeState }[];
  /** レイアウトの手順を含むとき、提案した時点の配置 */
  layoutBefore?: string;
}

/** 前提として比べるノードの状態 (操作の意味を左右するもの) */
type NodeState = Pick<SemanticNode, 'role' | 'label' | 'value' | 'checked' | 'disabled'>;
const nodeState = (n: SemanticNode): NodeState => ({ role: n.role, label: n.label, value: n.value, checked: n.checked, disabled: n.disabled });

export interface AuditEntry {
  at: number;
  by: Actor;
  app: AppId;
  target: NodeId;
  action: ActionName | 'layout';
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
  /** ドックに入っている窓 (木から外れている) */
  minimized: AppId[] = [];
  /** 全体に広げている窓 */
  zoomed: AppId | null = null;
  /** overlay: 落とす先を青い矩形で示す / live: ほかの窓が実際に場所を空けて見せる */
  previewMode: 'overlay' | 'live' = 'overlay';
  /** false にするとアニメーションせず、すぐに目標の位置へ (テスト用) */
  animations = true;
  onChange: () => void = () => {};
  /** 窓の工場 (launch に要る) */
  factory: WindowFactory | null = null;
  /** 窓を閉じたとき (プラットフォームが後片付けに使う) */
  onWindowClosed: (id: AppId) => void = () => {};

  private wins: Win[] = [];
  private closeRects = new Map<AppId, Rect>();
  /** launch の提案のプレビューで使う、まだ無い窓の名前 */
  private pendingTitles = new Map<AppId, string>();
  private focusedWin = 0;
  private preedit = '';
  private hits: { app: AppId; hit: HitRegion }[] = [];
  private dividers: Divider[] = [];
  private dividerDrag: Divider | null = null;
  private drag: (WindowDrag & { sx: number; sy: number; fromTab: boolean; next: TileNode | null }) | null = null;
  private hover = { x: -1, y: -1 };
  private titles = new Map<AppId, Rect>();
  private tabRects: { app: AppId; rect: Rect }[] = [];
  private chips: { app: AppId; rect: Rect }[] = [];
  private size = { w: 0, h: 0 };
  private area: Rect = { x: 0, y: 0, w: 0, h: 0 };
  private minWidths = new Map<AppId, number>();
  private anims = new Map<AppId, AnimatedRect>();
  private leaving = new Map<AppId, { rect: AnimatedRect; alpha: Spring }>();
  private preview: { rect: AnimatedRect; alpha: Spring; valid: boolean; label: string } | null = null;
  private pill = new Spring(0, PREVIEW_SPRING);
  private lastNow = 0;
  private lastSize = '';
  private snapTiles = false;
  /** 窓ごとのスクロール量 (ばねで滑らかに動かす) と、その上限 */
  private scrolls = new Map<AppId, Spring>();
  private scrollMax = new Map<AppId, number>();
  private tileRects = new Map<AppId, Rect>();
  /** 次のフレームで、フォーカスした要素を見える位置まで送る */
  private revealFocus = false;
  private seq = 1;
  private measure: Measure;
  /** 端末の画面に最後に知らせた桁と行 (窓/ノード → "colsxrows") */
  private gridSent = new Map<string, string>();

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
    // 4 つ目以降は、一番大きいペーンを分けて足す (初期の画面の大きさは仮に 1400x800)
    for (const id of ids.slice(3)) this.tiles = addToLargest(this.tiles, id, { x: 0, y: 0, w: 1400, h: 800 }, GAP);
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
      if (!p.steps.some((s) => !isLayoutStep(s) && s.app === req.app && s.target === req.target && s.action === req.action && s.data === req.data))
        return deny('提案に含まれない操作');
    }
    this.audit.push(entry);
    w.app.onAction(structuredClone({ t: 'action', ...req }));
    return entry;
  }

  // ---- 人間の入力 (意味的な入力に変換する) ----
  pointerDown(x: number, y: number): void {
    const chip = this.chips.find((c) => inside(c.rect, x, y));
    if (chip) {
      this.restoreApp(chip.app, 'human');
      return;
    }
    // 閉じるボタン (×)
    for (const [app, r] of this.closeRects) {
      if (!inside(r, x, y)) continue;
      this.applyLayout({ type: 'close', app }, 'human');
      return;
    }
    const div = this.zoomed ? undefined : this.dividers.find((d) => inside(grow(d.rect, 3), x, y));
    if (div) {
      this.dividerDrag = div;
      return;
    }
    // タブ / タイトルバーを掴む: 少し動かすまではフォーカスの移動 (タブは切り替え) だけ
    const tab = this.tabRects.find((t) => inside(t.rect, x, y));
    const title = tab ? null : [...this.titles].find(([, r]) => inside(r, x, y));
    const app = tab?.app ?? title?.[0];
    if (app) {
      this.focusApp(app);
      this.drag = { app, x, y, sx: x, sy: y, active: false, drop: null, valid: false, reason: '', label: '', fromTab: !!tab, next: null };
      this.onChange();
      return;
    }
    const h = [...this.hits].reverse().find((e) => inside(e.hit.rect, x, y));
    if (!h) return;
    this.focusApp(h.app);
    this.preedit = '';
    this.dispatch({ app: h.app, target: h.hit.node, action: 'focus', by: 'human' });
    if (h.hit.action) this.dispatch({ app: h.app, target: h.hit.node, action: h.hit.action, by: 'human' });
    else if (h.hit.widget !== 'textField') this.dispatch({ app: h.app, target: h.hit.node, action: 'click', by: 'human' });
  }

  /** タイトルバーのダブルクリックでズーム、境界のダブルクリックで均等にする */
  doubleClick(x: number, y: number): void {
    const div = this.dividers.find((d) => inside(grow(d.rect, 3), x, y));
    if (div) {
      const node = nodeAtPath(this.tiles, div.path);
      if (node) this.tiles = setRatio(this.tiles, div.path, (equalize(node) as Extract<TileNode, { kind: 'split' }>).ratio);
      this.onChange();
      return;
    }
    const title = [...this.titles].find(([, r]) => inside(r, x, y));
    if (title) this.applyLayout({ type: 'zoom', app: this.zoomed === title[0] ? null : title[0] }, 'human');
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
      if (!g.active && Math.hypot(x - g.sx, y - g.sy) > 5) {
        g.active = true;
        if (this.zoomed) this.zoomed = null; // ズーム中に掴んだら、元に戻してから運ぶ
      }
      if (g.active) this.updateDrop();
    }
    this.onChange();
  }

  /** ホイール: ポインタの下の窓をスクロールする */
  wheel(x: number, y: number, dy: number): void {
    for (const [app, r] of this.tileRects) {
      if (!inside(r, x, y)) continue;
      const sp = this.scrollOf(app);
      sp.target = Math.max(0, Math.min(this.scrollMax.get(app) ?? 0, sp.target + dy));
      if (!this.animations) sp.snap();
      this.onChange();
      return;
    }
  }

  private scrollOf(app: AppId): Spring {
    let sp = this.scrolls.get(app);
    if (!sp) this.scrolls.set(app, (sp = new Spring(0, PREVIEW_SPRING)));
    return sp;
  }

  pointerUp(): void {
    this.dividerDrag = null;
    const g = this.drag;
    this.drag = null;
    if (g && !g.active && g.fromTab) this.tiles = activateTab(this.tiles, g.app);
    if (g?.active && g.drop && g.valid) {
      const from = this.preview?.rect.value;
      if (g.drop.kind === 'minimize') this.minimizeApp(g.app, 'human');
      else if (g.next) {
        this.tiles = g.next;
        // 運んだ窓は、プレビューの位置から本来の位置へ広がる
        if (from && g.drop.kind !== 'tab') this.anims.set(g.app, new AnimatedRect(from, TILE_SPRING));
        if (g.drop.kind === 'tab') this.anims.delete(g.app);
        this.focusApp(g.app);
        this.logLayout('human', `move ${g.app}: ${describeDrop(g.drop)}`, g.app, 'ok');
      }
    }
    if (this.preview) this.preview.alpha.target = 0;
    this.pill.target = 0;
    this.onChange();
  }

  /** ドラッグ中の窓 (テストと UI 用) */
  windowDrag(): WindowDrag | null {
    if (!this.drag) return null;
    const { sx: _sx, sy: _sy, fromTab: _t, next: _n, ...rest } = this.drag;
    return rest;
  }

  private dockRect(count: number): Rect | null {
    return count > 0 ? { x: GAP, y: this.size.h - GAP - DOCK_H, w: this.size.w - 2 * GAP, h: DOCK_H } : null;
  }

  private areaFor(minimizedCount: number): Rect {
    const dock = count(minimizedCount);
    return { x: GAP, y: GAP, w: this.size.w - 2 * GAP, h: this.size.h - 2 * GAP - dock };
    function count(n: number) {
      return n > 0 ? DOCK_H + GAP : 0;
    }
  }

  /** ドラッグ中に出る「最小化」の受け口 (ドックがあればドックそのもの) */
  private dropDock(): Rect {
    return (
      this.dockRect(this.minimized.length) ?? { x: this.size.w / 2 - 90, y: this.area.y + this.area.h - 36 - 40, w: 180, h: 36 }
    );
  }

  private updateDrop() {
    const g = this.drag!;
    const hit = hitDrop(this.tiles, this.area, GAP, g.app, g.x, g.y, { titleH: TITLE_H, dock: this.dropDock() });
    this.pill.target = 1;
    g.drop = hit?.drop ?? null;
    g.next = null;
    g.valid = false;
    g.reason = '';
    g.label = '';
    if (!hit) return;
    g.label = describeDrop(hit.drop);
    let preview = hit.preview;
    if (hit.drop.kind === 'minimize') {
      g.reason = removeApp(this.tiles, g.app) ? this.invalidReason(removeApp(this.tiles, g.app)!, this.minimized.length + 1) : '最後の窓は最小化できない';
    } else {
      const next = applyDrop(this.tiles, g.app, hit.drop);
      if (!next) {
        g.drop = null;
        return;
      }
      g.next = next;
      g.reason = this.invalidReason(next, this.minimized.length);
      // 差し込みとライブプレビューでは、落とした後の実際の位置をプレビューにする
      if (hit.drop.kind === 'insert' || (this.previewMode === 'live' && hit.drop.kind !== 'tab')) preview = computeRects(next, this.area, GAP).tiles.get(g.app) ?? preview;
    }
    g.valid = g.reason === '';
    const p = (this.preview ??= { rect: new AnimatedRect(preview, PREVIEW_SPRING), alpha: new Spring(0, PREVIEW_SPRING), valid: true, label: '' });
    if (p.alpha.x < 0.02) p.rect = new AnimatedRect({ x: g.x - 20, y: g.y - 20, w: 40, h: 40 }, PREVIEW_SPRING); // 現れるときはポインタの位置から広がる
    p.rect.target = preview;
    p.alpha.target = 1;
    p.valid = g.valid;
    // 最小化は受け口そのものが状態を示すので、プレビューにラベルを重ねない
    p.label = hit.drop.kind === 'minimize' ? '' : g.valid ? g.label : g.reason;
  }

  /** 分割後に、どの窓も検証済みの最小幅 (veriss で証明する範囲と同じ下限) と最低限の高さを保てるか */
  private invalidReason(next: TileNode, minimizedCount: number): string {
    const { tiles } = computeRects(next, this.areaFor(minimizedCount), GAP);
    for (const [app, r] of tiles) {
      const min = this.minWidths.get(app) ?? 0;
      if (r.w < min) return `${this.titleOf(app)} が最小幅 ${min}px を下回る`;
      if (r.h - TITLE_H < MIN_CONTENT_H) return `${this.titleOf(app)} の高さが足りない`;
    }
    return '';
  }

  private titleOf(app: AppId): string {
    const w = this.wins.find((x) => x.app.id === app);
    if (!w) return this.pendingTitles.get(app) ?? app;
    const t = w.tree;
    return (t.root && t.get(t.root)?.label) || app;
  }

  // ---- 窓の追加と削除 (プラットフォームのアプリの起動と終了) ----
  /** 開いている窓 (表示中・タブの中・ドックの中のすべて) */
  windows(): { id: AppId; title: string }[] {
    return this.wins.map((w) => ({ id: w.app.id, title: this.titleOf(w.app.id) }));
  }

  private addWindow(app: App, from?: Rect) {
    if (this.wins.some((w) => w.app.id === app.id)) throw new Error(`窓 ${app.id} はすでにある`);
    const w: Win = { app, tree: new SemanticTree() };
    this.wins.push(w);
    app.start((u) => this.receive(w, u));
    if (from) this.anims.set(app.id, new AnimatedRect(from, TILE_SPRING));
  }

  private removeWindow(id: AppId) {
    const focused = this.wins[this.focusedWin]?.app.id;
    const i = this.wins.findIndex((w) => w.app.id === id);
    if (i < 0) return;
    const [w] = this.wins.splice(i, 1);
    w.app.stop?.();
    this.onWindowClosed(id);
    const cur = this.anims.get(id)?.value;
    // 閉じた窓は、その場で縮みながら消える
    if (cur) this.leaving.set(id, { rect: new AnimatedRect(cur, TILE_SPRING), alpha: new Spring(1, TILE_SPRING) });
    for (const m of [this.anims, this.minWidths, this.scrolls, this.scrollMax, this.tileRects, this.titles, this.closeRects]) m.delete(id);
    this.focusedWin = Math.max(0, this.wins.findIndex((x) => x.app.id === (focused === id ? leaves(this.tiles)[0] : focused)));
  }

  private focusApp(app: AppId) {
    this.focusedWin = this.wins.findIndex((w) => w.app.id === app);
  }

  // ---- レイアウトの操作 (人のキー操作・ドラッグと、agent の提案の両方がここを通る) ----
  private logLayout(by: Actor, what: string, app: AppId, result: 'ok' | 'denied', reason?: string, proposal?: string) {
    this.audit.push({ at: Date.now(), by, app, target: what, action: 'layout', result, reason, proposal });
  }

  /** 状態に操作を当てた結果 (検証込み)。失敗なら理由の文字列 */
  private simulate(st: LayoutState, op: LayoutOp): LayoutState | string {
    const visible = (a: AppId) => allApps(st.tiles).includes(a);
    switch (op.type) {
      case 'move': {
        if (!visible(op.app)) return `${op.app} は配置されていない`;
        const next = applyDrop(st.tiles, op.app, op.drop);
        if (!next) return `${op.app} をそこへは動かせない`;
        const why = this.invalidReason(next, st.minimized.length);
        return why || { ...st, tiles: next };
      }
      case 'zoom':
        if (op.app && !leaves(st.tiles).includes(op.app)) return `${op.app} は表示されていない`;
        return { ...st, zoomed: op.app };
      case 'minimize': {
        if (!visible(op.app)) return `${op.app} は配置されていない`;
        const next = removeApp(st.tiles, op.app);
        if (!next) return '最後の窓は最小化できない';
        const why = this.invalidReason(next, st.minimized.length + 1);
        return why || { tiles: next, minimized: [...st.minimized, op.app], zoomed: st.zoomed === op.app ? null : st.zoomed };
      }
      case 'restore': {
        if (!st.minimized.includes(op.app)) return `${op.app} はドックにない`;
        const rest = st.minimized.filter((a) => a !== op.app);
        const area = this.areaFor(rest.length);
        let next = addToLargest(st.tiles, op.app, area, GAP);
        if (this.invalidReason(next, rest.length)) {
          // 分けると狭すぎるなら、一番大きいペーンのタブに入れる
          const { tiles } = computeRects(st.tiles, area, GAP);
          const largest = [...tiles].sort((p, q) => q[1].w * q[1].h - p[1].w * p[1].h)[0][0];
          const g = groupOf(st.tiles, largest)!;
          next = mapGroup(st.tiles, g.app, (l) => ({ kind: 'leaf', app: op.app, tabs: [...(l.tabs ?? [l.app]), op.app] }));
        }
        return { ...st, tiles: next, minimized: rest };
      }
      case 'equalize':
        return { ...st, tiles: equalize(st.tiles) };
      case 'launch': {
        if (!this.factory) return 'アプリを起動できない (窓の工場が無い)';
        if (!this.factory.has(op.app)) return `アプリ ${op.app} を開けない (インストールされていないか、1 つの窓だけのアプリですでに開いている)`;
        const id = this.factory.nextId(op.app);
        this.pendingTitles.set(id, this.factory.title(op.app));
        const next = addToLargest(st.tiles, id, this.areaFor(st.minimized.length), GAP);
        const why = this.invalidReason(next, st.minimized.length);
        return why || { ...st, tiles: next };
      }
      case 'close': {
        if (!visible(op.app) && !st.minimized.includes(op.app)) return `${op.app} は開いていない`;
        const next = removeApp(st.tiles, op.app) ?? (st.minimized.includes(op.app) ? st.tiles : null);
        if (!next || (allApps(next).length === 0 && !st.minimized.some((a) => a !== op.app))) return '最後の窓は閉じられない';
        return { tiles: next, minimized: st.minimized.filter((a) => a !== op.app), zoomed: st.zoomed === op.app ? null : st.zoomed };
      }
    }
  }

  private state(): LayoutState {
    return { tiles: this.tiles, minimized: [...this.minimized], zoomed: this.zoomed };
  }

  /** 操作を当てる。成功なら ''、失敗なら理由 */
  applyLayout(op: LayoutOp, by: Actor, proposal?: string): string {
    const before = this.state();
    const r = this.simulate(before, op);
    const app = 'app' in op && op.app ? op.app : '*';
    const what = describeOp(op);
    if (typeof r === 'string') {
      this.logLayout(by, what, app, 'denied', r, proposal);
      this.onChange();
      return r;
    }
    // 最小化: 窓はドックの札に縮みながら消える
    if (op.type === 'minimize') {
      const cur = this.anims.get(op.app)?.value;
      if (cur) this.leaving.set(op.app, { rect: new AnimatedRect(cur, TILE_SPRING), alpha: new Spring(1, TILE_SPRING) });
      this.anims.delete(op.app);
    }
    // 起動: 窓を作り、画面の中央から広がる
    if (op.type === 'launch') {
      const id = this.factory!.nextId(op.app);
      const fin = computeRects(r.tiles, this.areaFor(r.minimized.length), GAP).tiles.get(id);
      const from = fin ? { x: fin.x + fin.w / 2 - 20, y: fin.y + fin.h / 2 - 20, w: 40, h: 40 } : undefined;
      this.addWindow(this.factory!.create(op.app, id), from);
      this.pendingTitles.delete(id);
      this.tiles = r.tiles;
      this.minimized = r.minimized;
      this.zoomed = r.zoomed;
      this.focusApp(id);
      this.logLayout(by, `launch ${op.app} → ${id}`, id, 'ok', undefined, proposal);
      this.onChange();
      return '';
    }
    if (op.type === 'close') {
      this.tiles = r.tiles;
      this.minimized = r.minimized;
      this.zoomed = r.zoomed;
      this.removeWindow(op.app);
      this.logLayout(by, what, op.app, 'ok', undefined, proposal);
      this.onChange();
      return '';
    }
    // 元に戻す: ドックの札の位置から広がる
    if (op.type === 'restore') {
      const chip = this.chips.find((c) => c.app === op.app)?.rect;
      if (chip) this.anims.set(op.app, new AnimatedRect(chip, TILE_SPRING));
    }
    this.tiles = r.tiles;
    this.minimized = r.minimized;
    this.zoomed = r.zoomed;
    if (op.type === 'restore' || op.type === 'zoom' && op.app) this.focusApp((op as { app: AppId }).app);
    const focused = this.wins[this.focusedWin]?.app.id;
    if (focused && !leaves(this.tiles).includes(focused)) this.focusApp(leaves(this.tiles)[0]);
    this.logLayout(by, what, app, 'ok', undefined, proposal);
    this.onChange();
    return '';
  }

  minimizeApp(app: AppId, by: Actor = 'human'): string {
    return this.applyLayout({ type: 'minimize', app }, by);
  }

  restoreApp(app: AppId, by: Actor = 'human'): string {
    return this.applyLayout({ type: 'restore', app }, by);
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
    const dirs: Record<string, Edge> = { MoveLeft: 'left', MoveRight: 'right', MoveUp: 'top', MoveDown: 'bottom' };
    switch (msg.key) {
      case 'Tab':
      case 'ShiftTab':
        this.moveFocus(msg.key === 'Tab' ? 1 : -1);
        break;
      case 'Space':
      case 'Enter':
        if (focus && focus.role !== 'textbox') this.dispatch({ app, target: focus.id, action: 'click', by: 'human' });
        // 入力欄が click (送信) を公開していれば、Enter で送信する (ターミナルのコマンド行など)
        else if (msg.key === 'Enter' && focus?.role === 'textbox' && focus.actions?.includes('click') && !this.preedit)
          this.dispatch({ app, target: focus.id, action: 'click', by: 'human' });
        break;
      case 'Launcher': {
        // ランチャー (システムのアプリ) を開く。開いていればそこへフォーカスする
        const open = this.wins.find((x) => x.app.id === 'launcher');
        if (open) this.focusApp('launcher');
        else this.applyLayout({ type: 'launch', app: 'launcher' }, 'human');
        this.onChange();
        break;
      }
      case 'Backspace':
        if (focus?.role === 'textbox' && !this.preedit)
          this.dispatch({ app, target: focus.id, action: 'setValue', data: [...(focus.value ?? '')].slice(0, -1).join(''), by: 'human' });
        break;
      case 'Escape':
        this.preedit = '';
        if (this.drag) {
          this.drag = null;
          if (this.preview) this.preview.alpha.target = 0;
          this.pill.target = 0;
        } else if (this.zoomed) this.zoomed = null;
        this.onChange();
        break;
      case 'MoveLeft':
      case 'MoveRight':
      case 'MoveUp':
      case 'MoveDown': {
        // AeroSpace の move のように、向きの隣の窓と入れ替える
        if (this.zoomed) break;
        const other = neighbor(this.tiles, this.area, GAP, app, dirs[msg.key]);
        if (other) this.applyLayout({ type: 'move', app, drop: { kind: 'swap', target: other } }, 'human');
        break;
      }
      case 'Zoom':
        this.applyLayout({ type: 'zoom', app: this.zoomed === app ? null : app }, 'human');
        break;
      case 'Minimize':
        this.minimizeApp(app, 'human');
        break;
      case 'Equalize':
        this.applyLayout({ type: 'equalize' }, 'human');
        break;
    }
  }

  /** フォーカス順は木の順。表示中の窓をまたいで巡回する */
  private moveFocus(dir: 1 | -1) {
    const shown = leaves(this.tiles);
    const order: { app: AppId; node: SemanticNode }[] = this.wins
      .filter((w) => shown.includes(w.app.id))
      .flatMap((w) => w.tree.actionable().filter((n) => n.actions?.includes('focus')).map((node) => ({ app: w.app.id, node })));
    if (order.length === 0) return;
    const w = this.wins[this.focusedWin];
    const cur = order.findIndex((o) => o.app === w.app.id && o.node.id === w.tree.focus);
    // フォーカスが無いときは、Tab で先頭、Shift+Tab で末尾へ
    const next = cur === -1 ? order[dir === 1 ? 0 : order.length - 1] : order[(cur + dir + order.length) % order.length];
    this.focusApp(next.app);
    this.preedit = '';
    this.revealFocus = true;
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
        // ターミナルの描画用の格子は渡さない (文字は value にある)
        nodes: w.tree.walk().map(({ screen: _screen, ...n }) => (n.sensitive && n.value !== undefined ? { ...n, value: '[redacted]' } : structuredClone(n))),
      }));
  }

  /** スクリーンリーダー向けのミラー DOM の材料: 表示中の窓のツリー、widget の位置、ドック、フォーカス */
  accessibility(): {
    windows: { app: AppId; title: string; root: NodeId | null; nodes: SemanticNode[]; focused: boolean; rect: Rect | null }[];
    rects: Map<string, Rect>;
    minimized: { app: AppId; title: string; rect: Rect | null }[];
  } {
    const shown = this.zoomed ? [this.zoomed] : leaves(this.tiles);
    const focusedApp = this.wins[this.focusedWin]?.app.id;
    return {
      windows: shown.map((app) => {
        const t = this.tree(app);
        return { app, title: this.titleOf(app), root: t.root, nodes: t.walk().map((n) => structuredClone(n)), focused: app === focusedApp, rect: this.tileRects.get(app) ?? null };
      }),
      rects: new Map(this.hits.map((h) => [`${h.app}/${h.hit.node}`, h.hit.rect])),
      minimized: this.minimized.map((app) => ({ app, title: this.titleOf(app), rect: this.chips.find((c) => c.app === app)?.rect ?? null })),
    };
  }

  /** スクリーンリーダーの操作 (ミラー DOM の click) を、人の意味的な操作にする */
  activate(app: AppId, node: NodeId): void {
    const n = this.tree(app).get(node);
    if (!n) return;
    this.focusApp(app);
    this.preedit = '';
    if (n.actions?.includes('focus')) this.dispatch({ app, target: node, action: 'focus', by: 'human' });
    if (n.role !== 'textbox' && n.actions?.includes('click')) this.dispatch({ app, target: node, action: 'click', by: 'human' });
  }

  /** agent から見えるレイアウト (木・ドック・ズーム) */
  getLayout(): LayoutState {
    return structuredClone(this.state());
  }

  propose(intent: string, steps: Step[]): { ok: true; proposal: Proposal } | { ok: false; errors: string[] } {
    const errors: string[] = [];
    let sensitive = false;
    let st = this.state();
    // 外 (MCP・パネルの JSON) から来るので、形を確かめてから使う
    if (typeof intent !== 'string') errors.push('intent は文字列');
    if (!Array.isArray(steps)) return { ok: false, errors: [...errors, 'steps は配列'] };
    const shapeErrors = steps.map((s, i) => stepShapeError(s, i)).filter((e): e is string => !!e);
    if (shapeErrors.length) return { ok: false, errors: [...errors, ...shapeErrors] };
    steps.forEach((s, i) => {
      if (isLayoutStep(s)) {
        // 窓を閉じると作業中の状態を失いうるので、自動承認しない
        if (s.op.type === 'close') sensitive = true;
        const r = this.simulate(st, s.op);
        if (typeof r === 'string') return errors.push(`steps[${i}]: ${r}`);
        st = r;
        return;
      }
      const w = this.wins.find((x) => x.app.id === s.app);
      const n = w?.tree.get(s.target);
      if (!n) return errors.push(`steps[${i}]: ${s.app}/${s.target} がない`);
      if (s.action === 'focus') return errors.push(`steps[${i}]: agent は focus を使わない (人間のフォーカスを奪わない)`);
      if (!n.actions?.includes(s.action)) return errors.push(`steps[${i}]: ${n.role} "${n.label}" は ${s.action} を公開していない`);
      if (n.sensitive) sensitive = true;
    });
    if (steps.length === 0) errors.push('steps が空');
    if (errors.length) return { ok: false, errors };
    const preconditions = steps.filter((s): s is ActionStep => !isLayoutStep(s)).map((s) => ({ app: s.app, target: s.target, state: nodeState(this.tree(s.app).get(s.target)!) }));
    const layoutBefore = steps.some(isLayoutStep) ? JSON.stringify(this.state()) : undefined;
    const p: Proposal = { id: `p${this.seq++}`, intent, steps: structuredClone(steps), status: 'pending', sensitive, preconditions, layoutBefore };
    this.proposals.push(p);
    if (this.policy.autoApproveSafe && !sensitive) this.approve(p.id);
    else this.onChange();
    return { ok: true, proposal: p };
  }

  approve(id: string): void {
    const p = this.proposals.find((x) => x.id === id);
    if (!p || p.status !== 'pending') return;
    // 提案と承認の間に人 (や別のアプリの更新) が状況を変えていたら、提案の意図と結果がずれる。実行せずに古い提案にする
    const stale = this.staleReason(p);
    if (stale) {
      p.status = 'stale';
      this.audit.push({ at: Date.now(), by: 'agent', app: '*', target: p.id, action: 'layout', result: 'denied', reason: `提案のあとに変わった: ${stale}`, proposal: p.id });
      this.onChange();
      return;
    }
    p.status = 'approved';
    for (const s of p.steps) {
      if (isLayoutStep(s)) this.applyLayout(s.op, 'agent', p.id);
      else this.dispatch({ app: s.app, target: s.target, action: s.action, data: s.data, by: 'agent', proposal: p.id });
    }
    p.status = 'done';
    this.onChange();
  }

  private staleReason(p: Proposal): string {
    for (const c of p.preconditions) {
      const n = this.wins.find((w) => w.app.id === c.app)?.tree.get(c.target);
      if (!n) return `${c.app}/${c.target} がなくなった`;
      const now = nodeState(n);
      for (const k of Object.keys(c.state) as (keyof NodeState)[])
        if (JSON.stringify(now[k]) !== JSON.stringify(c.state[k])) return `${c.app}/${c.target} の ${k} が ${JSON.stringify(c.state[k])} → ${JSON.stringify(now[k])}`;
    }
    if (p.layoutBefore !== undefined && p.layoutBefore !== JSON.stringify(this.state())) return '窓の配置';
    return '';
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
    const sizeKey = `${width}x${height}`;
    const resized = sizeKey !== this.lastSize;
    this.lastSize = sizeKey;
    this.size = { w: width, h: height };
    this.area = this.areaFor(this.minimized.length);

    const ops: DrawOp[] = [['rect', { x: 0, y: 0, w: width, h: height, fill: 'bg' }]];
    this.hits = [];
    this.titles.clear();
    this.closeRects.clear();
    this.tabRects = [];
    const windows: WindowInfo[] = [];
    let ime: ImeToHost = { t: 'ime-disable' };
    let animating = false;
    const resizes: { w: Win; node: NodeId; cols: number; rows: number }[] = [];
    const snap = !this.animations || resized || this.snapTiles;
    this.snapTiles = false;

    // ライブプレビュー: ドラッグ中は、落とした後の配置へほかの窓が動いて場所を空ける
    const g = this.drag?.active ? this.drag : null;
    const live = !!(g && g.valid && g.next && g.drop && (g.drop.kind === 'insert' || (this.previewMode === 'live' && g.drop.kind !== 'tab')));
    const shownTree = live ? g!.next! : this.tiles;
    const { tiles: targets, dividers } = computeRects(shownTree, this.area, GAP);
    this.dividers = live ? [] : dividers;

    const ghosts = new Map<string, Map<NodeId, number>>();
    for (const p of this.proposals.filter((p) => p.status === 'pending'))
      p.steps.forEach((s, i) => {
        if (isLayoutStep(s)) return;
        if (!ghosts.has(s.app)) ghosts.set(s.app, new Map());
        ghosts.get(s.app)!.set(s.target, i + 1);
      });

    const dragging = g?.app ?? null;
    const order = leaves(shownTree);
    // ズーム中の窓、ドラッグ中の窓は最後に描く (下に隠れない)
    const last = this.zoomed && order.includes(this.zoomed) ? this.zoomed : dragging && order.includes(dragging) ? dragging : null;
    const paintOrder = last ? [...order.filter((a) => a !== last), last] : order;
    let dragTitle = dragging ? this.titleOf(dragging) : '';
    for (const app of paintOrder) {
      const w = this.win(app);
      // ライブプレビュー中、運んでいる窓は青いプレビューが代わりに示す
      if (live && app === dragging) continue;
      const target = this.zoomed === app ? this.area : targets.get(app)!;
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
      this.tileRects.set(app, tile);
      // スクロール: 中身が窓より高ければ、ホイールで動かせる
      const sp = this.scrollOf(app);
      let lo = layout(view, content.w, this.measure, { x: content.x, y: content.y - sp.x });
      const max = Math.max(0, lo.height - content.h);
      this.scrollMax.set(app, max);
      if (sp.target > max) sp.target = max;
      if (this.revealFocus && i === this.focusedWin && w.tree.focus) {
        // フォーカスした要素が画面外なら、見える位置まで送る (AccessKit の scrollIntoView に当たる)
        const find = (l: typeof lo.root): Rect | null => (l.view.kind === 'widget' && l.view.node.id === w.tree.focus ? l.rect : l.children.map(find).find(Boolean) ?? null);
        const r = find(lo.root);
        if (r) {
          const top = r.y + sp.x - content.y;
          if (top < sp.target) sp.target = Math.max(0, top - 8);
          else if (top + r.h > sp.target + content.h) sp.target = Math.min(max, top + r.h - content.h + 8);
        }
        this.revealFocus = false;
      }
      if (snap || !this.animations) sp.snap();
      animating ||= !sp.step(dt);
      if (sp.x !== 0 || max > 0) lo = layout(view, content.w, this.measure, { x: content.x, y: content.y - sp.x });
      if (this.zoomed !== app) this.minWidths.set(app, lo.minWidth);
      // 窓が止まっていれば、端末の画面に使える桁と行を知らせる (アニメーション中・ドラッグ中は知らせない)
      if (settled && app !== dragging && w.app.onResize) gridFits(lo.root, content.h - lo.height, (node, cols, rows) => resizes.push({ w, node, cols, rows }));
      const focused = i === this.focusedWin;
      const pt = paint(lo.root, { focus: focused ? w.tree.focus : null, preedit: focused ? this.preedit : '', ghosts: ghosts.get(app) ?? new Map() }, this.measure, content);
      const title = this.titleOf(app);

      const dim = app === dragging;
      if (dim) ops.push(['layer', { alpha: 0.4 }]);
      ops.push(['rect', { ...tile, radius: 10, fill: 'surface', stroke: focused ? 'accent' : 'border', lineWidth: focused ? 2 : 1 }]);
      ops.push(['clip', tile]);
      // つまみ (⠿) : タイトルバーを掴んで動かせることを示す
      for (let r = 0; r < 3; r++)
        for (let c = 0; c < 2; c++) ops.push(['rect', { x: tile.x + 12 + c * 4, y: tile.y + 10 + r * 4, w: 2, h: 2, radius: 1, fill: 'muted' }]);
      const group = groupOf(shownTree, app);
      if (group?.tabs) {
        // タブ: 表に出ているものを強調。タブを掴めばその窓だけを運べる
        let tx = tile.x + 26;
        for (const tabApp of group.tabs) {
          const label = this.titleOf(tabApp);
          const tw = Math.min(140, this.measure(label, 12, 600) + 20);
          const r = { x: tx, y: tile.y + 4, w: tw, h: TITLE_H - 8 };
          const on = tabApp === app;
          if (on) ops.push(['rect', { ...r, radius: 6, fill: 'bg' }]);
          ops.push(['text', { x: r.x + 10, y: r.y + r.h / 2, str: label, size: 12, weight: 600, color: on ? 'text' : 'muted' }]);
          this.tabRects.push({ app: tabApp, rect: r });
          tx += tw + 4;
        }
      } else ops.push(['text', { x: tile.x + 28, y: tile.y + TITLE_H / 2, str: title, size: 12, weight: 600, color: 'muted' }]);
      // 閉じるボタン (×): 最後の 1 枚には出さない
      const closable = this.wins.length > 1;
      const cr = { x: tile.x + tile.w - 28, y: tile.y + 4, w: 22, h: 22 };
      if (closable) {
        const hot = inside(cr, this.hover.x, this.hover.y);
        if (hot) ops.push(['rect', { ...cr, radius: 6, fill: 'bg' }]);
        ops.push(['text', { x: cr.x + 6, y: cr.y + 11, str: '×', size: 15, weight: 600, color: hot ? 'text' : 'muted' }]);
      }
      const right = tile.x + tile.w - (closable ? 32 : 8);
      if (this.zoomed === app) {
        const z = 'ズーム中 (ダブルクリック / Esc で戻す)';
        const zw = this.measure(z, 11, 600) + 16;
        ops.push(['rect', { x: right - zw, y: tile.y + 6, w: zw, h: 18, radius: 9, fill: 'accent' }]);
        ops.push(['text', { x: right - zw + 8, y: tile.y + 15, str: z, size: 11, weight: 600, color: 'accentText' }]);
      } else if (lo.overflow.length && settled) {
        const long = `はみ出し: 最小 ${lo.minWidth}px > 幅 ${Math.floor(content.w)}px`;
        const fits = this.measure(long, 11, 600) + 12 + 110 < tile.w;
        const msg = fits ? long : `${lo.minWidth}px 必要`;
        const mw = this.measure(msg, 11, 600) + 12;
        ops.push(['rect', { x: right - mw, y: tile.y + 6, w: mw, h: 18, radius: 9, fill: 'danger' }]);
        ops.push(['text', { x: right - mw + 6, y: tile.y + 15, str: msg, size: 11, weight: 600, color: 'accentText' }]);
      }
      ops.push(['pop']);
      ops.push(['clip', content]);
      ops.push(...pt.ops);
      if (max > 0) {
        // スクロールバー (中身に重ねる。幅を取らない overlay 型)
        const th = Math.max(24, (content.h * content.h) / lo.height);
        const ty = content.y + ((content.h - th) * sp.x) / max;
        ops.push(['layer', { alpha: 0.5 }]);
        ops.push(['rect', { x: content.x + content.w - 7, y: ty + 2, w: 4, h: th - 4, radius: 2, fill: 'muted' }]);
        ops.push(['pop']);
      }
      ops.push(['pop']);
      if (dim) ops.push(['pop']);
      const hidden = this.zoomed && this.zoomed !== app;
      if (!hidden) this.titles.set(app, { x: tile.x, y: tile.y, w: tile.w - (closable ? 32 : 0), h: TITLE_H });
      if (!hidden && closable && !dim) this.closeRects.set(app, cr);
      // 窓の中身の矩形で切る (隣の窓にはみ出した部分や、タイトルバーの裏に隠れた部分は押せない)
      if (!dim && !hidden)
        for (const hit of pt.hits) {
          const r = intersect(hit.rect, content);
          if (r) this.hits.push({ app, hit: { ...hit, rect: r } });
        }
      if (focused && pt.caret && w.tree.focus && !hidden) {
        const f = w.tree.get(w.tree.focus);
        ime = { t: 'ime-enable', node: w.tree.focus, surrounding: f?.value ?? '', cursor: [...(f?.value ?? '')].length, caret: pt.caret };
      }
      windows.push({ app, title, tile, minWidth: lo.minWidth, overflow: settled && this.zoomed !== app ? lo.overflow : [], issues });
    }
    if (!this.zoomed)
      for (const d of this.dividers) {
        const c = d.dir === 'row' ? { x: d.rect.x + GAP / 2 - 1, y: d.rect.y + d.rect.h / 2 - 20, w: 2, h: 40 } : { x: d.rect.x + d.rect.w / 2 - 20, y: d.rect.y + GAP / 2 - 1, w: 40, h: 2 };
        ops.push(['rect', { ...c, radius: 1, fill: 'border' }]);
      }

    // ドック: 最小化した窓の札。押すと元に戻る
    this.chips = [];
    const dock = this.dockRect(this.minimized.length);
    if (dock) {
      ops.push(['rect', { ...dock, radius: 10, fill: 'surface', stroke: 'border' }]);
      let cx = dock.x + 8;
      for (const app of this.minimized) {
        const label = this.titleOf(app);
        const cw = Math.min(160, this.measure(label, 12, 600) + 28);
        const r = { x: cx, y: dock.y + 6, w: cw, h: dock.h - 12 };
        ops.push(['rect', { ...r, radius: 8, fill: 'bg', stroke: 'border' }]);
        ops.push(['text', { x: r.x + 14, y: r.y + r.h / 2, str: label, size: 12, weight: 600, color: 'text' }]);
        this.chips.push({ app, rect: r });
        cx += cw + 6;
      }
    }
    // 最小化した直後の窓: 札へ縮みながら消える
    for (const [app, l] of this.leaving) {
      const chip = this.chips.find((c) => c.app === app)?.rect ?? dock ?? { x: width / 2, y: height, w: 0, h: 0 };
      l.rect.target = chip;
      l.alpha.target = 0;
      if (!this.animations) {
        l.rect.snap();
        l.alpha.snap();
      }
      const done = l.rect.step(dt) && l.alpha.step(dt);
      const r = l.rect.value;
      const al = Math.max(0, Math.min(1, l.alpha.x));
      if (al > 0.01) {
        ops.push(['layer', { alpha: al }]);
        ops.push(['rect', { ...r, radius: 10, fill: 'surface', stroke: 'border' }]);
        if (r.w > 60) ops.push(['text', { x: r.x + 12, y: r.y + Math.min(15, r.h / 2), str: this.titleOf(app), size: 12, weight: 600, color: 'muted' }]);
        ops.push(['pop']);
      }
      if (done) this.leaving.delete(app);
      else animating = true;
    }

    // agent のレイアウト提案: 実行後の配置を破線で示す (承認するまで何も動かない)
    for (const p of this.proposals.filter((p) => p.status === 'pending' && p.steps.some(isLayoutStep))) {
      let st: LayoutState | string = this.state();
      for (const s of p.steps) if (isLayoutStep(s) && typeof st !== 'string') st = this.simulate(st, s.op);
      if (typeof st === 'string') continue;
      const fin = computeRects(st.tiles, this.areaFor(st.minimized.length), GAP).tiles;
      for (const [app, r] of fin) {
        const cur = targets.get(app);
        const rr = st.zoomed === app ? this.areaFor(st.minimized.length) : r;
        if (cur && Math.abs(cur.x - rr.x) + Math.abs(cur.y - rr.y) + Math.abs(cur.w - rr.w) + Math.abs(cur.h - rr.h) < 2) continue;
        ops.push(['rect', { ...rr, radius: 10, stroke: 'ghost', dash: true, lineWidth: 2 }]);
        const grp = groupOf(st.tiles, app);
        const names = (grp?.tabs ?? [app]).map((a) => this.titleOf(a)).join(' + ');
        const isNew = !this.wins.some((w) => w.app.id === app);
        const label = isNew ? `${p.id}: 新しい窓 ${this.titleOf(app)}` : `${p.id}: ${names}${grp?.tabs ? ' (タブ)' : ''} → ここ`;
        const lw = this.measure(label, 12, 600) + 20;
        ops.push(['rect', { x: rr.x + rr.w / 2 - lw / 2, y: rr.y + rr.h / 2 - 13, w: lw, h: 26, radius: 13, fill: 'ghost' }]);
        ops.push(['text', { x: rr.x + rr.w / 2 - lw / 2 + 10, y: rr.y + rr.h / 2, str: label, size: 12, weight: 600, color: 'accentText' }]);
      }
      for (const app of st.minimized.filter((a) => !this.minimized.includes(a))) {
        const cur = targets.get(app);
        if (!cur) continue;
        // 「最小化される」は窓の上端 (タイトルバーの下) に出し、中央の配置ラベルと重ねない
        const label = `${p.id}: ${this.titleOf(app)} は最小化される`;
        const lw = this.measure(label, 12, 600) + 20;
        ops.push(['rect', { x: cur.x + 10, y: cur.y + TITLE_H + 6, w: lw, h: 24, radius: 12, fill: 'ghost' }]);
        ops.push(['text', { x: cur.x + 20, y: cur.y + TITLE_H + 18, str: label, size: 12, weight: 600, color: 'accentText' }]);
      }
    }

    // ドラッグ中に現れる「最小化」の受け口
    {
      const s1 = this.animations ? this.pill.step(dt) : (this.pill.snap(), true);
      animating ||= !s1;
      const al = Math.max(0, Math.min(1, this.pill.x));
      if (al > 0.01 && !dock) {
        const r = this.dropDock();
        const hot = g?.drop?.kind === 'minimize';
        ops.push(['layer', { alpha: al }]);
        if (hot && !g!.valid) r.w = Math.max(r.w, this.measure(g!.reason, 12, 600) + 32);
        ops.push(['rect', { ...r, radius: r.h / 2, fill: hot ? (g!.valid ? 'accent' : 'danger') : 'surface', stroke: hot ? (g!.valid ? 'accent' : 'danger') : 'border', lineWidth: 1.5 }]);
        const label = hot ? (g!.valid ? '離すと最小化' : g!.reason) : '▾ ここで最小化';
        ops.push(['text', { x: r.x + r.w / 2 - this.measure(label, 12, 600) / 2, y: r.y + r.h / 2, str: label, size: 12, weight: 600, color: hot ? 'accentText' : 'muted' }]);
        ops.push(['pop']);
      }
    }

    // ドロップ先のプレビュー (青。最小幅を下回る分割は赤)
    const p = this.preview;
    if (p) {
      if (!g || !g.drop) p.alpha.target = 0;
      if (!this.animations) {
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
        ops.push(['layer', { alpha: (live ? 0.3 : 0.22) * alpha }]);
        ops.push(['rect', { ...r, radius: 10, fill: color }]);
        ops.push(['pop']);
        ops.push(['layer', { alpha }]);
        ops.push(['rect', { ...r, radius: 10, stroke: color, lineWidth: 2 }]);
        if (p.label && r.w > 60 && r.h > 30) {
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
    if (g) {
      dragTitle ||= this.titleOf(g.app);
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
    if (g) cursor = g.drop && !g.valid ? 'not-allowed' : 'grabbing';
    else if (div && !this.zoomed) cursor = div.dir === 'row' ? 'col-resize' : 'row-resize';
    else if (this.chips.some((c) => inside(c.rect, hv.x, hv.y))) cursor = 'pointer';
    else if ([...this.titles.values()].some((r) => inside(r, hv.x, hv.y))) cursor = 'grab';
    else if (this.hits.some((h) => inside(h.hit.rect, hv.x, hv.y))) cursor = 'pointer';

    // 描き終えてから知らせる (アプリが木を更新すると、次のフレームで反映される)
    for (const r of resizes) {
      const key = `${r.w.app.id}/${r.node}`;
      const v = `${r.cols}x${r.rows}`;
      if (this.gridSent.get(key) === v) continue;
      this.gridSent.set(key, v);
      r.w.app.onResize!(r.node, r.cols, r.rows);
    }
    return { ops, ime, windows, animating, cursor };
  }
}

const DOCK_H = 40;
const MIN_CONTENT_H = 80;

function describeOp(op: LayoutOp): string {
  switch (op.type) {
    case 'move':
      return `move ${op.app}: ${describeDrop(op.drop)}`;
    case 'zoom':
      return op.app ? `zoom ${op.app}` : 'unzoom';
    case 'minimize':
      return `minimize ${op.app}`;
    case 'restore':
      return `restore ${op.app}`;
    case 'equalize':
      return 'equalize';
    case 'launch':
      return `launch ${op.app}`;
    case 'close':
      return `close ${op.app}`;
  }
}

function nodeAtPath(t: TileNode, path: number[]): TileNode | null {
  let n: TileNode = t;
  for (const h of path) {
    if (n.kind !== 'split') return null;
    n = h === 0 ? n.a : n.b;
  }
  return n;
}

function mapGroup(t: TileNode, app: AppId, f: (l: Extract<TileNode, { kind: 'leaf' }>) => TileNode): TileNode {
  if (t.kind === 'leaf') return t.app === app ? f(t) : t;
  return { ...t, a: mapGroup(t.a, app, f), b: mapGroup(t.b, app, f) };
}

function grow(r: Rect, d: number): Rect {
  return { x: r.x - d, y: r.y - d, w: r.w + 2 * d, h: r.h + 2 * d };
}

function inside(r: Rect, x: number, y: number) {
  return x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h;
}

function intersect(a: Rect, b: Rect): Rect | null {
  const x = Math.max(a.x, b.x);
  const y = Math.max(a.y, b.y);
  const w = Math.min(a.x + a.w, b.x + b.w) - x;
  const h = Math.min(a.y + a.h, b.y + b.h) - y;
  return w > 0 && h > 0 ? { x, y, w, h } : null;
}

const LAYOUT_OPS = ['move', 'zoom', 'minimize', 'restore', 'equalize', 'launch', 'close'];
/** 提案の手順の形 (型) の検査。中身 (ノードがあるか、action を公開しているか) は propose が確かめる */
function stepShapeError(s: unknown, i: number): string | null {
  if (typeof s !== 'object' || s === null || Array.isArray(s)) return `steps[${i}]: object が必要`;
  const o = s as Record<string, unknown>;
  if (o.kind === 'layout') {
    const op = o.op as Record<string, unknown> | null;
    if (typeof op !== 'object' || op === null || !LAYOUT_OPS.includes(op.type as string)) return `steps[${i}].op.type: ${LAYOUT_OPS.join(' | ')}`;
    if ('app' in op && op.app !== null && typeof op.app !== 'string') return `steps[${i}].op.app: 文字列`;
    return null;
  }
  for (const k of ['app', 'target', 'action'] as const) if (typeof o[k] !== 'string') return `steps[${i}].${k}: 文字列`;
  if (o.data !== undefined && typeof o.data !== 'string') return `steps[${i}].data: 文字列`;
  return null;
}

/**
 * 端末の画面 (terminal widget) に使える桁と行。横は部品の幅、縦は今の高さに窓の余り (足りなければ負) を足した高さ。
 * 行を変えると部品の高さが変わり、余りがその分だけ減るので、次のフレームで行が落ち着く
 */
function gridFits(root: LaidOut, slack: number, out: (node: NodeId, cols: number, rows: number) => void): void {
  const visit = (l: LaidOut) => {
    if (l.view.kind === 'widget' && l.view.widget === 'terminal') {
      const cols = Math.max(20, Math.floor((l.rect.w - 2 * M.termPad) / M.termCell));
      const rows = Math.max(4, Math.floor((l.rect.h - 2 * M.termPad + slack) / M.termLine));
      out(l.view.node.id, cols, rows);
    }
    l.children.forEach(visit);
  };
  visit(root);
}
