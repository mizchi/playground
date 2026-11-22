// libghostty-vt (Ghostty の VT パーサ・端末の状態機械) の wasm を包む。
// ghostty-web (npm, MIT) が同梱する ghostty-vt.wasm を、自分で instantiate して使う (ブラウザでも Node でも同じ)。
// 端末の状態 (画面・色・カーソル・スクロールバック) は wasm の中にあり、ここでは画面を TermRun の行に写すだけ。
import { Ghostty } from 'ghostty-web';
import type { GhosttyCell, GhosttyTerminal } from 'ghostty-web';
import type { TermRun, TerminalScreen } from '../../protocol.ts';

export type { Ghostty };

/** wasm のバイト列から Ghostty を作る (fetch / readFile は呼び出し側) */
export async function instantiateGhostty(bytes: BufferSource): Promise<Ghostty> {
  let memory: WebAssembly.Memory | null = null;
  const { instance } = await WebAssembly.instantiate(bytes, {
    env: {
      log: (ptr: number, len: number) => {
        if (memory) console.log('[ghostty-vt]', new TextDecoder().decode(new Uint8Array(memory.buffer, ptr, len)));
      },
    },
  });
  memory = instance.exports.memory as WebAssembly.Memory;
  return new Ghostty(instance);
}

const hex = (r: number, g: number, b: number) => `#${((1 << 24) | (r << 16) | (g << 8) | b).toString(16).slice(1)}`;
const BOLD = 1;
const INVERSE = 16;
const INVISIBLE = 32;

/** 画面の配色 (既定の文字色・背景色) */
export const TERM_THEME = { fg: 0xe6e8ef, bg: 0x15171d };

export interface Vt {
  readonly cols: number;
  readonly rows: number;
  write(data: string): void;
  /** 今の画面 (描画用の行と、読み上げ・agent 向けの文字) */
  snapshot(): { screen: TerminalScreen; text: string };
  scrollback(): number;
  free(): void;
}

/** 端末を作る関数 (アプリはこれだけを受け取る。ghostty-web 自体は読み込みを遅らせられる) */
export type VtFactory = (cols: number, rows: number) => Vt;

export async function loadVtFactory(bytes: BufferSource): Promise<VtFactory> {
  const g = await instantiateGhostty(bytes);
  return (cols, rows) => createVt(g, cols, rows);
}

export function createVt(ghostty: Ghostty, cols: number, rows: number): Vt {
  const t: GhosttyTerminal = ghostty.createTerminal(cols, rows, { fgColor: TERM_THEME.fg, bgColor: TERM_THEME.bg, scrollbackLimit: 2000 });
  // ghostty-web 0.4.0 の wasm は、別の端末を free したあとのメモリを 0 で埋めずに画面に使う。
  // そのままだと空の cell にごみの文字が出るので、最初に画面を消去 (ED 2) して、すべての cell を空白で書かせる
  t.write('\x1b[2J\x1b[H');
  return {
    cols,
    rows,
    write: (d) => t.write(d),
    scrollback: () => t.getScrollbackLength(),
    free: () => t.free(),
    snapshot() {
      t.update();
      const colors = t.getColors();
      const fg0 = hex(colors.foreground.r, colors.foreground.g, colors.foreground.b);
      const bg0 = hex(colors.background.r, colors.background.g, colors.background.b);
      const cells = t.getViewport();
      const cur = t.getCursor();
      const lines: TermRun[][] = [];
      const text: string[] = [];
      for (let y = 0; y < rows; y++) {
        const row = cells.slice(y * cols, (y + 1) * cols);
        lines.push(runsOf(row, fg0, bg0));
        text.push(row.map((c) => (c.width === 0 ? '' : c.codepoint ? String.fromCodePoint(c.codepoint) : ' ')).join('').trimEnd());
      }
      t.markClean();
      while (text.length && text.at(-1) === '') text.pop();
      return { screen: { cols, rows, lines, cursor: cur.visible ? { x: cur.viewportX, y: cur.viewportY } : null, fg: fg0, bg: bg0 }, text: text.join('\n') };
    },
  };
}

/** 1 行の cell を、同じ見た目が続く区間にまとめる。全角は 1 文字ずつ、既定色は省く、行末の空白は捨てる */
function runsOf(row: GhosttyCell[], fg0: string, bg0: string): TermRun[] {
  const runs: TermRun[] = [];
  let cur: TermRun | null = null;
  row.forEach((c, col) => {
    if (c.width === 0) return; // 全角の 2 桁目
    let fg = hex(c.fg_r, c.fg_g, c.fg_b);
    let bg = hex(c.bg_r, c.bg_g, c.bg_b);
    if (c.flags & INVERSE) [fg, bg] = [bg, fg];
    const ch = c.codepoint && !(c.flags & INVISIBLE) ? String.fromCodePoint(c.codepoint) : ' ';
    const run: TermRun = { col, text: ch };
    if (fg !== fg0) run.fg = fg;
    if (bg !== bg0) run.bg = bg;
    if (c.flags & BOLD) run.bold = true;
    if (c.width === 2) run.wide = true;
    if (cur && !cur.wide && !run.wide && cur.fg === run.fg && cur.bg === run.bg && cur.bold === run.bold && cur.col + [...cur.text].length === col) cur.text += ch;
    else {
      cur = run;
      runs.push(run);
    }
  });
  // 背景の無い空白だけの区間と、行末の空白を落とす
  return runs
    .map((r) => (r.bg ? r : { ...r, text: r.text.trimEnd() }))
    .filter((r) => r.text !== '' && (r.bg || r.text.trim() !== ''));
}
