// P3: View Template のカタログ。LLM はこの範囲の JSON しか書けない。
import type { ColorToken, Role, SemanticNode } from './protocol.ts';

export type Bind = { bind: 'label' | 'value' | 'count'; format?: string };

/** portable な flex の部分集合 (ADR-0015 の portable 層に寄せる。veriss に lowering する前提) */
export interface BoxStyle {
  axis?: 'vertical' | 'horizontal';
  gap?: number;
  padding?: number;
  background?: ColorToken;
  border?: ColorToken;
  radius?: number;
  grow?: number;
  /** horizontal の子を等幅に並べる (CSS の flex: 1 1 0)。表の行どうしで列が揃う */
  equal?: boolean;
}

export type ViewTemplate =
  | ({ type: 'box'; children: ViewTemplate[] } & BoxStyle)
  | { type: 'text'; text: string | Bind; size?: number; weight?: number; color?: ColorToken }
  | { type: 'widget'; kind: WidgetKind }
  /** 文脈ノードの子を、それぞれの template で描く */
  | { type: 'children' }
  /** 文脈ノードの子ごとに item を描く。item の中の文脈は子になる */
  | { type: 'each'; item: ViewTemplate }
  /** each の中だけで使える: 文脈ノード (= 子) を、その子自身の template で描く */
  | { type: 'self' };

export type WidgetKind = 'checkbox' | 'switch' | 'textField' | 'button' | 'radio' | 'stepper' | 'terminal' | 'action';

/** widget と結びつけてよい role。widget は node.actions にある action しか起こせない */
export const WIDGET_ROLES: Record<WidgetKind, Role[]> = {
  checkbox: ['checkbox'],
  switch: ['switch', 'checkbox'],
  textField: ['textbox'],
  button: ['button', 'combobox', 'tab'],
  radio: ['radio'],
  stepper: ['slider', 'spinbutton'],
  terminal: ['log'],
  // 既存アプリの「クリックできる画像やセル」など、役割と操作が食い違うもの。既定レンダラだけが使う (LLM の template では使えない)
  action: ['img', 'cell', 'paragraph', 'heading', 'listitem', 'row', 'group', 'columnheader', 'progressbar', 'tablist', 'table', 'list', 'separator', 'x-window'],
};

export const COLOR_TOKENS: ColorToken[] = ['bg', 'surface', 'text', 'muted', 'accent', 'accentText', 'border', 'danger', 'ghost'];

export const NEUTRAL_BACKGROUNDS: ColorToken[] = ['bg', 'surface', 'border'];

const LIMITS = { gap: [0, 48], padding: [0, 48], radius: [0, 24], grow: [0, 4], size: [11, 28], weight: [300, 800] } as const;
const MAX_TEMPLATE_NODES = 64;

/** 静的な検査 (形・値の範囲・語彙)。木との整合 (role の一致、カバレッジ) は展開時に検査する */
export function validateTemplate(t: unknown): string[] {
  const errors: string[] = [];
  let count = 0;
  const num = (v: unknown, key: keyof typeof LIMITS, path: string) => {
    if (v === undefined) return;
    const [lo, hi] = LIMITS[key];
    if (typeof v !== 'number' || !Number.isFinite(v) || v < lo || v > hi) errors.push(`${path}.${key}: ${lo}..${hi} の数値`);
  };
  const color = (v: unknown, path: string) => {
    if (v !== undefined && !COLOR_TOKENS.includes(v as ColorToken)) errors.push(`${path}: 色はトークン (${COLOR_TOKENS.join(', ')}) のみ`);
  };
  const visit = (v: unknown, path: string, inEach: boolean) => {
    if (++count > MAX_TEMPLATE_NODES) {
      if (count === MAX_TEMPLATE_NODES + 1) errors.push(`template のノード数が ${MAX_TEMPLATE_NODES} を超える`);
      return;
    }
    if (typeof v !== 'object' || v === null || Array.isArray(v)) {
      errors.push(`${path}: object が必要`);
      return;
    }
    const o = v as Record<string, unknown>;
    const allow = (keys: string[]) => {
      for (const k of Object.keys(o)) if (!keys.includes(k)) errors.push(`${path}.${k}: 未知のプロパティ`);
    };
    switch (o.type) {
      case 'box':
        allow(['type', 'children', 'axis', 'gap', 'padding', 'background', 'border', 'radius', 'grow', 'equal']);
        if (o.equal !== undefined && typeof o.equal !== 'boolean') errors.push(`${path}.equal: boolean`);
        if (o.axis !== undefined && o.axis !== 'vertical' && o.axis !== 'horizontal') errors.push(`${path}.axis: vertical | horizontal`);
        num(o.gap, 'gap', path);
        num(o.padding, 'padding', path);
        num(o.radius, 'radius', path);
        num(o.grow, 'grow', path);
        color(o.background, `${path}.background`);
        color(o.border, `${path}.border`);
        // 生成 UI は面を強調できない: 危険色・強調色の背景で操作の印象を変えさせない
        if (o.background !== undefined && !NEUTRAL_BACKGROUNDS.includes(o.background as ColorToken))
          errors.push(`${path}.background: 背景は ${NEUTRAL_BACKGROUNDS.join(' | ')} のみ (生成 UI は面を強調できない)`);
        if (!Array.isArray(o.children)) errors.push(`${path}.children: 配列が必要`);
        else o.children.forEach((c, i) => visit(c, `${path}.children[${i}]`, inEach));
        break;
      case 'text':
        allow(['type', 'text', 'size', 'weight', 'color']);
        num(o.size, 'size', path);
        num(o.weight, 'weight', path);
        color(o.color, `${path}.color`);
        if (typeof o.text === 'string') {
          if (o.text.length > 80) errors.push(`${path}.text: 固定文字列は 80 文字まで`);
        } else if (typeof o.text === 'object' && o.text !== null) {
          const b = o.text as Record<string, unknown>;
          if (!['label', 'value', 'count'].includes(b.bind as string)) errors.push(`${path}.text.bind: label | value | count`);
          if (b.format !== undefined && (typeof b.format !== 'string' || !b.format.includes('{}'))) errors.push(`${path}.text.format: {} を含む文字列`);
          for (const k of Object.keys(b)) if (k !== 'bind' && k !== 'format') errors.push(`${path}.text.${k}: 未知のプロパティ`);
        } else errors.push(`${path}.text: 文字列か {bind}`);
        break;
      case 'widget':
        allow(['type', 'kind']);
        if (o.kind === 'action') errors.push(`${path}.kind: action は既定レンダラ専用`);
        else if (typeof o.kind !== 'string' || !Object.hasOwn(WIDGET_ROLES, o.kind)) errors.push(`${path}.kind: ${Object.keys(WIDGET_ROLES).join(' | ')}`);
        break;
      case 'children':
        allow(['type']);
        break;
      case 'each':
        allow(['type', 'item']);
        if (inEach) errors.push(`${path}: each は入れ子にできない`);
        visit(o.item, `${path}.item`, true);
        break;
      case 'self':
        allow(['type']);
        if (!inEach) errors.push(`${path}: self は each の中だけで使える (自己再帰の禁止)`);
        break;
      default:
        errors.push(`${path}.type: box | text | widget | children | each | self`);
    }
  };
  visit(t, '$', false);
  return errors;
}

/** 既定レンダラ: role ごとの決定的な template。どれもカバレッジを満たす */
export function defaultTemplate(node: SemanticNode): ViewTemplate {
  let t = baseTemplate(node);
  // 操作できるのに部品で描かない役割 (クリックできる画像など): 控えめなボタン型の部品で描く
  if (t.type !== 'widget' && node.actions?.includes('click') && !node.disabled) t = { type: 'widget', kind: 'action' };
  // 部品が子を持つ場合 (既存アプリのコンボボックスの中の入力欄など): 部品の隣に子を描く。子の操作を落とさない
  if (t.type === 'widget' && node.children?.length) return { type: 'box', axis: 'horizontal', gap: 8, children: [t, { type: 'children' }] };
  return t;
}

function baseTemplate(node: SemanticNode): ViewTemplate {
  switch (node.role) {
    case 'x-window':
      return { type: 'box', axis: 'vertical', gap: 10, padding: 14, children: [{ type: 'children' }] };
    case 'heading':
      return { type: 'text', text: { bind: 'label' }, size: 18, weight: 700, color: 'text' };
    case 'paragraph':
      return { type: 'text', text: { bind: 'label' }, size: 13, color: 'muted' };
    case 'list':
      return { type: 'box', axis: 'vertical', gap: 4, children: [{ type: 'children' }] };
    case 'group':
    case 'listitem':
      return { type: 'box', axis: 'horizontal', gap: 8, children: [{ type: 'children' }] };
    case 'checkbox':
      return { type: 'widget', kind: 'checkbox' };
    case 'switch':
      return { type: 'widget', kind: 'switch' };
    case 'textbox':
      return { type: 'widget', kind: 'textField' };
    case 'button':
    case 'combobox':
    case 'tab':
      return { type: 'widget', kind: 'button' };
    case 'radio':
      return { type: 'widget', kind: 'radio' };
    case 'slider':
    case 'spinbutton':
      return { type: 'widget', kind: 'stepper' };
    case 'progressbar':
      return { type: 'box', axis: 'horizontal', gap: 8, children: [{ type: 'text', text: { bind: 'label' }, color: 'text' }, { type: 'text', text: { bind: 'value' }, color: 'muted' }] };
    case 'tablist':
      return { type: 'box', axis: 'horizontal', gap: 4, children: [{ type: 'children' }] };
    case 'table':
      return { type: 'box', axis: 'vertical', gap: 2, children: [{ type: 'children' }] };
    case 'row':
      // セルを等幅にして、行どうしで列を揃える
      return { type: 'box', axis: 'horizontal', gap: 4, equal: true, children: [{ type: 'children' }] };
    case 'cell':
      return { type: 'text', text: { bind: 'label' }, size: 13, color: 'text' };
    case 'columnheader':
      return { type: 'text', text: { bind: 'label' }, size: 13, weight: 700, color: 'text' };
    case 'separator':
      return { type: 'box', border: 'border', children: [] };
    case 'img':
      return { type: 'text', text: { bind: 'label' }, size: 12, color: 'muted' };
    case 'log':
      return { type: 'widget', kind: 'terminal' };
  }
}
