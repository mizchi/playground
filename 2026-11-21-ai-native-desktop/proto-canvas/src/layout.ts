// portable な flex 部分集合の最小レイアウト。後で crater-layout / Taffy に置き換え、veriss で同じ意味を証明する。
import type { ViewNode } from './expand.ts';
import type { Measure, Rect } from './protocol.ts';

export const M = {
  check: 18,
  rowH: 26,
  fieldH: 32,
  fieldMinW: 160,
  btnPadX: 14,
  btnH: 32,
  switchW: 36,
  switchH: 20,
  labelGap: 8,
  fontSize: 14,
  stepBtn: 24,
  /** ターミナル: 1 桁の幅・1 行の高さ・文字の大きさ・内側の余白・最小の幅 (はみ出す桁は切り取る) */
  termCell: 8,
  termLine: 17,
  termSize: 13,
  termPad: 8,
  termMinW: 240,
} as const;

/** slider / spinbutton の表示値 */
export function stepperValue(n: { valueNow?: number; value?: string }): string {
  return n.valueNow !== undefined ? String(Math.round(n.valueNow * 100) / 100) : (n.value ?? '');
}

export interface LaidOut {
  view: ViewNode;
  rect: Rect;
  children: LaidOut[];
}

export interface Overflow {
  /** はみ出した box の矩形 */
  rect: Rect;
  required: number;
  available: number;
}

export interface LayoutResult {
  root: LaidOut;
  /** はみ出さずに描ける最小の幅 (= tile が守るべき下限) */
  minWidth: number;
  height: number;
  overflow: Overflow[];
}

const lineH = (size: number) => Math.ceil(size * 1.4);

export function layout(view: ViewNode, width: number, measure: Measure, origin = { x: 0, y: 0 }): LayoutResult {
  const cache = new Map<ViewNode, { w: number; h: number }>();
  const overflow: Overflow[] = [];

  const intrinsic = (v: ViewNode): { w: number; h: number } => {
    const hit = cache.get(v);
    if (hit) return hit;
    let r: { w: number; h: number };
    if (v.kind === 'text') r = { w: Math.ceil(measure(v.str, v.size, v.weight)), h: lineH(v.size) };
    else if (v.kind === 'widget') {
      const label = v.node.label ?? '';
      switch (v.widget) {
        case 'checkbox':
        case 'radio':
          r = { w: M.check + M.labelGap + Math.ceil(measure(label, M.fontSize, 400)), h: M.rowH };
          break;
        case 'stepper':
          r = { w: Math.ceil(measure(label, M.fontSize, 400)) + M.labelGap + 2 * M.stepBtn + 2 * M.labelGap + Math.ceil(measure(stepperValue(v.node), M.fontSize, 600)), h: M.rowH };
          break;
        case 'switch':
          r = { w: M.switchW + M.labelGap + Math.ceil(measure(label, M.fontSize, 400)), h: M.rowH };
          break;
        case 'textField':
          r = { w: M.fieldMinW, h: M.fieldH };
          break;
        case 'terminal':
          // 横は窓に合わせて伸び、狭ければ右の桁を切り取る (タイルの最小幅を押し上げない)
          r = { w: M.termMinW, h: (v.node.screen?.rows ?? 1) * M.termLine + 2 * M.termPad };
          break;
        case 'button':
        case 'action':
          r = { w: Math.ceil(measure(label, M.fontSize, 600)) + 2 * M.btnPadX, h: M.btnH };
          break;
      }
    } else {
      const p = v.style.padding ?? 0;
      const gap = v.style.gap ?? 0;
      const kids = v.children.map(intrinsic);
      const gaps = gap * Math.max(0, kids.length - 1);
      if ((v.style.axis ?? 'vertical') === 'vertical')
        r = { w: Math.max(0, ...kids.map((k) => k.w)) + 2 * p, h: kids.reduce((s, k) => s + k.h, 0) + gaps + 2 * p };
      else if (v.style.equal) r = { w: Math.max(0, ...kids.map((k) => k.w)) * kids.length + gaps + 2 * p, h: Math.max(0, ...kids.map((k) => k.h)) + 2 * p };
      else r = { w: kids.reduce((s, k) => s + k.w, 0) + gaps + 2 * p, h: Math.max(0, ...kids.map((k) => k.h)) + 2 * p };
    }
    cache.set(v, r);
    return r;
  };

  const fills = (v: ViewNode) => v.kind === 'widget' && (v.widget === 'textField' || v.widget === 'terminal');
  const stretches = (v: ViewNode) => v.kind === 'box' || fills(v);
  const growOf = (v: ViewNode) => (v.kind === 'box' ? (v.style.grow ?? 0) : fills(v) ? 1 : 0);

  const arrange = (v: ViewNode, x: number, y: number, w: number): LaidOut => {
    const h = intrinsic(v).h;
    if (v.kind !== 'box') return { view: v, rect: { x, y, w, h }, children: [] };
    const p = v.style.padding ?? 0;
    const gap = v.style.gap ?? 0;
    const inner = w - 2 * p;
    const children: LaidOut[] = [];
    if ((v.style.axis ?? 'vertical') === 'vertical') {
      let cy = y + p;
      for (const c of v.children) {
        const ci = intrinsic(c);
        if (ci.w > inner) overflow.push({ rect: { x, y, w, h }, required: ci.w + 2 * p, available: w });
        const cw = stretches(c) ? Math.max(inner, ci.w) : ci.w;
        children.push(arrange(c, x + p, cy, cw));
        cy += ci.h + gap;
      }
    } else {
      const kids = v.children.map(intrinsic);
      if (v.style.equal && kids.length) {
        // 等幅: 使える幅を子の数で割る (固有幅の最大より狭くはしない)
        const share = Math.max(Math.max(...kids.map((k) => k.w)), (inner - gap * (kids.length - 1)) / kids.length);
        if (share * kids.length + gap * (kids.length - 1) > inner + 0.5) overflow.push({ rect: { x, y, w, h }, required: intrinsic(v).w, available: w });
        let cx = x + p;
        v.children.forEach((c, i) => {
          children.push(arrange(c, cx, y + p + (h - 2 * p - kids[i].h) / 2, share));
          cx += share + gap;
        });
        return { view: v, rect: { x, y, w, h }, children };
      }
      const used = kids.reduce((s, k) => s + k.w, 0) + gap * Math.max(0, kids.length - 1);
      const free = inner - used;
      if (free < 0) overflow.push({ rect: { x, y, w, h }, required: used + 2 * p, available: w });
      const grows = v.children.map(growOf);
      const total = grows.reduce((s, g) => s + g, 0);
      let cx = x + p;
      v.children.forEach((c, i) => {
        const extra = free > 0 && total > 0 ? (free * grows[i]) / total : 0;
        const cw = kids[i].w + extra;
        const cy = y + p + (h - 2 * p - kids[i].h) / 2;
        children.push(arrange(c, cx, cy, cw));
        cx += cw + gap;
      });
    }
    return { view: v, rect: { x, y, w, h }, children };
  };

  const root = arrange(view, origin.x, origin.y, width);
  return { root, minWidth: intrinsic(view).w, height: intrinsic(view).h, overflow };
}
