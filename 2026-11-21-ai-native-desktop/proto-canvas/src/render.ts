// LaidOut -> P4b display list + hit test 用の表
import type { WidgetKind } from './catalog.ts';
import { M } from './layout.ts';
import type { LaidOut } from './layout.ts';
import type { ActionName, DrawOp, NodeId, Rect } from './protocol.ts';
import { stepperValue } from './layout.ts';

export interface HitRegion {
  rect: Rect;
  node: NodeId;
  widget: WidgetKind;
  /** 押したときの action (ステッパーの [−] [+] など)。無ければ click */
  action?: ActionName;
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

/** visible を渡すと、その外にある葉 (文字・widget) は描かず、hit test にも入れない (スクロールした窓の画面外) */
export function paint(root: LaidOut, state: PaintState, measure: (s: string, size: number, weight: number) => number, visible?: Rect): Painted {
  const off = (r: Rect) => !!visible && (r.y + r.h < visible.y || r.y > visible.y + visible.h || r.x + r.w < visible.x || r.x > visible.x + visible.w);
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
    if (off(r)) return; // 画面外の葉は描かない
    if (v.kind === 'text') {
      // 等幅の行で広げられた文字は中央に置く (それ以外は r.w が固有幅なので左寄せと同じ)
      const tw = measure(v.str, v.size, v.weight);
      ops.push(['text', { x: r.x + Math.max(0, (r.w - tw) / 2), y: r.y + r.h / 2, str: v.str, size: v.size, weight: v.weight, color: v.color }]);
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
      case 'radio': {
        const box = { x: r.x, y: cy - M.check / 2, w: M.check, h: M.check };
        ops.push(['rect', { ...box, radius: M.check / 2, fill: 'surface', stroke: n.checked ? 'accent' : 'border', lineWidth: 1.5 }]);
        if (n.checked) ops.push(['rect', { x: box.x + 5, y: box.y + 5, w: 8, h: 8, radius: 4, fill: 'accent' }]);
        ops.push(['text', { x: r.x + M.check + M.labelGap, y: cy, str: label, size: M.fontSize, color: 'text' }]);
        hitRect = { x: r.x, y: r.y, w: Math.min(r.w, M.check + M.labelGap + measure(label, M.fontSize, 400)), h: r.h };
        break;
      }
      case 'stepper': {
        // ラベル [−] 値 [+] : [−] [+] はそれぞれ decrement / increment
        ops.push(['text', { x: r.x, y: cy, str: label, size: M.fontSize, color: 'text' }]);
        const bx = r.x + Math.ceil(measure(label, M.fontSize, 400)) + M.labelGap;
        const val = stepperValue(n);
        const minus = { x: bx, y: cy - M.stepBtn / 2, w: M.stepBtn, h: M.stepBtn };
        const vx = bx + M.stepBtn + M.labelGap;
        const plus = { x: vx + Math.ceil(measure(val, M.fontSize, 600)) + M.labelGap, y: minus.y, w: M.stepBtn, h: M.stepBtn };
        for (const [b, s] of [[minus, '−'], [plus, '+']] as const) {
          ops.push(['rect', { ...b, radius: 6, fill: 'surface', stroke: 'border' }]);
          ops.push(['text', { x: b.x + 7, y: cy, str: s, size: 14, weight: 700, color: 'text' }]);
        }
        ops.push(['text', { x: vx, y: cy, str: val, size: M.fontSize, weight: 600, color: 'text' }]);
        if (n.actions?.includes('decrement')) hits.push({ rect: minus, node: n.id, widget: 'stepper', action: 'decrement' });
        if (n.actions?.includes('increment')) hits.push({ rect: plus, node: n.id, widget: 'stepper', action: 'increment' });
        hitRect = { x: r.x, y: r.y, w: plus.x + plus.w - r.x, h: r.h };
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
        // 入力が欄より長いときは、キャレット (末尾) が見えるように文字を左へ送る
        const endW = (value ? measure(value, M.fontSize, 400) : 0) + (focused && state.preedit ? measure(state.preedit, M.fontSize, 400) : 0);
        const tx = r.x + 10 - Math.max(0, endW - (r.w - 22));
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
      case 'terminal': {
        const s = n.screen;
        ops.push(['rect', { ...r, radius: 6, rgb: s?.bg ?? '#000000', fill: 'surface' }]);
        if (!s) break;
        const x0 = r.x + M.termPad;
        const y0 = r.y + M.termPad;
        ops.push(['clip', { x: r.x + 2, y: r.y + 2, w: r.w - 4, h: r.h - 4 }]);
        s.lines.forEach((line, row) => {
          const y = y0 + row * M.termLine;
          for (const run of line) {
            const cells = run.wide ? 2 : [...run.text].length;
            const x = x0 + run.col * M.termCell;
            if (x > r.x + r.w) break;
            if (run.bg) ops.push(['rect', { x, y, w: cells * M.termCell, h: M.termLine, rgb: run.bg, fill: 'surface' }]);
            // 1 桁ずつ置く (フォントの字幅に頼らず、格子に合わせる)
            [...run.text].forEach((ch, i) => {
              if (ch !== ' ')
                ops.push(['text', { x: x + i * M.termCell, y: y + M.termLine / 2, str: ch, size: M.termSize, weight: run.bold ? 700 : 400, color: 'text', rgb: run.fg ?? s.fg, mono: true }]);
            });
          }
        });
        if (s.cursor) {
          ops.push(['layer', { alpha: 0.55 }]);
          ops.push(['rect', { x: x0 + s.cursor.x * M.termCell, y: y0 + s.cursor.y * M.termLine, w: M.termCell, h: M.termLine, rgb: s.fg, fill: 'text' }]);
          ops.push(['pop']);
        }
        ops.push(['pop']);
        break;
      }
      case 'action':
      case 'button': {
        // 等幅の行で広げられたら、その幅いっぱいに描き、文字は中央 (それ以外は r.w が固有幅)
        const tw = Math.ceil(measure(label, M.fontSize, 600));
        const w = Math.max(tw + 2 * M.btnPadX, r.w);
        hitRect = { x: r.x, y: r.y, w, h: r.h };
        // tab は選ばれていなければ控えめに描く。action (クリックできるセルなど) は選ばれていれば強調する
        const quiet = (n.role === 'tab' && !n.selected) || (v.widget === 'action' && !n.selected);
        ops.push(['rect', { ...hitRect, radius: 6, fill: quiet ? 'surface' : n.sensitive ? 'danger' : 'accent', stroke: quiet ? 'border' : undefined }]);
        ops.push(['text', { x: r.x + (w - tw) / 2, y: cy, str: label, size: M.fontSize, weight: 600, color: quiet ? 'text' : 'accentText' }]);
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
    if (v.widget !== 'stepper') hits.push({ rect: hitRect, node: n.id, widget: v.widget });
  };
  visit(root);
  return { ops, hits, caret };
}
