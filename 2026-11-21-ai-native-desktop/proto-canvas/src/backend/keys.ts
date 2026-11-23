// キー入力 → shell への入力 (P5)。ブラウザ (canvas backend) とネイティブのホスト (SDL3) で同じ対応を使う。
// code はブラウザの KeyboardEvent.code の名前 (mac の Option は key を変えるので、レイアウトのキーは code で見る)。
import type { ImeFromHost, LayoutKey } from '../protocol.ts';

export interface KeyInput {
  code: string;
  key: string;
  alt: boolean;
  shift: boolean;
}

const LAYOUT: Record<string, LayoutKey> = {
  ArrowLeft: 'MoveLeft',
  ArrowRight: 'MoveRight',
  ArrowUp: 'MoveUp',
  ArrowDown: 'MoveDown',
  Enter: 'Zoom',
  KeyM: 'Minimize',
  KeyE: 'Equalize',
  KeyL: 'Launcher',
  Digit1: 'Workspace1',
  Digit2: 'Workspace2',
};

/** shell に渡すキー。渡さないキー (文字は文字入力として別に届く) は null */
export function mapKey(e: KeyInput, imeEnabled: boolean): ImeFromHost | null {
  if (e.alt && LAYOUT[e.code]) return { t: 'key', key: LAYOUT[e.code] };
  const key = e.key === 'Tab' ? (e.shift ? 'ShiftTab' : 'Tab') : e.key === ' ' && !imeEnabled ? 'Space' : e.key;
  if (key === 'Tab' || key === 'ShiftTab' || key === 'Space' || key === 'Enter' || key === 'Escape' || (key === 'Backspace' && imeEnabled)) return { t: 'key', key };
  return null;
}
