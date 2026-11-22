// View Tree -> veriss の入力 (HTML + CSS + 文字の契約)。layout.ts と同じ意味になるように CSS の flex に写す。
// View Tree には selector も cascade もないので、1 ノード 1 クラスにする (veriss の ADR-0014/0015 の S1・S2 が起きない)。
import type { ViewNode } from './expand.ts';
import { M, layout } from './layout.ts';
import type { ColorToken, Measure } from './protocol.ts';

/** www/index.html の light テーマと同じ値 (コントラスト検査用) */
export const THEME_LIGHT: Record<ColorToken, string> = {
  bg: '#eef0f4',
  surface: '#ffffff',
  text: '#1d2230',
  muted: '#5d6475',
  accent: '#3557d4',
  accentText: '#ffffff',
  border: '#c9ced9',
  danger: '#c2362f',
  ghost: '#b4529a',
};

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
  boxes: Record<string, TextContract>;
}

/** layout.ts と同じ規則の固有幅 */
const intrinsicWidth = (v: ViewNode, measure: Measure) => layout(v, 0, measure).minWidth;

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!);
const lineH = (size: number) => Math.ceil(size * 1.4);

export interface LowerOptions {
  theme?: Record<ColorToken, string>;
  /** CSS の font-family (計測に使ったフォントと同じにする) */
  fontFamily?: string;
  /**
   * flex の子の固有幅を px で明示する (column の非 stretch の子は width、row の子は flex-basis)。
   * veriss が「中身で決まる幅」(shrink-to-fit、flex-basis: auto) を、文字の契約 (min = max) があっても
   * Unknown にするための迂回 (mizchi/veriss#6, #7)。layout.ts が固有幅を決定的に出せるので、意味は変わらない。
   */
  pxIntrinsic?: boolean;
}

export function lower(view: ViewNode, measure: Measure, opts: LowerOptions = {}): Lowered {
  const theme = opts.theme ?? THEME_LIGHT;
  const family = opts.fontFamily ?? LOWER_FONT;
  let n = 0;
  const rules: string[] = [
    '* { box-sizing: border-box; margin: 0; padding: 0; }',
    `html { overflow: hidden; color: ${theme.text}; background: ${theme.surface}; font: 400 14px/20px ${family}; }`,
    'body { display: block; }',
  ];
  const boxes: Record<string, TextContract> = {};
  const text = (cls: string, str: string, size: number, weight: number) => {
    const w = Math.ceil(measure(str, size, weight));
    boxes[`.${cls}`] = { minInlineSize: w, maxInlineSize: w, intrinsicBlockSize: lineH(size), wrap: 'nowrap' };
    return `font: ${weight} ${size}px/${lineH(size)}px ${family}; white-space: nowrap;`;
  };

  /** parent: 親の向き。layout.ts の stretch / grow の規則を CSS の align-self / flex に写す */
  const visit = (v: ViewNode, parent: 'vertical' | 'horizontal' | null, bg: string): string => {
    const cls = `v${n++}`;
    const stretch = v.kind === 'box' || (v.kind === 'widget' && v.widget === 'textField');
    const grow = v.kind === 'box' ? (v.style.grow ?? 0) : v.kind === 'widget' && v.widget === 'textField' ? 1 : 0;
    let item = '';
    if (parent === 'vertical') item = `flex: none; align-self: ${stretch ? 'stretch' : 'flex-start'};`;
    if (parent === 'vertical' && !stretch && opts.pxIntrinsic) item += ` width: ${intrinsicWidth(v, measure)}px;`;
    if (parent === 'horizontal') {
      const basis = opts.pxIntrinsic ? `${intrinsicWidth(v, measure)}px` : 'auto';
      item = grow > 0 ? `flex: ${grow} 0 ${basis};` : `flex: 0 0 ${basis};`;
    }

    if (v.kind === 'box') {
      const axis = v.style.axis ?? 'vertical';
      const fill = v.style.background ? theme[v.style.background] : bg;
      rules.push(
        `.${cls} { display: flex; flex-direction: ${axis === 'vertical' ? 'column' : 'row'}; align-items: ${axis === 'vertical' ? 'flex-start' : 'center'}; gap: ${v.style.gap ?? 0}px; padding: ${v.style.padding ?? 0}px;${v.style.background ? ` background: ${fill};` : ''} ${item} }`,
      );
      return `<div class="${cls}">${v.children.map((c) => visit(c, axis, fill)).join('')}</div>`;
    }
    if (v.kind === 'text') {
      rules.push(`.${cls} { ${text(cls, v.str, v.size, v.weight)} color: ${theme[v.color]}; ${item} }`);
      return `<div class="${cls}">${esc(v.str)}</div>`;
    }
    const node = v.node;
    const label = node.label ?? '';
    switch (v.widget) {
      case 'checkbox':
      case 'switch': {
        const [w, h] = v.widget === 'checkbox' ? [M.check, M.check] : [M.switchW, M.switchH];
        rules.push(`.${cls} { display: flex; align-items: center; gap: ${M.labelGap}px; height: ${M.rowH}px; ${item} }`);
        rules.push(`.${cls}-i { flex: none; width: ${w}px; height: ${h}px; background: ${node.checked ? theme.accent : theme.border}; }`);
        rules.push(`.${cls}-l { flex: none; ${text(`${cls}-l`, label, M.fontSize, 400)} color: ${node.checked && v.widget === 'checkbox' ? theme.muted : theme.text}; }`);
        return `<div class="${cls}" role="${v.widget}" aria-checked="${node.checked ? 'true' : 'false'}" tabindex="0"><div class="${cls}-i"></div><div class="${cls}-l">${esc(label)}</div></div>`;
      }
      case 'textField':
        rules.push(`.${cls} { height: ${M.fieldH}px; min-width: ${M.fieldMinW}px; background: ${theme.surface}; ${item} }`);
        return `<div class="${cls}" role="textbox" tabindex="0" aria-label="${esc(label)}"></div>`;
      case 'button': {
        const pad = (M.btnH - lineH(M.fontSize)) / 2;
        rules.push(
          `.${cls} { ${text(cls, label, M.fontSize, 600)} padding: ${pad}px ${M.btnPadX}px; background: ${node.sensitive ? theme.danger : theme.accent}; color: ${theme.accentText}; ${item} }`,
        );
        return `<div class="${cls}" role="button" tabindex="0">${esc(label)}</div>`;
      }
    }
  };

  const html = `<main>${visit(view, null, theme.surface)}</main>`;
  // 断片を解析するので html の背景は届かない。ルートに不透明な背景を置く (コントラスト検査のため)
  rules.push(`main { display: block; background: ${theme.surface}; }`);
  return { html, css: rules.join('\n'), boxes };
}
