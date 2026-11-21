// PAL の web-canvas backend: display list の描画、文字の計測、IME の委譲 (隠し textarea)。
import type { ColorToken, DrawOp, ImeFromHost, ImeToHost, Measure } from '../protocol.ts';

export const FONT_FAMILY = 'system-ui, "Hiragino Sans", "Noto Sans JP", sans-serif';
const TOKENS: ColorToken[] = ['bg', 'surface', 'text', 'muted', 'accent', 'accentText', 'border', 'danger', 'ghost'];

export interface CanvasBackend {
  measure: Measure;
  draw(ops: DrawOp[]): void;
  ime(state: ImeToHost): void;
  size(): { w: number; h: number };
  focusInput(): void;
  setCursor(c: string): void;
}

export interface BackendEvents {
  pointerDown(x: number, y: number): void;
  pointerMove(x: number, y: number): void;
  pointerUp(): void;
  input(msg: ImeFromHost): void;
  resize(): void;
}

export function createCanvasBackend(canvas: HTMLCanvasElement, sink: HTMLTextAreaElement, ev: BackendEvents): CanvasBackend {
  const ctx = canvas.getContext('2d')!;
  const mctx = document.createElement('canvas').getContext('2d')!;
  const widths = new Map<string, number>();
  let colors = readColors();
  let imeEnabled = false;
  let composing = false;

  function readColors(): Record<ColorToken, string> {
    const s = getComputedStyle(document.documentElement);
    return Object.fromEntries(TOKENS.map((t) => [t, s.getPropertyValue(`--c-${t}`).trim() || '#f0f'])) as Record<ColorToken, string>;
  }
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
    colors = readColors();
    ev.resize();
  });

  const font = (size: number, weight: number) => `${weight} ${size}px ${FONT_FAMILY}`;
  const measure: Measure = (str, size, weight) => {
    const key = `${weight}/${size}/${str}`;
    let w = widths.get(key);
    if (w === undefined) {
      mctx.font = font(size, weight);
      w = mctx.measureText(str).width;
      widths.set(key, w);
    }
    return w;
  };

  const fit = () => {
    const r = canvas.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(r.width * dpr);
    canvas.height = Math.round(r.height * dpr);
    ev.resize();
  };
  new ResizeObserver(fit).observe(canvas);

  // ---- 入力: pointer はそのまま shell へ (shell が hit test して意味的な action にする) ----
  const local = (e: PointerEvent) => {
    const r = canvas.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };
  canvas.addEventListener('pointerdown', (e) => {
    const p = local(e);
    canvas.setPointerCapture(e.pointerId);
    ev.pointerDown(p.x, p.y);
    e.preventDefault();
    sink.focus({ preventScroll: true });
  });
  canvas.addEventListener('pointermove', (e) => {
    const p = local(e);
    ev.pointerMove(p.x, p.y);
  });
  canvas.addEventListener('pointerup', () => ev.pointerUp());

  // ---- IME: OS の IME はブラウザの textarea に任せ、結果だけを P5 で shell に渡す ----
  sink.addEventListener('compositionstart', () => (composing = true));
  sink.addEventListener('compositionupdate', (e) => ev.input({ t: 'ime-preedit', text: e.data }));
  sink.addEventListener('compositionend', (e) => {
    composing = false;
    ev.input({ t: 'ime-commit', text: e.data });
    sink.value = '';
  });
  sink.addEventListener('input', (e) => {
    const ie = e as InputEvent;
    if (ie.isComposing || composing || ie.inputType === 'insertCompositionText') return;
    const text = ie.data ?? sink.value;
    sink.value = '';
    if (text) ev.input({ t: 'ime-commit', text });
  });
  sink.addEventListener('keydown', (e) => {
    if (e.isComposing || composing || e.keyCode === 229) return;
    const key = e.key === 'Tab' ? (e.shiftKey ? 'ShiftTab' : 'Tab') : e.key === ' ' && !imeEnabled ? 'Space' : e.key;
    if (key === 'Tab' || key === 'ShiftTab' || key === 'Space' || key === 'Enter' || key === 'Escape' || (key === 'Backspace' && imeEnabled)) {
      e.preventDefault();
      ev.input({ t: 'key', key });
    }
  });

  return {
    measure,
    size: () => {
      const r = canvas.getBoundingClientRect();
      return { w: r.width, h: r.height };
    },
    focusInput: () => sink.focus({ preventScroll: true }),
    setCursor: (c) => {
      if (canvas.style.cursor !== c) canvas.style.cursor = c;
    },
    ime(state) {
      imeEnabled = state.t === 'ime-enable';
      // 変換候補ウィンドウを OS が正しい位置に出せるよう、textarea をキャレットに重ねる
      if (state.t === 'ime-enable') {
        sink.style.left = `${state.caret.x}px`;
        sink.style.top = `${state.caret.y}px`;
        sink.style.height = `${state.caret.h}px`;
      }
    },
    draw(ops) {
      const dpr = canvas.width / Math.max(1, canvas.getBoundingClientRect().width);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      for (const op of ops) {
        switch (op[0]) {
          case 'rect': {
            const r = op[1];
            ctx.beginPath();
            ctx.roundRect(r.x, r.y, r.w, r.h, r.radius ?? 0);
            if (r.fill) {
              ctx.fillStyle = colors[r.fill];
              ctx.fill();
            }
            if (r.stroke) {
              ctx.strokeStyle = colors[r.stroke];
              ctx.lineWidth = r.lineWidth ?? 1;
              ctx.setLineDash(r.dash ? [5, 4] : []);
              ctx.stroke();
              ctx.setLineDash([]);
            }
            break;
          }
          case 'text': {
            const t = op[1];
            ctx.font = font(t.size, t.weight ?? 400);
            ctx.fillStyle = colors[t.color];
            ctx.textBaseline = 'middle';
            ctx.fillText(t.str, t.x, t.y);
            if (t.underline) ctx.fillRect(t.x, t.y + t.size * 0.62, measure(t.str, t.size, t.weight ?? 400), 1.5);
            break;
          }
          case 'clip':
            ctx.save();
            ctx.beginPath();
            ctx.rect(op[1].x, op[1].y, op[1].w, op[1].h);
            ctx.clip();
            break;
          case 'layer':
            ctx.save();
            ctx.globalAlpha *= op[1].alpha;
            break;
          case 'pop':
            ctx.restore();
            break;
        }
      }
    },
  };
}
