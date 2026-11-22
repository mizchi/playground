// 実際のフォント 1 つ (IPAex ゴシック) で、証明・shell の計測・描画が同じ幅になるかを確かめる (VALIDATION.md の 4c)。
// (1) 文字列ごとの幅: mizchi/font (veriss の計測) / Chromium の canvas.measureText / Chromium の DOM
// (2) 窓の最小幅: layout.ts (mizchi/font で計測) / veriss / Chromium の DOM で配置した実測
// 使い方: node e2e/font-diff.ts [フォントのパス]   (Node 24 以上、Playwright の Chromium)
import fs from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { analyzeHTML } from '@mizchi/veriss/html';
import { loadTextMetrics } from '@mizchi/veriss/text/node';
import { createNotesApp } from '../src/apps/notes.ts';
import { createSettingsApp } from '../src/apps/settings.ts';
import { createTodoApp } from '../src/apps/todo.ts';
import { memoryStorage } from '../src/cache.ts';
import { layout } from '../src/layout.ts';
import { lower } from '../src/lower.ts';
import { Shell } from '../src/shell.ts';

const FONT = process.argv[2] ?? '/usr/share/fonts/opentype/ipaexfont-gothic/ipaexg.ttf';
const FAMILY = 'IPAexGothic';
const { chromium } = createRequire(import.meta.url)('playwright') as typeof import('playwright');

const metrics = await loadTextMetrics({ fonts: { [FAMILY]: { file: FONT, weight: 400 } } });
// フォントは regular 1 つ。ブラウザ側も同じ face を 100〜900 に当てるので、太字の合成は起きない
const measure = (str: string, size: number) => (str ? metrics.measureText({ font: { family: FAMILY, size, lineHeight: Math.ceil(size * 1.4), weight: 400 }, text: str }).width : 0);

// ---- 試作が描く文字列を集める (shell を mizchi/font の計測で動かす) ----
const shell = new Shell([createTodoApp(), createSettingsApp(), createNotesApp()], (s, z) => measure(s, z), memoryStorage());
const frame = shell.frame(1400, 800, 0);
const texts = new Map<string, { str: string; size: number; weight: number }>();
for (const o of frame.ops) if (o[0] === 'text' && o[1].str) texts.set(`${o[1].size}/${o[1].weight ?? 400}/${o[1].str}`, { str: o[1].str, size: o[1].size, weight: o[1].weight ?? 400 });

// 試作の文字列に加え、崩れやすい組み合わせを足す (カーニング、全角の約物、数字、混在)
const EXTRA = [
  'AVATAR To Ty Wa We Yo', 'office ffi fl', '「保存」しました。', '（全角括弧）と、読点。', '0123456789 1,234.56',
  'PR をレビューする (v2.0)', 'あいうえお カタカナ 漢字', 'Mixed 日本語 and English テキスト', '……――〜', '#$%&@*+=<>?',
  'iiiiiiiiii WWWWWWWWWW', '最小幅 252px を下回る', 'ｈａｌｆ／ｆｕｌｌ ｗｉｄｔｈ', '①②③ ㈱ ℃', 'Ｑ＆Ａ：よくある質問',
];
for (const str of EXTRA) for (const [size, weight] of [[12, 400], [14, 400], [14, 600], [18, 700]] as const) texts.set(`${size}/${weight}/${str}`, { str, size, weight });

const browser = await chromium.launch();
const page = await browser.newPage();
const fontBytes = fs.readFileSync(FONT);
await page.route('http://font.test/**', (r) =>
  r.request().url().endsWith('.ttf')
    ? r.fulfill({ body: fontBytes, contentType: 'font/ttf' })
    : r.fulfill({ body: `<!doctype html><style>@font-face{font-family:${FAMILY};src:url(/f.ttf);font-weight:100 900}</style><body></body>`, contentType: 'text/html' }),
);
await page.goto('http://font.test/');
await page.evaluate(async (fam) => {
  await document.fonts.load(`16px ${fam}`);
}, FAMILY);

// ---- (1) 文字列ごとの幅 ----
const list = [...texts.values()];
const browserWidths = await page.evaluate(
  ({ list, fam }) => {
    const ctx = document.createElement('canvas').getContext('2d')!;
    return list.map((t) => {
      ctx.font = `${t.weight} ${t.size}px ${fam}`;
      const span = document.createElement('span');
      span.style.cssText = `font: ${t.weight} ${t.size}px ${fam}; white-space: nowrap; position: absolute;`;
      span.textContent = t.str;
      document.body.append(span);
      const dom = span.getBoundingClientRect().width;
      span.remove();
      return { canvas: ctx.measureText(t.str).width, dom };
    });
  },
  { list, fam: FAMILY },
);
const rows = list.map((t, i) => ({ ...t, font: measure(t.str, t.size), ...browserWidths[i] }));
const UNIT = 1 / 64;
const diff = (a: number, b: number) => Math.abs(a - b);
const over = (k: 'canvas' | 'dom') => rows.filter((r) => diff(r.font, r[k]) > UNIT);
const maxDiff = (k: 'canvas' | 'dom') => Math.max(...rows.map((r) => diff(r.font, r[k])));
console.log(`## 文字列ごとの幅 (${rows.length} 個、フォント ${path.basename(FONT)})`);
console.log(`mizchi/font と canvas.measureText: 最大差 ${maxDiff('canvas').toFixed(4)}px、1/64px を超えるもの ${over('canvas').length} 個`);
console.log(`mizchi/font と DOM:                最大差 ${maxDiff('dom').toFixed(4)}px、1/64px を超えるもの ${over('dom').length} 個`);
for (const r of [...over('canvas'), ...over('dom')].slice(0, 8))
  console.log(`  "${r.str}" ${r.size}px w${r.weight}: font ${r.font.toFixed(3)} / canvas ${r.canvas.toFixed(3)} / dom ${r.dom.toFixed(3)}`);

// ---- (2) 窓の最小幅: layout.ts / veriss / Chromium の DOM ----
console.log('\n## 窓の最小幅 (px)');
const probe = await browser.newPage();
await probe.route('http://font.test/**', (r) =>
  r.request().url().endsWith('.ttf') ? r.fulfill({ body: fontBytes, contentType: 'font/ttf' }) : r.fulfill({ body: '<!doctype html>', contentType: 'text/html' }),
);
await probe.goto('http://font.test/');
const results: Record<string, unknown>[] = [];
for (const app of ['todo', 'settings', 'notes']) {
  const { view } = shell.viewOf(app);
  const min = layout(view, 0, (s, z) => measure(s, z)).minWidth;
  const low = lower(view, (s, z) => measure(s, z), { pxIntrinsic: true, fontFamily: FAMILY });
  const report = analyzeHTML(low.html, { name: app, css: low.css, boxes: low.boxes, viewport: { min: 120, max: 700, height: { min: 300, max: 1200 } } });
  const errEnd = Math.max(120, ...report.diagnostics.filter((d) => d.severity === 'error').map((d) => Number(d.range!.max)));
  await probe.setContent(`<!doctype html><style>@font-face{font-family:${FAMILY};src:url(http://font.test/f.ttf);font-weight:100 900}${low.css}</style><body>${low.html}</body>`);
  await probe.evaluate(async (fam) => {
    await document.fonts.load(`14px ${fam}`);
  }, FAMILY);
  let browserMin = 120;
  for (let w = 700; w >= 120; w--) {
    await probe.setViewportSize({ width: w, height: 800 });
    const bad = await probe.evaluate((w) => {
      for (const el of document.querySelectorAll('main *')) {
        const a = el.getBoundingClientRect();
        const pe = el.parentElement!;
        const p = pe.getBoundingClientRect();
        const cs = getComputedStyle(pe);
        const right = p.right - parseFloat(cs.paddingRight) - parseFloat(cs.borderRightWidth);
        // 葉の文字は「契約の幅 (切り上げ) の中で中身がはみ出さないか」も見る
        if (a.right > right + 0.5 || a.right > w + 0.5) return true;
        if (el.children.length === 0 && el.textContent && el.scrollWidth > el.clientWidth + 0.5) return true;
      }
      return false;
    }, w);
    if (bad) {
      browserMin = w + 1;
      break;
    }
  }
  const row = { app, layoutMin: min, veriss: report.status === 'unknown' ? 'unknown' : errEnd, browserMin };
  results.push(row);
  console.log(`${app.padEnd(9)} layout.ts ${min} / veriss ${row.veriss} / Chromium ${browserMin}`);
}
await browser.close();
fs.writeFileSync(path.join(import.meta.dirname, '..', 'font-diff.json'), JSON.stringify({ font: FONT, strings: rows, minWidths: results }, null, 2));
