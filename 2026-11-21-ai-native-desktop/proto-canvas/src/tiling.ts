// タイルの分割ツリーと、ドラッグ&ドロップでの配置換え。DOM に依存しない純粋な関数だけ。
// パターン: 端で分割 / 中央で入れ替え / 外周で全体の端 / タイトルバーでタブにまとめる / 境界に差し込む / 最小化 (shell 側)
import type { AppId, Rect } from './protocol.ts';

export type Dir = 'row' | 'column';
/** leaf の tabs はグループ内の全ての窓 (app はその中で表示中のもの)。2 つ以上のときだけ持つ */
export type TileNode = { kind: 'leaf'; app: AppId; tabs?: AppId[] } | { kind: 'split'; dir: Dir; ratio: number; a: TileNode; b: TileNode };
export type Edge = 'left' | 'right' | 'top' | 'bottom';
export type Drop =
  /** target が null ならワークスペース全体の端 */
  | { kind: 'split'; target: AppId | null; edge: Edge }
  | { kind: 'swap'; target: AppId }
  /** target の窓と同じタブグループに入れる */
  | { kind: 'tab'; target: AppId }
  /** path の split の a と b の間に差し込む */
  | { kind: 'insert'; path: number[] }
  /** ドックへ (木から外す。shell が扱う) */
  | { kind: 'minimize' };

export interface Divider {
  /** split ノードまでの道 (0 = a, 1 = b) */
  path: number[];
  dir: Dir;
  /** split ノードが占める領域 (比率の計算用) */
  area: Rect;
  /** つまめる帯 */
  rect: Rect;
}

export const leaf = (app: AppId): TileNode => ({ kind: 'leaf', app });

/** 表示中の窓 (タブグループでは表に出ているものだけ) */
export function leaves(t: TileNode): AppId[] {
  return t.kind === 'leaf' ? [t.app] : [...leaves(t.a), ...leaves(t.b)];
}

/** 木に入っている全ての窓 (隠れたタブも含む) */
export function allApps(t: TileNode): AppId[] {
  return t.kind === 'leaf' ? (t.tabs ?? [t.app]) : [...allApps(t.a), ...allApps(t.b)];
}

/** app を含む葉 (タブの中も探す) */
export function groupOf(t: TileNode, app: AppId): Extract<TileNode, { kind: 'leaf' }> | null {
  if (t.kind === 'leaf') return t.app === app || t.tabs?.includes(app) ? t : null;
  return groupOf(t.a, app) ?? groupOf(t.b, app);
}

export function computeRects(t: TileNode, area: Rect, gap: number): { tiles: Map<AppId, Rect>; dividers: Divider[] } {
  const tiles = new Map<AppId, Rect>();
  const dividers: Divider[] = [];
  const visit = (n: TileNode, r: Rect, path: number[]) => {
    if (n.kind === 'leaf') {
      tiles.set(n.app, r);
      return;
    }
    if (n.dir === 'row') {
      const aw = (r.w - gap) * n.ratio;
      visit(n.a, { x: r.x, y: r.y, w: aw, h: r.h }, [...path, 0]);
      visit(n.b, { x: r.x + aw + gap, y: r.y, w: r.w - gap - aw, h: r.h }, [...path, 1]);
      dividers.push({ path, dir: n.dir, area: r, rect: { x: r.x + aw, y: r.y, w: gap, h: r.h } });
    } else {
      const ah = (r.h - gap) * n.ratio;
      visit(n.a, { x: r.x, y: r.y, w: r.w, h: ah }, [...path, 0]);
      visit(n.b, { x: r.x, y: r.y + ah + gap, w: r.w, h: r.h - gap - ah }, [...path, 1]);
      dividers.push({ path, dir: n.dir, area: r, rect: { x: r.x, y: r.y + ah, w: r.w, h: gap } });
    }
  };
  visit(t, area, []);
  return { tiles, dividers };
}

/** 窓を取り除く。タブグループなら抜けるだけ。葉が空になれば兄弟がその split の場所を引き継ぐ */
export function removeApp(t: TileNode, app: AppId): TileNode | null {
  if (t.kind === 'leaf') {
    if (t.tabs?.includes(app)) {
      const i = t.tabs.indexOf(app);
      const tabs = t.tabs.filter((a) => a !== app);
      const shown = t.app === app ? tabs[Math.max(0, i - 1)] : t.app;
      return tabs.length > 1 ? { kind: 'leaf', app: shown, tabs } : leaf(shown);
    }
    return t.app === app ? null : t;
  }
  const a = removeApp(t.a, app);
  const b = removeApp(t.b, app);
  if (!a) return b;
  if (!b) return a;
  return { ...t, a, b };
}
/** 以前の名前 */
export const removeLeaf = removeApp;

const dirOf = (e: Edge): Dir => (e === 'left' || e === 'right' ? 'row' : 'column');
const first = (e: Edge) => e === 'left' || e === 'top';

function mapLeaves(t: TileNode, f: (l: Extract<TileNode, { kind: 'leaf' }>) => TileNode): TileNode {
  if (t.kind === 'leaf') return f(t);
  return { ...t, a: mapLeaves(t.a, f), b: mapLeaves(t.b, f) };
}

function nodeAt(t: TileNode, path: number[]): TileNode | null {
  let n: TileNode = t;
  for (const h of path) {
    if (n.kind !== 'split') return null;
    n = h === 0 ? n.a : n.b;
  }
  return n;
}

function replaceAt(t: TileNode, path: number[], f: (n: TileNode) => TileNode): TileNode {
  if (path.length === 0) return f(t);
  if (t.kind !== 'split') return t;
  const [h, ...rest] = path;
  return h === 0 ? { ...t, a: replaceAt(t.a, rest, f) } : { ...t, b: replaceAt(t.b, rest, f) };
}

/** 2 つの窓の位置を入れ替える (タブの中の位置も) */
export function swapApps(t: TileNode, x: AppId, y: AppId): TileNode {
  const sw = (a: AppId) => (a === x ? y : a === y ? x : a);
  return mapLeaves(t, (l) => (l.tabs ? { kind: 'leaf', app: sw(l.app), tabs: l.tabs.map(sw) } : leaf(sw(l.app))));
}

/** ドロップを木に適用する。意味のないもの (自分自身へ、など) は null。minimize は shell が扱うので null */
export function applyDrop(t: TileNode, app: AppId, drop: Drop): TileNode | null {
  switch (drop.kind) {
    case 'minimize':
      return null;
    case 'swap': {
      if (drop.target === app || groupOf(t, drop.target) === groupOf(t, app)) return null;
      return swapApps(t, app, drop.target);
    }
    case 'tab': {
      if (drop.target === app) return null;
      const g = groupOf(t, drop.target);
      if (!g || g.tabs?.includes(app)) return null;
      const rest = removeApp(t, app);
      if (!rest) return null;
      return mapLeaves(rest, (l) =>
        l.app === drop.target || l.tabs?.includes(drop.target) ? { kind: 'leaf', app, tabs: [...(l.tabs ?? [l.app]), app] } : l,
      );
    }
    case 'insert': {
      const target = nodeAt(t, drop.path);
      if (!target || target.kind !== 'split') return null;
      // 取り除く前に印を付け、取り除いた後の木で同じ split を探す (道が変わるため)
      const MARK = '__insert__';
      const marked = replaceAt(t, drop.path, (n) => ({ ...n, [MARK]: true }) as unknown as TileNode);
      const rest = removeApp(marked, app);
      if (!rest) return null;
      const find = (n: TileNode, path: number[]): number[] | null => {
        if ((n as unknown as Record<string, unknown>)[MARK]) return path;
        if (n.kind !== 'split') return null;
        return find(n.a, [...path, 0]) ?? find(n.b, [...path, 1]);
      };
      const at = find(rest, []);
      if (!at) return null; // 運んだ窓がその split の直接の子だった: 差し込む先がない
      const out = replaceAt(rest, at, (n) => {
        if (n.kind !== 'split') return n;
        const { [MARK]: _m, ...s } = n as TileNode & Record<string, unknown>;
        const r = (s as Extract<TileNode, { kind: 'split' }>).ratio;
        // a と b はそれぞれ 2/3 に縮み、差し込んだ窓が全体の 1/3 を取る
        const inner = 1 / 3 / (1 / 3 + (2 / 3) * (1 - r));
        return { kind: 'split', dir: n.dir, ratio: (2 / 3) * r, a: n.a, b: { kind: 'split', dir: n.dir, ratio: inner, a: leaf(app), b: n.b } };
      });
      return out;
    }
    case 'split': {
      if (drop.target === app) return null;
      const rest = removeApp(t, app);
      if (!rest) return null;
      const pair = (other: TileNode): TileNode => {
        const moved = leaf(app);
        return { kind: 'split', dir: dirOf(drop.edge), ratio: 0.5, a: first(drop.edge) ? moved : other, b: first(drop.edge) ? other : moved };
      };
      if (drop.target === null) return pair(rest);
      if (!groupOf(rest, drop.target)) return null;
      return mapLeaves(rest, (l) => (l.app === drop.target || l.tabs?.includes(drop.target!) ? pair(l) : l));
    }
  }
}

export function setRatio(t: TileNode, path: number[], ratio: number): TileNode {
  return replaceAt(t, path, (n) => (n.kind === 'split' ? { ...n, ratio } : n));
}

export function activateTab(t: TileNode, app: AppId): TileNode {
  return mapLeaves(t, (l) => (l.tabs?.includes(app) ? { ...l, app } : l));
}

/** すべての split の比率を、両側のペーンの数に比例させる (どのペーンも同じ大きさになる) */
export function equalize(t: TileNode): TileNode {
  if (t.kind === 'leaf') return t;
  const a = equalize(t.a);
  const b = equalize(t.b);
  const count = (n: TileNode, dir: Dir): number => (n.kind === 'split' && n.dir === dir ? count(n.a, dir) + count(n.b, dir) : 1);
  const ca = count(a, t.dir);
  const cb = count(b, t.dir);
  return { ...t, a, b, ratio: ca / (ca + cb) };
}

/** 窓を 1 つ足す: 一番大きいペーンを、長い辺の向きに分ける */
export function addToLargest(t: TileNode | null, app: AppId, area: Rect, gap: number): TileNode {
  if (!t) return leaf(app);
  const { tiles } = computeRects(t, area, gap);
  let best: [AppId, Rect] | null = null;
  for (const e of tiles) if (!best || e[1].w * e[1].h > best[1].w * best[1].h) best = e;
  const [target, r] = best!;
  return mapLeaves(t, (l) =>
    l.app === target ? { kind: 'split', dir: r.w >= r.h ? 'row' : 'column', ratio: 0.5, a: l, b: leaf(app) } : l,
  );
}

/** 向き (方向キー) で隣のペーンを探す */
export function neighbor(t: TileNode, area: Rect, gap: number, app: AppId, dir: Edge): AppId | null {
  const { tiles } = computeRects(t, area, gap);
  const me = tiles.get(app);
  if (!me) return null;
  const cx = me.x + me.w / 2;
  const cy = me.y + me.h / 2;
  let best: [AppId, number] | null = null;
  for (const [other, r] of tiles) {
    if (other === app) continue;
    const ox = r.x + r.w / 2;
    const oy = r.y + r.h / 2;
    const overlapY = Math.min(me.y + me.h, r.y + r.h) - Math.max(me.y, r.y) > 0;
    const overlapX = Math.min(me.x + me.w, r.x + r.w) - Math.max(me.x, r.x) > 0;
    const ok =
      (dir === 'left' && ox < cx && overlapY) || (dir === 'right' && ox > cx && overlapY) || (dir === 'top' && oy < cy && overlapX) || (dir === 'bottom' && oy > cy && overlapX);
    if (!ok) continue;
    const d = Math.hypot(ox - cx, oy - cy);
    if (!best || d < best[1]) best = [other, d];
  }
  return best?.[0] ?? null;
}

/** ドロップの候補の帯の幅 */
export const ROOT_BAND = 28;
/** 境界に差し込む帯 (境界の中心線からの距離) */
export const INSERT_BAND = 12;
const PANE_ZONE = 0.28;

export interface HitOptions {
  /** タイトルバーの高さ (この帯に落とすとタブにまとめる) */
  titleH?: number;
  /** 最小化の受け口 */
  dock?: Rect | null;
}

/**
 * ポインタの位置からドロップの候補を決める。優先順:
 * ドック → 境界の帯 (差し込み) → ワークスペースの外周 (全体の端) → ペーンのタイトルバー (タブ) → ペーンの端 (分割) → 中央 (入れ替え)
 */
export function hitDrop(t: TileNode, area: Rect, gap: number, dragged: AppId, x: number, y: number, opts: HitOptions = {}): { drop: Drop; preview: Rect } | null {
  const inside = (r: Rect) => x >= r.x && y >= r.y && x <= r.x + r.w && y <= r.y + r.h;
  if (opts.dock && inside(opts.dock)) return allApps(t).length > 1 ? { drop: { kind: 'minimize' }, preview: opts.dock } : null;
  if (!inside(area)) return null;
  const half = (r: Rect, e: Edge): Rect => {
    const w = (r.w - gap) / 2;
    const h = (r.h - gap) / 2;
    if (e === 'left') return { x: r.x, y: r.y, w, h: r.h };
    if (e === 'right') return { x: r.x + r.w - w, y: r.y, w, h: r.h };
    if (e === 'top') return { x: r.x, y: r.y, w: r.w, h };
    return { x: r.x, y: r.y + r.h - h, w: r.w, h };
  };
  const { tiles, dividers } = computeRects(t, area, gap);
  for (const d of dividers) {
    const band =
      d.dir === 'row'
        ? { x: d.rect.x + d.rect.w / 2 - INSERT_BAND, y: d.rect.y, w: 2 * INSERT_BAND, h: d.rect.h }
        : { x: d.rect.x, y: d.rect.y + d.rect.h / 2 - INSERT_BAND, w: d.rect.w, h: 2 * INSERT_BAND };
    if (inside(band) && applyDrop(t, dragged, { kind: 'insert', path: d.path })) return { drop: { kind: 'insert', path: d.path }, preview: d.rect };
  }
  if (leaves(t).length > 1 || allApps(t).length > 1) {
    const cands: [Edge, number][] = [
      ['left', x - area.x],
      ['right', area.x + area.w - x],
      ['top', y - area.y],
      ['bottom', area.y + area.h - y],
    ];
    cands.sort((p, q) => p[1] - q[1]);
    if (cands[0][1] <= ROOT_BAND) return { drop: { kind: 'split', target: null, edge: cands[0][0] }, preview: half(area, cands[0][0]) };
  }
  for (const [app, r] of tiles) {
    if (!inside(r)) continue;
    const same = app === dragged;
    if (opts.titleH && y <= r.y + opts.titleH) {
      if (same || groupOf(t, app)?.tabs?.includes(dragged)) return null;
      return { drop: { kind: 'tab', target: app }, preview: r };
    }
    if (same) return null;
    // 帯の幅をペーンの大きさに比例させる (細いペーンでも中央を残す)
    const fx = (x - r.x) / r.w;
    const fy = (y - r.y) / r.h;
    const cands: [Edge, number][] = [
      ['left', fx],
      ['right', 1 - fx],
      ['top', fy],
      ['bottom', 1 - fy],
    ];
    cands.sort((p, q) => p[1] - q[1]);
    if (cands[0][1] <= PANE_ZONE) return { drop: { kind: 'split', target: app, edge: cands[0][0] }, preview: half(r, cands[0][0]) };
    if (groupOf(t, app)?.tabs?.includes(dragged)) return null;
    return { drop: { kind: 'swap', target: app }, preview: r };
  }
  return null;
}

export function describeDrop(d: Drop): string {
  switch (d.kind) {
    case 'swap':
      return '入れ替え';
    case 'tab':
      return 'タブにまとめる';
    case 'insert':
      return '間に差し込む';
    case 'minimize':
      return '最小化';
    case 'split': {
      const e = { left: '左', right: '右', top: '上', bottom: '下' }[d.edge];
      return d.target === null ? `全体の${e}に配置` : `${e}に分割`;
    }
  }
}
