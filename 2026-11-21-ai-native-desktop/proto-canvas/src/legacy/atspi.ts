// 既存アプリ (AT-SPI のアクセシビリティツリー) を、試作の意味ツリー (ARIA の語彙) に写す。純粋な関数だけ。
// 「二層構え」の既存アプリ側: 新しいアプリは意味ツリーを自分で出し、既存アプリは OS の a11y ツリーから写す。
import type { ActionName, Role, SemanticNode } from '../protocol.ts';

/** legacy/atspi_bridge.py dump の出力 */
export interface AtspiNode {
  path: string;
  role: string;
  name: string;
  states: string[];
  actions: string[];
  text: string | null;
  editable: boolean;
  value?: { current: number; min: number; max: number; step: number } | null;
  children: AtspiNode[];
}

/** AT-SPI の role 名 → ARIA (flatten: 名前の無い入れ物は平らにして子を親に上げる) */
const ROLE_MAP: Record<string, Role | 'flatten'> = {
  application: 'flatten',
  frame: 'x-window',
  window: 'x-window',
  dialog: 'x-window',
  filler: 'flatten',
  panel: 'flatten',
  'scroll pane': 'flatten',
  viewport: 'flatten',
  section: 'flatten',
  heading: 'heading',
  label: 'paragraph',
  static: 'paragraph',
  'check box': 'checkbox',
  'check menu item': 'checkbox',
  'toggle button': 'switch',
  switch: 'switch',
  'push button': 'button',
  button: 'button',
  text: 'textbox',
  entry: 'textbox',
  'password text': 'textbox',
  list: 'list',
  'list box': 'list',
  'list item': 'listitem',
  'radio button': 'radio',
  'radio menu item': 'radio',
  'combo box': 'combobox',
  slider: 'slider',
  'spin button': 'spinbutton',
  'progress bar': 'progressbar',
  'level bar': 'progressbar',
  'page tab list': 'tablist',
  'page tab': 'tab',
  table: 'table',
  'tree table': 'table',
  tree: 'table',
  'table row': 'row',
  'table cell': 'cell',
  'table column header': 'columnheader',
  'column header': 'columnheader',
  separator: 'separator',
  icon: 'img',
  image: 'img',
  animation: 'img',
  // スクロールバーは写さない (shell が自分でスクロールする)
  'scroll bar': 'flatten',
};

/** AT-SPI の action のうち、「押す」に当たるもの (優先順) */
export const PRESS_ACTIONS = ['click', 'press', 'activate', 'toggle'];

export interface MappedTree {
  root: string;
  nodes: SemanticNode[];
  /** 写せなかった AT-SPI の role と、その数 (group か paragraph に落とした) */
  unknownRoles: Record<string, number>;
  /** 意味ツリーの id → AT-SPI の道 (操作の実行に使う) */
  paths: Record<string, string>;
}

const idOf = (path: string) => `p${path || 'root'}`;

export function mapAtspi(app: AtspiNode, opts: { includeHidden?: boolean } = {}): MappedTree {
  const nodes: SemanticNode[] = [];
  const unknownRoles: Record<string, number> = {};
  const paths: Record<string, string> = {};
  const visible = (n: AtspiNode) => opts.includeHidden || n.states.includes('showing') || n.role === 'application';

  /** 子を意味ツリーの節点の列にする (flatten は子を上げる) */
  const convert = (n: AtspiNode): string[] => {
    if (!visible(n)) return [];
    let role = ROLE_MAP[n.role];
    const label = n.name || n.text || '';
    if (role === undefined) {
      unknownRoles[n.role] = (unknownRoles[n.role] ?? 0) + 1;
      // 写せない role: 子があり名前があれば group、名前だけなら paragraph、どちらも無ければ平らにする
      role = n.children.length ? (label ? 'group' : 'flatten') : label ? 'paragraph' : 'flatten';
    }
    if (role === 'flatten' && label && n.children.length && n.role !== 'application' && n.role !== 'filler') role = 'group';
    let kids = n.children.flatMap(convert);
    if (role === 'flatten') return kids;
    if (role === 'table') kids = groupRows(n, kids);
    const id = idOf(n.path);
    paths[id] = n.path;
    const actions: ActionName[] = [];
    const sensitive = n.states.includes('sensitive') || n.states.includes('enabled');
    if (role === 'textbox' && n.editable) actions.push('focus', 'setValue', 'replaceSelectedText');
    else if ((role === 'slider' || role === 'spinbutton') && n.value && sensitive) actions.push('increment', 'decrement', 'focus');
    else if (n.actions.some((a) => PRESS_ACTIONS.includes(a))) actions.push('click', 'focus');
    const node: SemanticNode = { id, role, label: role === 'textbox' ? n.name || label : label };
    if (role === 'textbox') node.value = n.text ?? '';
    if (role === 'checkbox' || role === 'switch' || role === 'radio') node.checked = n.states.includes('checked') || n.states.includes('pressed');
    if (role === 'tab') node.selected = n.states.includes('selected');
    if (n.value && (role === 'slider' || role === 'spinbutton' || role === 'progressbar')) {
      node.valueNow = n.value.current;
      node.valueMin = n.value.min;
      node.valueMax = n.value.max;
      if (role === 'progressbar') node.value = n.value.max > n.value.min ? `${Math.round(((n.value.current - n.value.min) / (n.value.max - n.value.min)) * 100)}%` : String(n.value.current);
    }
    if (!sensitive && role !== 'x-window' && role !== 'paragraph' && role !== 'heading') node.disabled = true;
    if (actions.length) node.actions = actions;
    if (kids.length) node.children = kids;
    nodes.push(node);
    return [id];
  };
  /** GTK の表は行の節点を持たず、列見出しとセルが並ぶだけ。列見出しの数ごとにセルを行にまとめる */
  const groupRows = (n: AtspiNode, kids: string[]): string[] => {
    const byId = new Map(nodes.map((x) => [x.id, x] as const));
    const headers = kids.filter((k) => byId.get(k)?.role === 'columnheader');
    const cells = kids.filter((k) => byId.get(k)?.role === 'cell');
    if (!headers.length || !cells.length) return kids;
    const rows: string[] = [];
    const head = `${idOf(n.path)}-h`;
    nodes.push({ id: head, role: 'row', children: headers });
    rows.push(head);
    for (let i = 0; i < cells.length; i += headers.length) {
      const id = `${idOf(n.path)}-r${i / headers.length}`;
      nodes.push({ id, role: 'row', children: cells.slice(i, i + headers.length) });
      rows.push(id);
    }
    return [...kids.filter((k) => !headers.includes(k) && !cells.includes(k)), ...rows];
  };
  const top = convert(app);
  // 窓が 1 つならそれを根に、複数なら最初の窓 (この試作は 1 アプリ 1 窓)
  const root = top.find((id) => nodes.find((n) => n.id === id)?.role === 'x-window') ?? top[0];
  return { root, nodes, unknownRoles, paths };
}
