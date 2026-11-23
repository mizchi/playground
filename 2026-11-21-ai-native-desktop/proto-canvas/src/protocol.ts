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
  | 'button'
  | 'radio'
  | 'combobox'
  | 'slider'
  | 'spinbutton'
  | 'progressbar'
  | 'tablist'
  | 'tab'
  | 'table'
  | 'row'
  | 'cell'
  | 'columnheader'
  | 'separator'
  | 'img'
  /** ターミナルの画面など、追記されていく出力 (ARIA の log)。文字は value、描画用の色付きの格子は screen */
  | 'log';

/** AccessKit の Action 名を camelCase で借りたもの。 */
export type ActionName = 'click' | 'focus' | 'setValue' | 'replaceSelectedText' | 'increment' | 'decrement';

export interface SemanticNode {
  id: NodeId;
  role: Role;
  /** accessible name */
  label?: string;
  value?: string;
  checked?: boolean | 'mixed';
  /** tab などで選ばれている */
  selected?: boolean;
  /** slider / spinbutton / progressbar の値 (aria-valuenow / min / max) */
  valueNow?: number;
  valueMin?: number;
  valueMax?: number;
  disabled?: boolean;
  multiline?: boolean;
  /** agent からの操作に毎回承認を要する */
  sensitive?: boolean;
  actions?: ActionName[];
  children?: NodeId[];
  /** role log のターミナルの画面 (描画用)。agent には渡さない (文字は value にある) */
  screen?: TerminalScreen;
}

/** ターミナルの画面の 1 行の中の、同じ見た目が続く区間 */
export interface TermRun {
  /** 始まりの桁 */
  col: number;
  text: string;
  /** '#rrggbb'。無ければ画面の既定色 */
  fg?: string;
  bg?: string;
  bold?: boolean;
  /** 全角 (2 桁を占める 1 文字) */
  wide?: boolean;
}

export interface TerminalScreen {
  cols: number;
  rows: number;
  lines: TermRun[][];
  cursor: { x: number; y: number } | null;
  fg: string;
  bg: string;
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
  | { t: 'key'; key: 'Tab' | 'ShiftTab' | 'Space' | 'Enter' | 'Backspace' | 'Escape' | LayoutKey };
/** レイアウトのキー操作 (Alt + 方向キー / Enter / M / E) */
export type LayoutKey = 'MoveLeft' | 'MoveRight' | 'MoveUp' | 'MoveDown' | 'Zoom' | 'Minimize' | 'Equalize' | 'Launcher' | 'Workspace1' | 'Workspace2';

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** P4b: display list */
export type ColorToken = 'bg' | 'surface' | 'text' | 'muted' | 'accent' | 'accentText' | 'border' | 'danger' | 'ghost';
export type DrawOp =
  /** rgb は fill の代わりの色 ('#rrggbb')。ターミナルの画面のような、アプリの中身の色だけに使う (shell の UI は token だけ) */
  | ['rect', Rect & { fill?: ColorToken; stroke?: ColorToken; radius?: number; dash?: boolean; lineWidth?: number; rgb?: string }]
  /** mono は等幅のフォントで描く */
  | ['text', { x: number; y: number; str: string; size: number; weight?: number; color: ColorToken; underline?: boolean; rgb?: string; mono?: boolean }]
  | ['clip', Rect]
  /** 不透明度を掛けた層を始める。pop で戻す */
  | ['layer', { alpha: number }]
  | ['pop'];

/** 文字幅の計測 (今は backend に委譲。veriss 統合時は shell 側の font metrics に置き換える) */
export type Measure = (str: string, size: number, weight: number) => number;
