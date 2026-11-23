// ネイティブのホストのパネル: ブラウザでは DOM のパネルが受け持つ「人だけが答える」口 (許可の要求・agent の提案・持ち込み) を、
// ホストが自分で描く。shell の描画命令のあと (窓より上)、shell に渡す幅の外 (右端) に描くので、アプリはここに描けない。
// キーボード: F6 でパネルへ移り、Tab / Shift+Tab でボタンを移り、Enter / Space で押す。F6 / Escape で戻る。
import type { DrawOp, Measure, Rect } from '../src/protocol.ts';

export const PANEL_W = 320;
const PAD = 14;

export interface PanelState {
  workspace: 1 | 2;
  merges: { id: string; by: string; intent: string; changes: { app: string; key: string }[] }[];
  prompts: { id: string; text: string }[];
  proposals: { id: string; intent: string; steps: string[]; sensitive: boolean }[];
  status: string[];
}

export interface PanelButton {
  /** 押したときの動作 ('ws:1' / 'grant:<id>:1' / 'merge:<id>:0' / 'prop:<id>:1' ...) */
  action: string;
  label: string;
  rect: Rect;
}

export interface PanelFrame {
  ops: DrawOp[];
  buttons: PanelButton[];
}

/** 幅に収まるように、文字単位で折り返す (最大 maxLines 行。超えたら … で切る) */
export function wrap(str: string, w: number, size: number, measure: Measure, maxLines = 3): string[] {
  const out: string[] = [];
  let line = '';
  for (const ch of str.replace(/\s+/g, ' ')) {
    if (line && measure(line + ch, size, 400) > w) {
      out.push(line);
      line = ch === ' ' ? '' : ch;
      if (out.length === maxLines) break;
    } else line += ch;
  }
  if (out.length < maxLines && line) out.push(line);
  else if (out.length === maxLines && line) {
    let last = out[maxLines - 1];
    while (last && measure(last + '…', size, 400) > w) last = [...last].slice(0, -1).join('');
    out[maxLines - 1] = last + '…';
  }
  return out;
}

export function drawPanel(s: PanelState, x0: number, h: number, measure: Measure, focus: number | null): PanelFrame {
  const ops: DrawOp[] = [];
  const buttons: PanelButton[] = [];
  const w = PANEL_W;
  const inner = w - PAD * 2;
  ops.push(['rect', { x: x0, y: 0, w, h, fill: 'surface' }]);
  ops.push(['rect', { x: x0, y: 0, w: 1, h, fill: 'border' }]);
  ops.push(['clip', { x: x0, y: 0, w, h }]);
  let y = PAD;
  const text = (str: string, size: number, weight: number, color: 'text' | 'muted', x = x0 + PAD) => {
    ops.push(['text', { x, y: y + size * 0.7, str, size, weight, color }]);
    y += Math.round(size * 1.5);
  };
  const lines = (str: string, size: number, color: 'text' | 'muted', max = 3) => {
    for (const l of wrap(str, inner - 20, size, measure, max)) text(l, size, 400, color, x0 + PAD + 10);
  };
  /** ボタンを横に並べる */
  const row = (items: { action: string; label: string; primary?: boolean; pressed?: boolean }[], x = x0 + PAD) => {
    let bx = x;
    for (const it of items) {
      const bw = Math.ceil(measure(it.label, 13, 600)) + 22;
      const r = { x: bx, y, w: bw, h: 28 };
      const filled = it.primary || it.pressed;
      ops.push(['rect', { ...r, radius: 6, fill: filled ? 'accent' : 'surface', stroke: filled ? undefined : 'border' }]);
      ops.push(['text', { x: r.x + 11, y: r.y + 14, str: it.label, size: 13, weight: 600, color: filled ? 'accentText' : 'text' }]);
      if (focus === buttons.length) ops.push(['rect', { x: r.x - 3, y: r.y - 3, w: r.w + 6, h: r.h + 6, radius: 8, stroke: 'accent', lineWidth: 2 }]);
      buttons.push({ action: it.action, label: it.label, rect: r });
      bx += bw + 8;
    }
    y += 28 + 10;
  };
  /** 枠を描く (中身を描き終えてから高さが決まるので、あとから枠の命令を差し込む) */
  const card = (danger: boolean, body: () => void) => {
    const at = ops.length;
    const top = y;
    y += 10;
    body();
    const r = { x: x0 + PAD - 2, y: top, w: inner + 4, h: y - top };
    ops.splice(at, 0, ['rect', { ...r, radius: 8, stroke: danger ? 'danger' : 'border', lineWidth: danger ? 1.5 : 1 }]);
    y += 10;
  };

  text('システム (ホストの UI)', 13, 600, 'text');
  y += 4;
  row([
    { action: 'ws:1', label: '人 (Alt+1)', pressed: s.workspace === 1 },
    { action: 'ws:2', label: 'agent (Alt+2)', pressed: s.workspace === 2 },
  ]);

  for (const m of s.merges)
    card(true, () => {
      text(`持ち込みの提案 ${m.id} (${m.by === 'agent' ? 'agent' : '人'})`, 13, 600, 'text', x0 + PAD + 10);
      lines(m.intent, 12, 'muted', 2);
      for (const c of m.changes.slice(0, 4)) lines(`${c.app} / ${c.key}`, 12, 'text', 1);
      if (m.changes.length > 4) lines(`ほか ${m.changes.length - 4} 件`, 12, 'muted', 1);
      row([{ action: `merge:${m.id}:1`, label: '人のワークスペースに書く', primary: true }, { action: `merge:${m.id}:0`, label: '却下' }], x0 + PAD + 10);
    });
  for (const p of s.prompts)
    card(true, () => {
      lines(p.text, 13, 'text', 3);
      row([{ action: `grant:${p.id}:1`, label: '許可', primary: true }, { action: `grant:${p.id}:0`, label: '拒否' }], x0 + PAD + 10);
    });
  for (const p of s.proposals)
    card(p.sensitive, () => {
      text(`AI の提案 ${p.id}${p.sensitive ? ' (sensitive)' : ''}`, 13, 600, 'text', x0 + PAD + 10);
      lines(p.intent, 13, 'text', 2);
      p.steps.slice(0, 5).forEach((st, i) => lines(`${i + 1}. ${st}`, 12, 'muted', 1));
      if (p.steps.length > 5) lines(`ほか ${p.steps.length - 5} 手順`, 12, 'muted', 1);
      row([{ action: `prop:${p.id}:1`, label: '承認', primary: true }, { action: `prop:${p.id}:0`, label: '却下' }], x0 + PAD + 10);
    });
  if (!s.merges.length && !s.prompts.length && !s.proposals.length) text('答えを待っているものは無い', 12, 400, 'muted');
  y += 6;
  for (const l of s.status) text(l, 12, 400, 'muted');
  text('F6 でパネルへ。Tab で移り、Enter で押す', 12, 400, 'muted');
  ops.push(['pop']);
  return { ops, buttons };
}
