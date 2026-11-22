// ダークテーマ: 配色の正 (theme.ts) と index.html の一致、veriss によるライト・ダーク両方のコントラスト検査。
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { analyzeHTML } from '@mizchi/veriss/html';
import { createCalendarApp } from '../src/apps/calendar.ts';
import { builtinManifests } from '../src/apps/manifests.ts';
import { terminalManifest } from '../src/apps/terminal/terminal.ts';
import { loadVtFactory } from '../src/apps/terminal/vt.ts';
import { memoryStorage } from '../src/cache.ts';
import { layout } from '../src/layout.ts';
import { contrastSpecimen, lower } from '../src/lower.ts';
import type { ColorPair } from '../src/lower.ts';
import { createPlatform } from '../src/platform/platform.ts';
import { memoryKeyValue } from '../src/platform/services.ts';
import type { ColorToken } from '../src/protocol.ts';
import { Shell } from '../src/shell.ts';
import { CHROME_PAIRS, THEMES } from '../src/theme.ts';
import type { Palette } from '../src/theme.ts';

const measure = (s: string, size: number, weight: number) => [...s].length * size * (weight >= 600 ? 0.66 : 0.6);
const HEIGHT = { min: 300, max: 1200 };
const wasm = await readFile(new URL('../node_modules/ghostty-web/ghostty-vt.wasm', import.meta.url));
const ALL = ['todo', 'settings', 'notes', 'launcher', 'calendar', 'terminal'];

const platform = async () => {
  const p = createPlatform({
    apps: [...builtinManifests.map((m) => (m.id === 'calendar' ? { ...m, create: (id: string) => createCalendarApp({ id, now: () => new Date(2026, 10, 22) }) } : m)), terminalManifest(() => loadVtFactory(wasm))],
    initial: ALL,
    measure,
    templates: memoryStorage(),
    kv: memoryKeyValue(),
  });
  await new Promise((r) => setTimeout(r, 20)); // ターミナルの wasm の読み込み
  return p;
};
// LayoutOptions の型に accessibility が無い (mizchi/veriss#8) ので、ここで 1 回だけ型を合わせる
const analyze = (html: string, opts: Record<string, unknown>) => analyzeHTML(html, opts as unknown as Parameters<typeof analyzeHTML>[1]);
const a11yErrors = (r: { diagnostics: { code: string; severity: string }[] }) => r.diagnostics.filter((d) => /A11Y/.test(d.code) && d.severity === 'error');

test('index.html の CSS 変数 (ライト・ダーク) は theme.ts と同じ', async () => {
  const html = await readFile(new URL('../www/index.html', import.meta.url), 'utf8');
  const block = (re: RegExp) => {
    const m = html.match(re);
    assert.ok(m, String(re));
    return Object.fromEntries([...m[1].matchAll(/--c-(\w+):\s*(#[0-9a-f]{6})/gi)].map((x) => [x[1], x[2].toLowerCase()]));
  };
  assert.deepEqual(block(/:root\s*\{([^}]*)\}/), THEMES.light);
  assert.deepEqual(block(/:root\[data-theme="dark"\]\s*\{([^}]*)\}/), THEMES.dark);
  // OS の設定に従う場合 (data-theme が light でないとき) も同じ値
  assert.deepEqual(block(/@media \(prefers-color-scheme: dark\)\s*\{\s*:root:not\(\[data-theme="light"\]\)\s*\{([^}]*)\}/), THEMES.dark);
});

test('lowering はダークの配色を prefers-color-scheme: dark の色の宣言として出す', async () => {
  const p = await platform();
  const low = lower(p.shell.viewOf('todo').view, measure, { pxIntrinsic: true });
  assert.match(low.css, /@media \(prefers-color-scheme: dark\)/);
  assert.ok(low.css.includes(`color: ${THEMES.dark.text}`));
  assert.ok(low.css.includes(`background: ${THEMES.dark.surface}`));
  assert.ok(!lower(p.shell.viewOf('todo').view, measure, { dark: null }).css.includes('prefers-color-scheme'));
});

// view を丸ごと veriss に通す (配置 + 操作対象の大きさ + 名前 + ライト・ダークのコントラスト)
for (const app of ['todo', 'settings', 'notes', 'launcher', 'terminal']) {
  test(`veriss: ${app} の view は、ライトとダークの両方で verified (コントラストを含む)`, async () => {
    const p = await platform();
    const { view } = p.shell.viewOf(app);
    const min = layout(view, 0, measure).minWidth;
    const low = lower(view, measure, { pxIntrinsic: true, scroll: true });
    const r = analyze(low.html, { name: app, css: low.css, boxes: low.boxes, scrollbars: low.scrollbars, accessibility: true, viewport: { min, max: 900, height: HEIGHT } });
    assert.equal(r.status, 'verified', JSON.stringify(r.diagnostics.slice(0, 3)));
  });
}

test('veriss: カレンダーの表は、等幅の行で崩れない (配置。コントラストは下の見本で検査する)', async () => {
  // a11y と縦方向の検査を同時にすると、この大きさの表では veriss の時間が伸びる (mizchi/veriss#12)。配置は integrity で証明する
  const shell = new Shell([createCalendarApp({ now: () => new Date(2026, 10, 22) })], measure, memoryStorage());
  const { view } = shell.viewOf('calendar');
  const min = layout(view, 0, measure).minWidth;
  const low = lower(view, measure, { pxIntrinsic: true, scroll: true, dark: null });
  const r = analyzeHTML(low.html, { name: 'calendar', css: low.css, boxes: low.boxes, scrollbars: low.scrollbars, integrity: true, viewport: { min, max: 900, height: HEIGHT } });
  assert.equal(r.status, 'verified', JSON.stringify(r.diagnostics.slice(0, 3)));
});

/** すべてのアプリの view と shell 自身の UI が使う色の組 */
const allPairs = async (): Promise<ColorPair[]> => {
  const p = await platform();
  const fromApps = ALL.flatMap((app) => lower(p.shell.viewOf(app).view, measure).pairs);
  return [...fromApps, ...CHROME_PAIRS.map(({ fg, bg, size, weight }) => ({ fg, bg, size, weight }))];
};
const checkPairs = (pairs: ColorPair[], dark?: Palette) => {
  const sp = contrastSpecimen(pairs, measure, { dark });
  return analyze(sp.html, { name: 'specimen', css: sp.css, boxes: sp.boxes, scrollbars: sp.scrollbars, accessibility: true, viewport: { min: 320, max: 900, height: HEIGHT } });
};

test('veriss: 全アプリ (カレンダー・ターミナルを含む) と shell の UI の色の組は、ライトとダークで WCAG AA を満たす', async () => {
  const pairs = await allPairs();
  const kinds = new Set(pairs.map((p) => `${p.fg}/${p.bg}`));
  for (const k of ['text/surface', 'muted/surface', 'accentText/accent', 'accentText/danger', 'accentText/ghost', 'text/bg']) assert.ok(kinds.has(k), `${k} が見本にない`);
  const r = checkPairs(pairs);
  assert.equal(r.status, 'verified', JSON.stringify(a11yErrors(r).slice(0, 3)));
});

test('veriss: ダークだけ崩れた配色は、ダークのコントラスト違反として見つかる', async () => {
  const pairs = await allPairs();
  // 補足の文字を枠線の色にしてしまった、ありがちな誤り (ライトは正しいまま)
  const bad: Palette = { ...THEMES.dark, muted: THEMES.dark.border };
  const r = checkPairs(pairs, bad);
  assert.equal(r.status, 'violated');
  const errs = a11yErrors(r) as unknown as { code: string; message: string; preferences?: unknown; state?: unknown }[];
  assert.ok(errs.length > 0);
  assert.ok(errs.every((e) => e.code === 'E_A11Y_CONTRAST'), errs.map((e) => e.code).join());
  assert.ok(JSON.stringify(errs).includes('dark'), 'ダークのときの違反として報告される');
  // ライトの配色を崩したときも見つかる (ライトの検査も効いている)
  const sp = contrastSpecimen(pairs, measure, { theme: { ...THEMES.light, accentText: THEMES.light.border } as Record<ColorToken, string> });
  const lr = analyze(sp.html, { name: 'light', css: sp.css, boxes: sp.boxes, scrollbars: sp.scrollbars, accessibility: true, viewport: { min: 320, max: 900, height: HEIGHT } });
  assert.equal(lr.status, 'violated');
});
