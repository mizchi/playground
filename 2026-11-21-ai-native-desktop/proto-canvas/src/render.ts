// LaidOut -> P4b display list + hit test 用の表
import type { WidgetKind } from './catalog.ts';
import { M } from './layout.ts';
import type { LaidOut } from './layout.ts';
import type { DrawOp, NodeId, Rect } from './protocol.ts';

export interface HitRegion {
  rect: Rect;
  node: NodeId;
  widget: WidgetKind;
}

export interface PaintState {
  focus: NodeId | null;
  /** 確定前の IME 文字列 (tree には入れない) */
  preedit: string;
  /** agent の提案のプレビュー: ノード -> 手順番号 */
  ghosts: Map<NodeId, number>;
}

export interface Painted {
  ops: DrawOp[];
  hits: HitRegion[];
  /** フォーカス中の textField のキャレット (IME の候補ウィンドウ位置) */
  caret: Rect | null;
}

export function paint(root: LaidOut, state: PaintState, measure: (s: string, size: number, weight: number) => number): Painted {
  const ops: DrawOp[] = [];
  const hits: HitRegion[] = [];
  let caret: Rect | null = null;

  const visit = (l: LaidOut) => {
    const v = l.view;
    const r = l.rect;
    if (v.kind === 'box') {
      if (v.style.background || v.style.border)
        ops.push(['rect', { ...r, fill: v.style.background, stroke: v.style.border, radius: v.style.radius ?? 0 }]);
      l.children.forEach(visit);
      return;
    }
    if (v.kind === 'text') {
      ops.push(['text', { x: r.x, y: r.y + r.h / 2, str: v.str, size: v.size, weight: v.weight, color: v.color }]);
      return;
    }
    const n = v.node;
    const label = n.label ?? '';
    const cy = r.y + r.h / 2;
    let hitRect: Rect = r;
    switch (v.widget) {
      case 'checkbox': {
        const box = { x: r.x, y: cy - M.check / 2, w: M.check, h: M.check };
        ops.push(['rect', { ...box, radius: 4, fill: n.checked ? 'accent' : 'surface', stroke: n.checked ? 'accent' : 'border', lineWidth: 1.5 }]);
        if (n.checked) ops.push(['text', { x: box.x + 4, y: cy, str: '✓', size: 13, weight: 700, color: 'accentText' }]);
        ops.push(['text', { x: r.x + M.check + M.labelGap, y: cy, str: label, size: M.fontSize, color: n.checked ? 'muted' : 'text' }]);
        hitRect = { x: r.x, y: r.y, w: Math.min(r.w, M.check + M.labelGap + measure(label, M.fontSize, 400)), h: r.h };
        break;
      }
      case 'switch': {
        const track = { x: r.x, y: cy - M.switchH / 2, w: M.switchW, h: M.switchH };
        ops.push(['rect', { ...track, radius: M.switchH / 2, fill: n.checked ? (n.sensitive ? 'danger' : 'accent') : 'border' }]);
        ops.push(['rect', { x: track.x + (n.checked ? M.switchW - 18 : 2), y: track.y + 2, w: 16, h: 16, radius: 8, fill: 'surface' }]);
        ops.push(['text', { x: r.x + M.switchW + M.labelGap, y: cy, str: label, size: M.fontSize, color: 'text' }]);
        hitRect = { x: r.x, y: r.y, w: Math.min(r.w, M.switchW + M.labelGap + measure(label, M.fontSize, 400)), h: r.h };
        break;
      }
      case 'textField': {
        const focused = state.focus === n.id;
        ops.push(['rect', { ...r, radius: 6, fill: 'surface', stroke: focused ? 'accent' : 'border', lineWidth: focused ? 2 : 1 }]);
        const value = n.value ?? '';
        const tx = r.x + 10;
        ops.push(['clip', { x: r.x + 2, y: r.y, w: r.w - 4, h: r.h }]);
        if (value === '' && !(focused && state.preedit)) ops.push(['text', { x: tx, y: cy, str: label, size: M.fontSize, color: 'muted' }]);
        else ops.push(['text', { x: tx, y: cy, str: value, size: M.fontSize, color: 'text' }]);
        let cx = tx + (value ? measure(value, M.fontSize, 400) : 0);
        if (focused && state.preedit) {
          ops.push(['text', { x: cx, y: cy, str: state.preedit, size: M.fontSize, color: 'accent', underline: true }]);
          cx += measure(state.preedit, M.fontSize, 400);
        }
        if (focused) {
          caret = { x: cx, y: cy - 9, w: 1.5, h: 18 };
          ops.push(['rect', { ...caret, fill: 'text' }]);
        }
        ops.push(['pop']);
        break;
      }
      case 'button': {
        const w = Math.ceil(measure(label, M.fontSize, 600)) + 2 * M.btnPadX;
        hitRect = { x: r.x, y: r.y, w, h: r.h };
        ops.push(['rect', { ...hitRect, radius: 6, fill: n.sensitive ? 'danger' : 'accent' }]);
        ops.push(['text', { x: r.x + M.btnPadX, y: cy, str: label, size: M.fontSize, weight: 600, color: 'accentText' }]);
        break;
      }
    }
    if (state.focus === n.id && v.widget !== 'textField')
      ops.push(['rect', { x: hitRect.x - 3, y: hitRect.y - 1, w: hitRect.w + 6, h: hitRect.h + 2, radius: 6, stroke: 'accent', lineWidth: 2 }]);
    const step = state.ghosts.get(n.id);
    if (step !== undefined) {
      ops.push(['rect', { x: hitRect.x - 5, y: hitRect.y - 3, w: hitRect.w + 10, h: hitRect.h + 6, radius: 8, stroke: 'ghost', dash: true, lineWidth: 2 }]);
      ops.push(['rect', { x: hitRect.x + hitRect.w - 4, y: hitRect.y - 10, w: 18, h: 18, radius: 9, fill: 'ghost' }]);
      ops.push(['text', { x: hitRect.x + hitRect.w + 1, y: hitRect.y - 1, str: String(step), size: 11, weight: 700, color: 'accentText' }]);
    }
    hits.push({ rect: hitRect, node: n.id, widget: v.widget });
  };
  visit(root);
  return { ops, hits, caret };
}
