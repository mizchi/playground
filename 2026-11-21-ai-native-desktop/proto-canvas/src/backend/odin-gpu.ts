// PAL の Odin backend の GPU 版 (vendor:wgpu → ブラウザの WebGPU) の JS 側。
// 描画命令を Odin の関数呼び出しに変え、Odin が 1 回のインスタンス描画で WebGPU の canvas に描く。
// WebGPU の canvas は、入力を受ける canvas (#screen) の下に重ねる。入力と IME は canvas backend のまま。
import type { ColorToken, DrawOp, Measure } from '../protocol.ts';

interface GpuExports {
  memory: WebAssembly.Memory;
  _start(): void;
  is_ready(): number;
  alloc(n: number): number;
  resize(w: number, h: number): void;
  frame_begin(bg: number): void;
  rect(x: number, y: number, w: number, h: number, r: number, fill: number, stroke: number, lw: number, dash: number): void;
  clip(x: number, y: number, w: number, h: number): void;
  layer(a: number): void;
  pop_state(): void;
  load_font(ptr: number, n: number): number;
  measure(ptr: number, n: number, size: number): number;
  text(x: number, y: number, size: number, color: number, underline: number, ptr: number, n: number): void;
  frame_end(): number;
  surface_status(): number;
  capture(): number;
  set_offscreen(on: number): void;
  capture_state(): number;
  capture_ptr(): number;
}

type OdinGlobal = {
  WasmMemoryInterface: new () => { memory?: WebAssembly.Memory; setExports(e: unknown): void; setMemory(m: WebAssembly.Memory): void; setIntSize(n: number): void };
  setupDefaultImports(m: unknown, c: unknown, mem: unknown): WebAssembly.Imports;
  WebGPUInterface: new (mem: unknown) => { getInterface(): WebAssembly.ModuleImports };
};

const rgba = (hex: string) => {
  const n = parseInt(hex.replace('#', ''), 16);
  return ((0xff << 24) | ((n & 0xff) << 16) | (n & 0xff00) | ((n >> 16) & 0xff)) >>> 0;
};
const script = (src: string) =>
  new Promise<void>((ok, ng) => {
    const s = document.createElement('script');
    s.src = src;
    s.onload = () => ok();
    s.onerror = () => ng(new Error(`${src} を読めない`));
    document.head.append(s);
  });

export interface OdinGpu {
  measure: Measure;
  draw(ops: DrawOp[], colors: Record<ColorToken, string>, input: HTMLCanvasElement): void;
  /** 直近のフレームのインスタンス数 (= draw call 1 回で描いた四角形の数) */
  lastInstances(): number;
  /** 直近のフレームをオフスクリーンに描き直して画素を読み戻す (検証用。ヘッドレスのスクリーンショットには WebGPU の canvas が写らない) */
  capture(): Promise<ImageData>;
}

/** offscreen: canvas に表示せず、オフスクリーンにだけ描く (検証用。capture で読み戻す) */
export async function createOdinGpu(wasmUrl: string, fontUrl: string, opts: { offscreen?: boolean } = {}): Promise<OdinGpu> {
  if (!('gpu' in navigator)) throw new Error('この環境では WebGPU が使えない');
  const w = window as unknown as { odin?: OdinGlobal };
  if (!w.odin?.setupDefaultImports) await script('odin.js');
  if (!w.odin?.WebGPUInterface) await script('wgpu.js');
  const odin = w.odin!;

  // WebGPU の canvas を、入力を受ける canvas の下に置く (Odin は "#gpu-screen" で canvas を探す)
  const input = document.getElementById('screen') as HTMLCanvasElement;
  const gpuCanvas = document.createElement('canvas');
  gpuCanvas.id = 'gpu-screen';
  gpuCanvas.setAttribute('aria-hidden', 'true');
  gpuCanvas.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;';
  input.before(gpuCanvas);

  const mem = new odin.WasmMemoryInterface();
  mem.setIntSize(4);
  const wgpu = new odin.WebGPUInterface(mem);
  const imports = { ...odin.setupDefaultImports(mem, null, mem.memory), wgpu: wgpu.getInterface() };
  const { instance } = await WebAssembly.instantiateStreaming(fetch(wasmUrl), imports);
  const x = instance.exports as unknown as GpuExports;
  mem.setExports(x);
  mem.setMemory(x.memory);
  Object.assign(window, { odinGpuExports: x });
  if (opts.offscreen) x.set_offscreen(1);
  x._start(); // main: adapter と device を非同期に要求する
  for (let i = 0; x.is_ready() === 0; i++) {
    if (i > 500) throw new Error('WebGPU の準備が終わらない');
    await new Promise((r) => setTimeout(r, 10));
  }
  if (x.is_ready() < 0) throw new Error('WebGPU の adapter / device を取れない');

  const font = new Uint8Array(await (await fetch(fontUrl)).arrayBuffer());
  const fptr = x.alloc(font.length);
  new Uint8Array(x.memory.buffer, fptr, font.length).set(font);
  if (!x.load_font(fptr, font.length)) throw new Error('Odin がフォントを読めない');

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
  let gw = 0;
  let gh = 0;
  let last = 0;
  return {
    measure: (s, size) => {
      const k = `${size}/${s}`;
      let v = cache.get(k);
      if (v === undefined) {
        const [p, n] = str(s);
        cache.set(k, (v = x.measure(p, n, size)));
      }
      return v;
    },
    lastInstances: () => last,
    async capture() {
      if (!x.capture()) throw new Error('読み戻しを始められない');
      for (let i = 0; x.capture_state() === 1; i++) {
        if (i > 1000) throw new Error('読み戻しが終わらない');
        await new Promise((r) => setTimeout(r, 5));
      }
      if (x.capture_state() < 0) throw new Error('読み戻しに失敗した');
      return new ImageData(new Uint8ClampedArray(x.memory.buffer, x.capture_ptr(), gw * gh * 4).slice(), gw, gh);
    },
    draw(ops, colors, inputCanvas) {
      const dpr = inputCanvas.width / Math.max(1, inputCanvas.getBoundingClientRect().width);
      if (inputCanvas.width !== gw || inputCanvas.height !== gh) {
        gw = gpuCanvas.width = inputCanvas.width;
        gh = gpuCanvas.height = inputCanvas.height;
        x.resize(gw, gh);
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
      last = x.frame_end();
    },
  };
}
