// LayoutOp を sway (i3 互換の IPC) のコマンドに写し、sway の木を TileNode に読み戻す。DOM にも sway にも依存しない純粋な関数。
// 「タイル型 WM への自動操作」の抽象が、実在の WM に写せるかを確かめるためのもの (e2e/sway-bridge.ts)。
import type { LayoutOp } from './shell.ts';
import type { AppId, Rect } from './protocol.ts';
import { leaf } from './tiling.ts';
import type { Dir, TileNode } from './tiling.ts';

/** swaymsg -t get_tree の節点 (使う所だけ) */
export interface SwayNode {
  id: number;
  type: string;
  name?: string | null;
  layout?: string;
  app_id?: string | null;
  percent?: number | null;
  rect: { x: number; y: number; width: number; height: number };
  focus?: number[];
  fullscreen_mode?: number;
  nodes: SwayNode[];
  floating_nodes?: SwayNode[];
}

export interface SwayState {
  tiles: TileNode | null;
  minimized: AppId[];
  zoomed: AppId | null;
  /** 窓の矩形 (sway の実測) */
  rects: Map<AppId, Rect>;
  /** ワークスペースの領域 */
  area: Rect;
  /** app_id -> con_id */
  ids: Map<AppId, number>;
}

const walk = (n: SwayNode, f: (n: SwayNode) => void) => {
  f(n);
  n.nodes.forEach((c) => walk(c, f));
  n.floating_nodes?.forEach((c) => walk(c, f));
};

/** sway の木を読む。ワークスペースは名前で選ぶ */
export function readSway(root: SwayNode, workspace = '1'): SwayState {
  let ws: SwayNode | null = null;
  let scratch: SwayNode | null = null;
  walk(root, (n) => {
    if (n.type === 'workspace' && n.name === workspace) ws = n;
    if (n.type === 'workspace' && n.name === '__i3_scratch') scratch = n;
  });
  if (!ws) throw new Error(`workspace ${workspace} がない`);
  const w = ws as SwayNode;
  const ids = new Map<AppId, number>();
  const rects = new Map<AppId, Rect>();
  let zoomed: AppId | null = null;
  walk(root, (n) => {
    if (n.app_id) {
      ids.set(n.app_id, n.id);
      if (n.fullscreen_mode) zoomed = n.app_id;
    }
  });
  const conv = (n: SwayNode): TileNode | null => {
    if (n.app_id) {
      rects.set(n.app_id, { x: n.rect.x, y: n.rect.y, w: n.rect.width, h: n.rect.height });
      return leaf(n.app_id);
    }
    const kids = n.nodes.map(conv).filter((k): k is TileNode => !!k);
    if (kids.length === 0) return null;
    if (n.layout === 'tabbed' || n.layout === 'stacked') {
      const apps = kids.flatMap((k) => (k.kind === 'leaf' ? (k.tabs ?? [k.app]) : []));
      const shownId = n.focus?.[0];
      const shown = n.nodes.find((c) => c.id === shownId)?.app_id ?? apps[0];
      for (const c of n.nodes) if (c.app_id) rects.set(c.app_id, { x: n.rect.x, y: n.rect.y, w: n.rect.width, h: n.rect.height });
      return apps.length > 1 ? { kind: 'leaf', app: shown, tabs: apps } : leaf(apps[0]);
    }
    if (kids.length === 1) return kids[0];
    const dir: Dir = n.layout === 'splitv' ? 'column' : 'row';
    const live = n.nodes.filter((c) => conv0(c));
    // 全画面の子は percent が 2.0 などになる。ほかの子の残りを割り当てる
    const others = live.filter((c) => !c.fullscreen_mode).reduce((a, c) => a + (c.percent ?? 1 / kids.length), 0);
    const pct = live.map((c) => (c.fullscreen_mode ? Math.max(0.05, 1 - others) : (c.percent ?? 1 / kids.length)));
    // n 分木を右に入れ子の 2 分木にする。比率は sway の percent から
    const build = (i: number): TileNode => {
      if (i === kids.length - 1) return kids[i];
      const rest = pct.slice(i).reduce((a, b) => a + b, 0);
      return { kind: 'split', dir, ratio: pct[i] / rest, a: kids[i], b: build(i + 1) };
    };
    return build(0);
  };
  const conv0 = (n: SwayNode): boolean => !!n.app_id || n.nodes.some(conv0);
  const minimized: AppId[] = [];
  if (scratch) walk(scratch as SwayNode, (n) => n.app_id && minimized.push(n.app_id));
  const tiles = w.nodes.length ? conv({ ...w, layout: w.layout }) : null;
  return { tiles, minimized, zoomed, rects, area: { x: w.rect.x, y: w.rect.y, w: w.rect.width, h: w.rect.height }, ids };
}

/** 比べるための正規化: 同じ向きの入れ子を平らにした n 分木 (比率は捨てる) */
export type Shape = string | { dir: Dir; children: Shape[] } | { tabs: string[]; shown: string };
export function shape(t: TileNode | null): Shape | null {
  if (!t) return null;
  if (t.kind === 'leaf') return t.tabs ? { tabs: [...t.tabs].sort(), shown: t.app } : t.app;
  const flat = (n: TileNode): Shape[] => (n.kind === 'split' && n.dir === t.dir ? [...flat(n.a), ...flat(n.b)] : [shape(n)!]);
  return { dir: t.dir, children: [...flat(t.a), ...flat(t.b)] };
}

/** 葉の並び (左上から) */
const leavesOf = (t: TileNode): AppId[] => (t.kind === 'leaf' ? [t.app] : [...leavesOf(t.a), ...leavesOf(t.b)]);
const firstLeaf = (t: TileNode): AppId => leavesOf(t)[0];
const lastLeaf = (t: TileNode): AppId => leavesOf(t).at(-1)!;

function nodeAt(t: TileNode, path: number[]): TileNode | null {
  let n: TileNode = t;
  for (const h of path) {
    if (n.kind !== 'split') return null;
    n = h === 0 ? n.a : n.b;
  }
  return n;
}

/**
 * LayoutOp を swaymsg のコマンド列に写す。写せないものは unsupported と理由を返す。
 * 方針: 対象を mark し、`move container to mark` で隣に置き、必要なら swap で左右 (上下) を入れ替える。
 */
export function toSwayCommands(op: LayoutOp, st: SwayState): { commands: string[] } | { unsupported: string } {
  const id = (a: AppId) => {
    const v = st.ids.get(a);
    if (v === undefined) throw new Error(`sway に ${a} がない`);
    return v;
  };
  const M = '_bridge_target';
  switch (op.type) {
    case 'zoom':
      return { commands: op.app ? [`[con_id=${id(op.app)}] fullscreen enable`] : st.zoomed ? [`[con_id=${id(st.zoomed)}] fullscreen disable`] : [] };
    case 'minimize':
      return { commands: [`[con_id=${id(op.app)}] move scratchpad`] };
    case 'restore':
      // scratchpad から出した窓は浮いた状態で出る。タイルに戻すと、フォーカスのある窓の隣に入る
      return { commands: [`[con_id=${id(op.app)}] scratchpad show`, `[con_id=${id(op.app)}] floating disable`] };
    case 'equalize':
      // sway (i3) に「すべてを均等にする」コマンドはない。構造は変えず、大きさだけを送る (sizeCommands は呼び出し側が送る)
      return { commands: [] };
    case 'move': {
      const a = id(op.app);
      const d = op.drop;
      switch (d.kind) {
        case 'swap':
          return { commands: [`[con_id=${a}] swap container with con_id ${id(d.target)}`] };
        case 'split': {
          if (d.target === null) return { unsupported: 'ワークスペース全体の端に置く操作は、i3/sway のコマンド 1 つには対応しない (ワークスペースの子を包み直す必要がある)' };
          const t = id(d.target);
          const split = d.edge === 'left' || d.edge === 'right' ? 'splith' : 'splitv';
          const cmds = [`[con_id=${t}] mark --replace ${M}`, `[con_id=${t}] ${split}`, `[con_id=${a}] move container to mark ${M}`, `unmark ${M}`];
          if (d.edge === 'left' || d.edge === 'top') cmds.splice(3, 0, `[con_id=${a}] swap container with con_id ${t}`);
          return { commands: cmds };
        }
        case 'tab': {
          const t = id(d.target);
          return {
            commands: [`[con_id=${t}] mark --replace ${M}`, `[con_id=${t}] splith`, `[con_id=${a}] move container to mark ${M}`, `unmark ${M}`, `[con_id=${t}] focus`, 'layout tabbed', `[con_id=${a}] focus`],
          };
        }
        case 'insert': {
          if (!st.tiles) return { unsupported: '木が空' };
          const n = nodeAt(st.tiles, d.path);
          if (!n || n.kind !== 'split') return { unsupported: '差し込む境界がない' };
          // a の最後の窓の後ろに置く。sway は n 分木なので、同じ向きの容器なら a と b の間に入る
          const after = lastLeaf(n.a);
          void firstLeaf;
          return { commands: [`[con_id=${id(after)}] mark --replace ${M}`, `[con_id=${a}] move container to mark ${M}`, `unmark ${M}`] };
        }
      }
    }
  }
}

/** 木に入っている全ての窓 (タブの隠れた窓も、左上から) */
const anchorsOf = (t: TileNode): AppId[] => (t.kind === 'leaf' ? (t.tabs ?? [t.app]) : [...anchorsOf(t.a), ...anchorsOf(t.b)]);

/**
 * 宣言的方式: モデルの状態をそのまま sway に作り直すコマンド列。
 * 全ての窓を一度 scratchpad に入れ、木の順に「フォーカスした窓の隣にタイルとして出す」を繰り返す。
 * i3/sway の split は子が 1 つの容器を残し、move to mark は容器の中に入るので、操作を 1 つずつ写すと隠れた構造に左右される。
 * 作り直せばその影響を受けない (代わりに、全ての窓が一瞬動く)。
 */
export function realizeCommands(target: { tiles: TileNode | null; minimized: AppId[]; zoomed: AppId | null }, st: SwayState): string[] {
  const id = (a: AppId) => {
    const v = st.ids.get(a);
    if (v === undefined) throw new Error(`sway に ${a} がない`);
    return v;
  };
  const cmds: string[] = [];
  if (st.zoomed) cmds.push(`[con_id=${id(st.zoomed)}] fullscreen disable`);
  for (const a of st.ids.keys()) if (!st.minimized.includes(a)) cmds.push(`[con_id=${id(a)}] move scratchpad`);
  if (!target.tiles) return cmds;
  const show = (f: AppId, after: AppId | null) => {
    if (after) cmds.push(`[con_id=${id(after)}] focus`);
    cmds.push(`[con_id=${id(f)}] scratchpad show`, `[con_id=${id(f)}] floating disable`);
  };
  const build = (n: TileNode, e: AppId) => {
    if (n.kind === 'leaf') {
      if (!n.tabs) return;
      cmds.push(`[con_id=${id(e)}] splith`, `[con_id=${id(e)}] layout tabbed`);
      let prev = e;
      for (const t of n.tabs.slice(1)) {
        show(t, prev);
        prev = t;
      }
      cmds.push(`[con_id=${id(n.app)}] focus`);
      return;
    }
    const f = anchorsOf(n.b)[0];
    cmds.push(`[con_id=${id(e)}] ${n.dir === 'row' ? 'splith' : 'splitv'}`);
    show(f, e);
    build(n.a, e);
    build(n.b, f);
  };
  const first = anchorsOf(target.tiles)[0];
  show(first, null);
  build(target.tiles, first);
  if (target.zoomed) cmds.push(`[con_id=${id(target.zoomed)}] fullscreen enable`);
  return cmds;
}

/** 窓の大きさをモデルに合わせるコマンド列 (sway は空いた場所を均等に配り直すので、比率はこちらから送る) */
export function sizeCommands(rects: Map<AppId, Rect>, st: SwayState): string[] {
  const out: string[] = [];
  for (const [app, r] of rects) {
    const v = st.ids.get(app);
    if (v !== undefined) out.push(`[con_id=${v}] resize set width ${Math.round(r.w)} px height ${Math.round(r.h)} px`);
  }
  return out;
}
