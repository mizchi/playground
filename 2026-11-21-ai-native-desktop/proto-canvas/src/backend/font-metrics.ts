// shell 側の文字幅の計測を、veriss と同じ mizchi/font (MoonBit → JS) で行う。
// 証明 (veriss) と描画時のレイアウトが同じ計測値を使うので、「証明した最小幅」と「実際の最小幅」が一致する (VALIDATION.md の 4c)。
// フォントは 1 つ (regular)。Canvas 側も同じファイルを FontFace で読み、太字の合成をさせない (100〜900 に当てる)。
import { createTextMetrics } from '@mizchi/veriss/text';
// @ts-ignore 型定義の無い MoonBit の出力 (load_font / font_info / shape だけを使う)
import * as ffiModule from '../../node_modules/@mizchi/veriss/text-metrics/_build/js/release/build/font_ffi/font_ffi.js';

const ffi = ffiModule as { load_font(d: Uint8Array): number; font_info(h: number): string; shape(h: number, t: string, s: number, axes: string, lang: string, lig: boolean): string };
import type { Measure } from '../protocol.ts';

export async function loadFontMeasure(url: string, family: string): Promise<Measure> {
  const data = new Uint8Array(await (await fetch(url)).arrayBuffer());
  const shaper = {
    load: (d: Uint8Array) => ffi.load_font(d),
    info: (h: number) => JSON.parse(ffi.font_info(h)),
    shape: (h: number, text: string, size: number, axes: Record<string, number>, language = 'und', ligatures = true) =>
      JSON.parse(ffi.shape(h, text, size, JSON.stringify(axes ?? {}), language, ligatures)),
  };
  const metrics = createTextMetrics({ fonts: { [family]: { data, weight: 400 } }, shaper });
  const face = new FontFace(family, data, { weight: '100 900' });
  await face.load();
  document.fonts.add(face);
  const cache = new Map<string, number>();
  return (str, size) => {
    if (!str) return 0;
    const k = `${size}/${str}`;
    let w = cache.get(k);
    if (w === undefined) {
      w = metrics.measureText({ font: { family, size, lineHeight: Math.ceil(size * 1.4), weight: 400 }, text: str }).width;
      cache.set(k, w);
    }
    return w;
  };
}
