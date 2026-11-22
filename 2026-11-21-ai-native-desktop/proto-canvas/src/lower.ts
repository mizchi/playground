// View Tree -> veriss の入力 (HTML + CSS + 文字の契約)。layout.ts と同じ意味になるように CSS の flex に写す。
// View Tree には selector も cascade もないので、1 ノード 1 クラスにする (veriss の ADR-0014/0015 の S1・S2 が起きない)。
import type { ViewNode } from './expand.ts';
import { M, layout, stepperValue } from './layout.ts';
import type { ColorToken, Measure } from './protocol.ts';
import { THEMES } from './theme.ts';
import type { Palette } from './theme.ts';

/** www/index.html の light テーマと同じ値 (正は theme.ts) */
export const THEME_LIGHT: Palette = THEMES.light;

export const LOWER_FONT = 'Arial, Helvetica, sans-serif';

export interface TextContract {
  minInlineSize: number;
  maxInlineSize: number;
  intrinsicBlockSize: number;
  wrap: 'nowrap';
}

export interface Lowered {
  html: string;
  css: string;
  boxes: Record<string, TextContract | { overflowBlock: 'scroll' }>;
  /** 使った「文字色 on 背景色」の組 (contrastSpecimen でまとめて検査する) */
  pairs: ColorPair[];
  /** scroll のとき veriss に渡す scrollbar の寸法 (canvas の scrollbar は中身に重ねるので 0) */
  scrollbars?: { name: string; inlineSize: number; blockSize: number }[];
}

export interface ColorPair {
  fg: ColorToken;
  bg: ColorToken;
  size: number;
  weight: number;
}

/** layout.ts と同じ規則の固有幅 */
const intrinsicWidth = (v: ViewNode, measure: Measure) => layout(v, 0, measure).minWidth;

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!);
const lineH = (size: number) => Math.ceil(size * 1.4);

export interface LowerOptions {
  /** 本体の配色 (既定はライト) */
  theme?: Palette;
  /**
   * prefers-color-scheme: dark のときの配色 (既定は theme.ts のダーク。null で出さない)。
   * 色の宣言だけを @media に複製するので、veriss は同じ配置のまま、ダークでもコントラストを検査する
   */
  dark?: Palette | null;
  /** CSS の font-family (計測に使ったフォントと同じにする) */
  fontFamily?: string;
  /** 窓の中身を縦にスクロールする容器にする (shell の窓と同じ)。中身が窓より高くても、はみ出しではなくなる */
  scroll?: boolean;
  /**
   * flex の子の固有幅を px で明示する (column の非 stretch の子は width、row の子は flex-basis)。
   * veriss が「中身で決まる幅」(shrink-to-fit、flex-basis: auto) を、文字の契約 (min = max) があっても
   * Unknown にするための迂回 (mizchi/veriss#6, #7)。layout.ts が固有幅を決定的に出せるので、意味は変わらない。
   */
  pxIntrinsic?: boolean;
}

export function lower(view: ViewNode, measure: Measure, opts: LowerOptions = {}): Lowered {
  const theme = opts.theme ?? THEME_LIGHT;
  const darkTheme = opts.dark === undefined ? THEMES.dark : opts.dark;
  const darkRules: string[] = [];
  /** 色の宣言 (token で指定)。本体の宣言を返し、ダークの宣言を @media 用に控える */
  const paint = (sel: string, decls: [string, ColorToken][]) => {
    if (darkTheme) darkRules.push(`${sel} { ${decls.map(([k, t]) => `${k}: ${darkTheme[t]};`).join(' ')} }`);
    return decls.map(([k, t]) => `${k}: ${theme[t]};`).join(' ');
  };
  const family = opts.fontFamily ?? LOWER_FONT;
  let n = 0;
  const rules: string[] = [
    '* { box-sizing: border-box; margin: 0; padding: 0; }',
    `html { overflow: hidden; font: 400 14px/20px ${family}; }`,
    'body { display: block; }',
  ];
  const boxes: Lowered['boxes'] = {};
  const text = (cls: string, str: string, size: number, weight: number) => {
    const w = Math.ceil(measure(str, size, weight));
    boxes[`.${cls}`] = { minInlineSize: w, maxInlineSize: w, intrinsicBlockSize: lineH(size), wrap: 'nowrap' };
    return `font: ${weight} ${size}px/${lineH(size)}px ${family}; white-space: nowrap;`;
  };

  /** parent: 親の向き。layout.ts の stretch / grow の規則を CSS の align-self / flex に写す */
  const pairs = new Map<string, ColorPair>();
  const pair = (fg: ColorToken, bg: ColorToken, size: number, weight: number) => pairs.set(`${fg}/${bg}/${size}/${weight}`, { fg, bg, size, weight });

  /** bg: 下に塗られている背景の token (文字のコントラストの相手) */
  const visit = (v: ViewNode, parent: 'vertical' | 'horizontal' | null, bg: ColorToken, equal = false): string => {
    const cls = `v${n++}`;
    const fills = v.kind === 'widget' && (v.widget === 'textField' || v.widget === 'terminal');
    const stretch = v.kind === 'box' || fills;
    const grow = v.kind === 'box' ? (v.style.grow ?? 0) : fills ? 1 : 0;
    let item = '';
    if (parent === 'vertical') item = `flex: none; align-self: ${stretch ? 'stretch' : 'flex-start'};`;
    if (parent === 'vertical' && !stretch && opts.pxIntrinsic) item += ` width: ${intrinsicWidth(v, measure)}px;`;
    if (parent === 'horizontal') {
      const basis = opts.pxIntrinsic ? `${intrinsicWidth(v, measure)}px` : 'auto';
      item = grow > 0 ? `flex: ${grow} 0 ${basis};` : `flex: 0 0 ${basis};`;
      // 等幅の行: 基準 0 から同じ割合で伸ばす。固有幅は下限 (layout.ts は箱の固有幅を「最大の子 × 数」にしている)
      if (equal) item = `flex: 1 0 0; min-width: ${intrinsicWidth(v, measure)}px; text-align: center;`;
    }

    if (v.kind === 'box') {
      const axis = v.style.axis ?? 'vertical';
      const fill = v.style.background ?? bg;
      rules.push(
        `.${cls} { display: flex; flex-direction: ${axis === 'vertical' ? 'column' : 'row'}; align-items: ${axis === 'vertical' ? 'flex-start' : 'center'}; gap: ${v.style.gap ?? 0}px; padding: ${v.style.padding ?? 0}px;${v.style.background ? ` ${paint(`.${cls}`, [['background', v.style.background]])}` : ''} ${item} }`,
      );
      return `<div class="${cls}">${v.children.map((c) => visit(c, axis, fill, axis === 'horizontal' && !!v.style.equal)).join('')}</div>`;
    }
    if (v.kind === 'text') {
      pair(v.color, bg, v.size, v.weight);
      rules.push(`.${cls} { ${text(cls, v.str, v.size, v.weight)} ${paint(`.${cls}`, [['color', v.color]])} ${item} }`);
      return `<div class="${cls}">${esc(v.str)}</div>`;
    }
    const node = v.node;
    const label = node.label ?? '';
    switch (v.widget) {
      case 'checkbox':
      case 'radio':
      case 'switch': {
        const [w, h] = v.widget === 'switch' ? [M.switchW, M.switchH] : [M.check, M.check];
        rules.push(`.${cls} { display: flex; align-items: center; gap: ${M.labelGap}px; height: ${M.rowH}px; ${item} }`);
        rules.push(`.${cls}-i { flex: none; width: ${w}px; height: ${h}px; ${paint(`.${cls}-i`, [['background', node.checked ? 'accent' : 'border']])} }`);
        const fg: ColorToken = node.checked && v.widget === 'checkbox' ? 'muted' : 'text';
        pair(fg, bg, M.fontSize, 400);
        rules.push(`.${cls}-l { flex: none; ${text(`${cls}-l`, label, M.fontSize, 400)} ${paint(`.${cls}-l`, [['color', fg]])} }`);
        return `<div class="${cls}" role="${v.widget}" aria-checked="${node.checked ? 'true' : 'false'}" tabindex="0"><div class="${cls}-i"></div><div class="${cls}-l">${esc(label)}</div></div>`;
      }
      case 'textField':
        rules.push(`.${cls} { height: ${M.fieldH}px; min-width: ${M.fieldMinW}px; ${paint(`.${cls}`, [['background', 'surface']])} ${item} }`);
        return `<div class="${cls}" role="textbox" tabindex="0" aria-label="${esc(label)}"></div>`;
      case 'terminal': {
        // 格子の中身は証明の対象にしない (アプリの出力)。大きさだけを固定した箱として渡す
        const h = (node.screen?.rows ?? 1) * M.termLine + 2 * M.termPad;
        rules.push(`.${cls} { height: ${h}px; min-width: ${M.termMinW}px; overflow: hidden; ${paint(`.${cls}`, [['background', 'surface']])} ${item} }`);
        return `<div class="${cls}" role="log" aria-label="${esc(label)}"></div>`;
      }
      case 'action':
      case 'button': {
        const pad = (M.btnH - lineH(M.fontSize)) / 2;
        // render.ts と同じ: 選ばれていない tab と action (クリックできるセルなど) は控えめ (surface の上の text)
        const quiet = (node.role === 'tab' && !node.selected) || (v.widget === 'action' && !node.selected);
        const colors: [string, ColorToken][] = quiet ? [['background', 'surface'], ['color', 'text']] : [['background', node.sensitive ? 'danger' : 'accent'], ['color', 'accentText']];
        pair(colors[1][1], colors[0][1], M.fontSize, 600);
        rules.push(`.${cls} { ${text(cls, label, M.fontSize, 600)} padding: ${pad}px ${M.btnPadX}px; ${paint(`.${cls}`, colors)} ${item} }`);
        return `<div class="${cls}" role="${v.widget === 'action' || node.role === 'button' ? 'button' : node.role}" tabindex="0">${esc(label)}</div>`;
      }
      case 'stepper': {
        // ラベル [−] 値 [+] (layout.ts と同じ並び)
        const val = stepperValue(node);
        rules.push(`.${cls} { display: flex; align-items: center; gap: ${M.labelGap}px; height: ${M.rowH}px; ${item} }`);
        pair('text', bg, M.fontSize, 400);
        pair('text', bg, M.fontSize, 600);
        rules.push(`.${cls}-l { flex: none; ${text(`${cls}-l`, label, M.fontSize, 400)} ${paint(`.${cls}-l`, [['color', 'text']])} }`);
        rules.push(`.${cls}-b { flex: none; width: ${M.stepBtn}px; height: ${M.stepBtn}px; ${paint(`.${cls}-b`, [['background', 'border']])} }`);
        rules.push(`.${cls}-v { flex: none; ${text(`${cls}-v`, val, M.fontSize, 600)} ${paint(`.${cls}-v`, [['color', 'text']])} }`);
        return `<div class="${cls}" role="${node.role}" aria-label="${esc(label)}" aria-valuenow="${node.valueNow ?? 0}" tabindex="0"><div class="${cls}-l">${esc(label)}</div><div class="${cls}-b" role="button" aria-label="減らす"></div><div class="${cls}-v">${esc(val)}</div><div class="${cls}-b" role="button" aria-label="増やす"></div></div>`;
      }
    }
  };

  const html = `<main>${visit(view, null, 'surface')}</main>`;
  // 断片を解析するので html の背景は届かない。ルートに不透明な背景を置く (コントラスト検査のため)
  rules.push(`main { display: block; ${paint('main', [['background', 'surface'], ['color', 'text']])}${opts.scroll ? ' overflow-y: auto; height: 100%;' : ''} }`);
  if (darkRules.length) rules.push(`@media (prefers-color-scheme: dark) {\n${darkRules.join('\n')}\n}`);
  if (opts.scroll) {
    boxes.main = { overflowBlock: 'scroll' };
    return { html, css: rules.join('\n'), boxes, pairs: [...pairs.values()], scrollbars: [{ name: 'overlay', inlineSize: 0, blockSize: 0 }] };
  }
  return { html, css: rules.join('\n'), boxes, pairs: [...pairs.values()] };
}

/**
 * 色の組の見本: 組ごとに「その背景の上の、その大きさ・太さの文字」を 1 行ずつ並べたページ。
 * 文字のコントラストは (文字色, 背景色, 大きさ, 太さ) だけで決まるので、大きな view (カレンダーの表など) を
 * 丸ごと veriss に通す代わりに、使われた組をここにまとめて、ライトとダークの両方で検査する。
 */
export function contrastSpecimen(pairs: ColorPair[], measure: Measure, opts: Pick<LowerOptions, 'theme' | 'dark' | 'fontFamily'> = {}): Omit<Lowered, 'pairs'> {
  const theme = opts.theme ?? THEME_LIGHT;
  const darkTheme = opts.dark === undefined ? THEMES.dark : opts.dark;
  const family = opts.fontFamily ?? LOWER_FONT;
  const sample = 'Aa あ 123';
  const rules = ['* { box-sizing: border-box; margin: 0; padding: 0; }', `html { overflow: hidden; font: 400 14px/20px ${family}; }`];
  const dark: string[] = [];
  const boxes: Lowered['boxes'] = {};
  const seen = new Set<string>();
  const uniq = pairs.filter((p) => {
    const k = `${p.fg}/${p.bg}/${p.size}/${p.weight}`;
    return !seen.has(k) && seen.add(k);
  });
  const items = uniq.map((p, i) => {
    const w = Math.ceil(measure(sample, p.size, p.weight));
    boxes[`.t${i}`] = { minInlineSize: w, maxInlineSize: w, intrinsicBlockSize: lineH(p.size), wrap: 'nowrap' };
    rules.push(`.p${i} { flex: none; align-self: flex-start; width: ${w + 16}px; padding: 4px 8px; background: ${theme[p.bg]}; }`);
    rules.push(`.t${i} { font: ${p.weight} ${p.size}px/${lineH(p.size)}px ${family}; white-space: nowrap; color: ${theme[p.fg]}; }`);
    if (darkTheme) dark.push(`.p${i} { background: ${darkTheme[p.bg]}; } .t${i} { color: ${darkTheme[p.fg]}; }`);
    return `<div class="p${i}"><div class="t${i}" title="${p.fg} on ${p.bg} ${p.size}px ${p.weight}">${sample}</div></div>`;
  });
  rules.push(`main { display: flex; flex-direction: column; gap: 4px; background: ${theme.bg}; color: ${theme.text}; overflow-y: auto; height: 100%; }`);
  if (darkTheme) rules.push(`@media (prefers-color-scheme: dark) {\nmain { background: ${darkTheme.bg}; color: ${darkTheme.text}; }\n${dark.join('\n')}\n}`);
  boxes.main = { overflowBlock: 'scroll' };
  // スクロールする領域はキーボードで届くように (veriss の scrollable-region-focusable)
  return { html: `<main tabindex="0" aria-label="配色の見本">${items.join('')}</main>`, css: rules.join('\n'), boxes, scrollbars: [{ name: 'overlay', inlineSize: 0, blockSize: 0 }] };
}
