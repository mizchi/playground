// PAL の Odin backend (js_wasm32) の JS 側: 描画命令を Odin の関数呼び出しに変え、Odin のフレームバッファを Canvas に写す。
// 入力と IME は canvas backend のものをそのまま使い、描画と計測だけを差し替える。
import type { ColorToken, DrawOp, Measure } from '../protocol.ts';

interface RasterExports {
  memory: WebAssembly.Memory;
  _start?: () => void;
  init(): void;
  alloc(n: number): number;
  resize(w: number, h: number): number;
  frame_begin(bg: number): void;
  rect(x: number, y: number, w: number, h: number, r: number, fill: number, stroke: number, lw: number, dash: number): void;
  clip(x: number, y: number, w: number, h: number): void;
  layer(a: number): void;
  pop_state(): void;
  load_font(ptr: number, n: number): number;
  measure(ptr: number, n: number, size: number): number;
  text(x: number, y: number, size: number, color: number, underline: number, ptr: number, n: number): void;
}

declare global {
  interface Window {
    odin: { WasmMemoryInterface: new () => { memory?: WebAssembly.Memory; setExports(e: unknown): void; setMemory(m: WebAssembly.Memory): void; setIntSize(n: number): void }; setupDefaultImports(m: unknown, c: unknown, mem: unknown): WebAssembly.Imports };
  }
}

/** '#rrggbb' → 0xAABBGGRR */
const rgba = (hex: string, alpha = 1) => {
  const n = parseInt(hex.replace('#', ''), 16);
  return ((Math.round(alpha * 255) << 24) | ((n & 0xff) << 16) | (n & 0xff00) | ((n >> 16) & 0xff)) >>> 0;
};

export interface OdinRaster {
  measure: Measure;
  draw(ops: DrawOp[], colors: Record<ColorToken, string>, canvas: HTMLCanvasElement): void;
}

export async function createOdinRaster(wasmUrl: string, fontUrl: string): Promise<OdinRaster> {
  if (!window.odin) {
    // Odin の wasm 用ランタイム (core/sys/wasm/js/odin.js をビルド時に www に写したもの)
    await new Promise<void>((ok, ng) => {
      const s = document.createElement('script');
      s.src = 'odin.js';
      s.onload = () => ok();
      s.onerror = () => ng(new Error('odin.js を読めない'));
      document.head.append(s);
    });
  }
  const mem = new window.odin.WasmMemoryInterface();
  mem.setIntSize(4);
  const imports = window.odin.setupDefaultImports(mem, null, mem.memory);
  const { instance } = await WebAssembly.instantiateStreaming(fetch(wasmUrl), imports);
  const x = instance.exports as unknown as RasterExports;
  mem.setExports(x);
  mem.setMemory(x.memory);
  x._start?.();
  x.init();

  const font = new Uint8Array(await (await fetch(fontUrl)).arrayBuffer());
  const fptr = x.alloc(font.length);
  new Uint8Array(x.memory.buffer, fptr, font.length).set(font);
  if (!x.load_font(fptr, font.length)) throw new Error('Odin がフォントを読めない');

  // 文字列の受け渡し用の領域 (足りなければ取り直す)
  const enc = new TextEncoder();
  let sptr = 0;
  let scap = 0;
  const str = (s: string): [number, number] => {
    const b = enc.encode(s);
    if (b.length > scap) {
      scap = Math.max(256, b.length * 2);
      sptr = x.alloc(scap);
    }
    new Uint8Array(x.memory.buffer, sptr, b.length).set(b);
    return [sptr, b.length];
  };
  const cache = new Map<string, number>();
  let fbPtr = 0;
  let fw = 0;
  let fh = 0;
  return {
    measure: (s, size) => {
      const k = `${size}/${s}`;
      let w = cache.get(k);
      if (w === undefined) {
        const [p, n] = str(s);
        cache.set(k, (w = x.measure(p, n, size)));
      }
      return w;
    },
    draw(ops, colors, canvas) {
      const dpr = canvas.width / Math.max(1, canvas.getBoundingClientRect().width);
      if (canvas.width !== fw || canvas.height !== fh) {
        fw = canvas.width;
        fh = canvas.height;
        fbPtr = x.resize(fw, fh);
      }
      const c = (t?: ColorToken) => (t ? rgba(colors[t]) : 0);
      x.frame_begin(rgba(colors.bg));
      for (const op of ops) {
        switch (op[0]) {
          case 'rect': {
            const r = op[1];
            x.rect(r.x * dpr, r.y * dpr, r.w * dpr, r.h * dpr, (r.radius ?? 0) * dpr, r.rgb ? rgba(r.rgb) : c(r.fill), c(r.stroke), (r.lineWidth ?? 1) * dpr, r.dash ? 1 : 0);
            break;
          }
          case 'text': {
            const t = op[1];
            const [p, n] = str(t.str);
            x.text(t.x * dpr, t.y * dpr, t.size * dpr, t.rgb ? rgba(t.rgb) : c(t.color), t.underline ? 1 : 0, p, n);
            break;
          }
          case 'clip':
            x.clip(op[1].x * dpr, op[1].y * dpr, op[1].w * dpr, op[1].h * dpr);
            break;
          case 'layer':
            x.layer(op[1].alpha);
            break;
          case 'pop':
            x.pop_state();
            break;
        }
      }
      // wasm のメモリを直接参照する (コピーしない)。メモリが伸びると buffer が変わるので、毎回作り直す
      const img = new ImageData(new Uint8ClampedArray(x.memory.buffer, fbPtr, fw * fh * 4), fw, fh);
      canvas.getContext('2d')!.putImageData(img, 0, 0);
    },
  };
}
