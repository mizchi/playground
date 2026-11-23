// ネイティブのホスト → 描画プロセス (native/renderer) の通信の形。[u32 長さ][u8 種類][中身] (little endian)
// 描画命令 (P4b の DrawOp) は、色 token を RGBA に解決し、座標は論理ピクセルのまま送る (描画側が拡大率を掛ける)。
import type { ColorToken, DrawOp } from '../src/protocol.ts';

export const MSG = { font: 1, frame: 2, capture: 3, ime: 4, cursor: 5, quit: 6 } as const;
const OP = { rect: 1, text: 2, clip: 3, layer: 4, pop: 5 } as const;
export const CURSOR: Record<string, number> = { default: 0, pointer: 1, grab: 2, grabbing: 2, 'col-resize': 3, 'row-resize': 4, 'not-allowed': 5 };

/** '#rrggbb' → 0xAABBGGRR (Odin の Color) */
export const rgba = (hex: string) => {
  const n = parseInt(hex.replace('#', ''), 16);
  return ((0xff << 24) | ((n & 0xff) << 16) | (n & 0xff00) | ((n >> 16) & 0xff)) >>> 0;
};

class Writer {
  buf = new Uint8Array(1 << 16);
  view = new DataView(this.buf.buffer);
  n = 0;
  private room(k: number) {
    if (this.n + k <= this.buf.length) return;
    const next = new Uint8Array(Math.max(this.buf.length * 2, this.n + k));
    next.set(this.buf.subarray(0, this.n));
    this.buf = next;
    this.view = new DataView(next.buffer);
  }
  u8(v: number) {
    this.room(1);
    this.buf[this.n++] = v;
  }
  u32(v: number) {
    this.room(4);
    this.view.setUint32(this.n, v >>> 0, true);
    this.n += 4;
  }
  f32(v: number) {
    this.room(4);
    this.view.setFloat32(this.n, v, true);
    this.n += 4;
  }
  bytes(b: Uint8Array) {
    this.u32(b.length);
    this.room(b.length);
    this.buf.set(b, this.n);
    this.n += b.length;
  }
  done() {
    return this.buf.subarray(0, this.n);
  }
}

/** 1 通にする */
export function message(type: number, body: Uint8Array = new Uint8Array()): Buffer {
  const out = Buffer.alloc(5 + body.length);
  out.writeUInt32LE(body.length + 1, 0);
  out[4] = type;
  out.set(body, 5);
  return out;
}

const enc = new TextEncoder();

/** 描画命令の列を FRAME の中身にする */
export function encodeFrame(ops: DrawOp[], colors: Record<ColorToken, string>): Uint8Array {
  const w = new Writer();
  const c = (t?: ColorToken) => (t ? rgba(colors[t]) : 0);
  w.u32(rgba(colors.bg));
  for (const op of ops) {
    switch (op[0]) {
      case 'rect': {
        const r = op[1];
        w.u8(OP.rect);
        for (const v of [r.x, r.y, r.w, r.h, r.radius ?? 0]) w.f32(v);
        w.u32(r.rgb ? rgba(r.rgb) : c(r.fill));
        w.u32(c(r.stroke));
        w.f32(r.lineWidth ?? 1);
        w.u8(r.dash ? 1 : 0);
        break;
      }
      case 'text': {
        const t = op[1];
        w.u8(OP.text);
        w.f32(t.x);
        w.f32(t.y);
        w.f32(t.size);
        w.u32(t.rgb ? rgba(t.rgb) : c(t.color));
        w.u8(t.underline ? 1 : 0);
        w.bytes(enc.encode(t.str));
        break;
      }
      case 'clip':
        w.u8(OP.clip);
        for (const v of [op[1].x, op[1].y, op[1].w, op[1].h]) w.f32(v);
        break;
      case 'layer':
        w.u8(OP.layer);
        w.f32(op[1].alpha);
        break;
      case 'pop':
        w.u8(OP.pop);
        break;
    }
  }
  return w.done();
}

export function encodeIme(on: boolean, caret = { x: 0, y: 0, w: 0, h: 0 }): Uint8Array {
  const w = new Writer();
  w.u8(on ? 1 : 0);
  for (const v of [caret.x, caret.y, caret.w, caret.h]) w.f32(v);
  return w.done();
}

/** 描画プロセスから届く入力・応答 (JSON Lines) */
export type RendererEvent =
  | { t: 'ready'; w: number; h: number; scale: number; headless: boolean; format: string; adapter: string }
  | { t: 'size'; w: number; h: number; scale: number }
  | { t: 'down'; x: number; y: number; clicks: number }
  | { t: 'move'; x: number; y: number }
  | { t: 'up' }
  | { t: 'wheel'; x: number; y: number; dy: number }
  | { t: 'key'; code: string; key: string; alt: boolean; shift: boolean }
  | { t: 'text'; text: string }
  | { t: 'preedit'; text: string }
  | { t: 'frame'; n: number; ms: number; skipped?: boolean }
  | { t: 'captured'; ok: boolean; path: string; w: number; h: number }
  | { t: 'close' }
  | { t: 'error'; message: string };
