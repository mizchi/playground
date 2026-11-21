// タイルの分割ツリーと、ドラッグ&ドロップでの分割 (VS Code のエディタ分割のような動き)。DOM に依存しない純粋な関数だけ。
import type { AppId, Rect } from './protocol.ts';

export type Dir = 'row' | 'column';
export type TileNode = { kind: 'leaf'; app: AppId } | { kind: 'split'; dir: Dir; ratio: number; a: TileNode; b: TileNode };
export type Edge = 'left' | 'right' | 'top' | 'bottom';
/** target が null ならワークスペース全体の端 */
export type Drop = { kind: 'split'; target: AppId | null; edge: Edge } | { kind: 'swap'; target: AppId };

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

export function leaves(t: TileNode): AppId[] {
  return t.kind === 'leaf' ? [t.app] : [...leaves(t.a), ...leaves(t.b)];
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

/** 葉を取り除く。兄弟がその split の場所を引き継ぐ */
export function removeLeaf(t: TileNode, app: AppId): TileNode | null {
  if (t.kind === 'leaf') return t.app === app ? null : t;
  const a = removeLeaf(t.a, app);
  const b = removeLeaf(t.b, app);
  if (!a) return b;
  if (!b) return a;
  return { ...t, a, b };
}

const dirOf = (e: Edge): Dir => (e === 'left' || e === 'right' ? 'row' : 'column');
const first = (e: Edge) => e === 'left' || e === 'top';

function replaceLeaf(t: TileNode, app: AppId, f: (l: TileNode) => TileNode): TileNode {
  if (t.kind === 'leaf') return t.app === app ? f(t) : t;
  return { ...t, a: replaceLeaf(t.a, app, f), b: replaceLeaf(t.b, app, f) };
}

/** ドロップを木に適用する。自分自身へのドロップなど意味のないものは null */
export function applyDrop(t: TileNode, app: AppId, drop: Drop): TileNode | null {
  if (drop.kind === 'swap') {
    if (drop.target === app) return null;
    const swap = (n: TileNode): TileNode =>
      n.kind === 'leaf' ? (n.app === app ? leaf(drop.target) : n.app === drop.target ? leaf(app) : n) : { ...n, a: swap(n.a), b: swap(n.b) };
    return swap(t);
  }
  if (drop.target === app) return null;
  const rest = removeLeaf(t, app);
  if (!rest) return null;
  const pair = (other: TileNode): TileNode => {
    const moved = leaf(app);
    return { kind: 'split', dir: dirOf(drop.edge), ratio: 0.5, a: first(drop.edge) ? moved : other, b: first(drop.edge) ? other : moved };
  };
  return drop.target === null ? pair(rest) : replaceLeaf(rest, drop.target, pair);
}

export function setRatio(t: TileNode, path: number[], ratio: number): TileNode {
  if (t.kind === 'leaf') return t;
  if (path.length === 0) return { ...t, ratio };
  const [h, ...rest] = path;
  return h === 0 ? { ...t, a: setRatio(t.a, rest, ratio) } : { ...t, b: setRatio(t.b, rest, ratio) };
}

/** ドロップの候補の帯の幅 */
export const ROOT_BAND = 28;
const PANE_ZONE = 0.28;

/**
 * ポインタの位置からドロップの候補を決める。
 * ワークスペースの外周の帯 → 全体の端に入れる。ペーンの中 → 端の 28% ならその向きに分割、中央なら入れ替え。
 */
export function hitDrop(t: TileNode, area: Rect, gap: number, dragged: AppId, x: number, y: number): { drop: Drop; preview: Rect } | null {
  if (x < area.x || y < area.y || x > area.x + area.w || y > area.y + area.h) return null;
  const half = (r: Rect, e: Edge): Rect => {
    const w = (r.w - gap) / 2;
    const h = (r.h - gap) / 2;
    if (e === 'left') return { x: r.x, y: r.y, w, h: r.h };
    if (e === 'right') return { x: r.x + r.w - w, y: r.y, w, h: r.h };
    if (e === 'top') return { x: r.x, y: r.y, w: r.w, h };
    return { x: r.x, y: r.y + r.h - h, w: r.w, h };
  };
  const nearest = (r: Rect, band: number): Edge | null => {
    const d: [Edge, number][] = [
      ['left', x - r.x],
      ['right', r.x + r.w - x],
      ['top', y - r.y],
      ['bottom', r.y + r.h - y],
    ];
    d.sort((p, q) => p[1] - q[1]);
    return d[0][1] <= band ? d[0][0] : null;
  };
  if (leaves(t).length > 1) {
    const e = nearest(area, ROOT_BAND);
    if (e) return { drop: { kind: 'split', target: null, edge: e }, preview: half(area, e) };
  }
  const { tiles } = computeRects(t, area, gap);
  for (const [app, r] of tiles) {
    if (x < r.x || y < r.y || x > r.x + r.w || y > r.y + r.h) continue;
    if (app === dragged) return null;
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
    return { drop: { kind: 'swap', target: app }, preview: r };
  }
  return null;
}

export function describeDrop(d: Drop): string {
  const e = d.kind === 'split' ? { left: '左', right: '右', top: '上', bottom: '下' }[d.edge] : '';
  if (d.kind === 'swap') return '入れ替え';
  return d.target === null ? `全体の${e}に配置` : `${e}に分割`;
}
