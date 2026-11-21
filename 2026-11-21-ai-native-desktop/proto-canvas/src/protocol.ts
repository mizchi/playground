// PROTOCOL.md のワイヤ形式。すべて JSON で表せる値だけを使う。

export type NodeId = string;
export type AppId = string;
export type Actor = 'human' | 'agent';

/** WAI-ARIA の role 名。ARIA にないものは x- 接頭辞の拡張。 */
export type Role =
  | 'x-window'
  | 'heading'
  | 'paragraph'
  | 'group'
  | 'list'
  | 'listitem'
  | 'checkbox'
  | 'switch'
  | 'textbox'
  | 'button';

/** AccessKit の Action 名を camelCase で借りたもの。 */
export type ActionName = 'click' | 'focus' | 'setValue' | 'replaceSelectedText';

export interface SemanticNode {
  id: NodeId;
  role: Role;
  /** accessible name */
  label?: string;
  value?: string;
  checked?: boolean | 'mixed';
  disabled?: boolean;
  multiline?: boolean;
  /** agent からの操作に毎回承認を要する */
  sensitive?: boolean;
  actions?: ActionName[];
  children?: NodeId[];
}

/** P1: app -> shell */
export interface TreeUpdate {
  t: 'tree-update';
  app: AppId;
  /** 最初の更新では必須 */
  root?: NodeId;
  focus: NodeId | null;
  nodes: SemanticNode[];
  remove?: NodeId[];
}

/** P2: shell -> app */
export interface ActionRequest {
  t: 'action';
  app: AppId;
  target: NodeId;
  action: ActionName;
  data?: string;
  by: Actor;
  proposal?: string;
}

/** P5: host <-> shell */
export type ImeToHost =
  | { t: 'ime-enable'; node: NodeId; surrounding: string; cursor: number; caret: Rect }
  | { t: 'ime-disable' };
export type ImeFromHost =
  | { t: 'ime-preedit'; text: string }
  | { t: 'ime-commit'; text: string }
  | { t: 'key'; key: 'Tab' | 'ShiftTab' | 'Space' | 'Enter' | 'Backspace' | 'Escape' };

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** P4b: display list */
export type ColorToken = 'bg' | 'surface' | 'text' | 'muted' | 'accent' | 'accentText' | 'border' | 'danger' | 'ghost';
export type DrawOp =
  | ['rect', Rect & { fill?: ColorToken; stroke?: ColorToken; radius?: number; dash?: boolean; lineWidth?: number }]
  | ['text', { x: number; y: number; str: string; size: number; weight?: number; color: ColorToken; underline?: boolean }]
  | ['clip', Rect]
  | ['pop'];

/** 文字幅の計測 (今は backend に委譲。veriss 統合時は shell 側の font metrics に置き換える) */
export type Measure = (str: string, size: number, weight: number) => number;
