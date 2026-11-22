// 実ブラウザでの確認: テーマ (システムに従う / ライト / ダーク)。Canvas の画素が theme.ts の値で描かれ、切り替えが保存される。
// 使い方: pnpm build && node e2e/theme.cjs [--shots]
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

const root = path.join(__dirname, '..', 'www');
const shotsDir = path.join(__dirname, '..', 'screenshots');
const types = { '.html': 'text/html', '.js': 'text/javascript', '.map': 'application/json', '.wasm': 'application/wasm' };
const server = http.createServer((req, res) => {
  const p = req.url.split('?')[0];
  const file = path.join(root, p === '/' ? 'index.html' : p);
  if (!file.startsWith(root) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) return res.writeHead(404).end();
  res.writeHead(200, { 'content-type': types[path.extname(file)] ?? 'application/octet-stream' }).end(fs.readFileSync(file));
});
// src/theme.ts と同じ値 (window 面と背景)
const SURFACE = { light: [255, 255, 255], dark: [0x1f, 0x22, 0x2b] };
const shots = process.argv.includes('--shots');
let failures = 0;
async function step(name, fn) {
  try {
    await fn();
    console.log(`ok   ${name}`);
  } catch (e) {
    failures++;
    console.log(`FAIL ${name}\n     ${e.message.split('\n').join('\n     ')}`);
  }
}

(async () => {
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const browser = await chromium.launch();
  const context = await browser.newContext({ viewport: { width: 1500, height: 860 }, deviceScaleFactor: shots ? 2 : 1, colorScheme: 'light' });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  await page.evaluate(() => localStorage.clear());
  await page.reload();
  await page.waitForFunction(() => window.platform);
  if (shots) fs.mkdirSync(shotsDir, { recursive: true });
  const settle = async () => {
    await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
    await page.waitForFunction(() => window.lastFrame && !window.lastFrame.animating);
  };
  /** Todo の窓の中 (文字のない所) の画素 */
  const windowPixel = () =>
    page.evaluate(() => {
      const c = document.getElementById('screen');
      const r = c.getBoundingClientRect();
      const t = window.lastFrame.windows.find((w) => w.app === 'todo').tile;
      const dpr = c.width / r.width;
      const d = c.getContext('2d').getImageData(Math.round((t.x + t.w - 20) * dpr), Math.round((t.y + t.h - 20) * dpr), 1, 1).data;
      return [d[0], d[1], d[2]];
    });
  const near = (a, b) => a.every((v, i) => Math.abs(v - b[i]) <= 2);

  await step('既定 (システムに従う、OS はライト) はライトで描く', async () => {
    await settle();
    assert.equal(await page.inputValue('#theme'), 'system');
    assert.ok(near(await windowPixel(), SURFACE.light), String(await windowPixel()));
  });

  await step('OS がダークになると、Canvas もダークで描き直す', async () => {
    await page.emulateMedia({ colorScheme: 'dark' });
    await settle();
    assert.ok(near(await windowPixel(), SURFACE.dark), String(await windowPixel()));
  });

  await step('パネルでライトを選ぶと、OS がダークでもライト', async () => {
    await page.selectOption('#theme', 'light');
    await settle();
    assert.ok(near(await windowPixel(), SURFACE.light), String(await windowPixel()));
  });

  await step('ダークを選ぶと、再読み込みしてもダーク (保存される)', async () => {
    await page.emulateMedia({ colorScheme: 'light' });
    await page.selectOption('#theme', 'dark');
    await settle();
    assert.ok(near(await windowPixel(), SURFACE.dark));
    await page.reload();
    await page.waitForFunction(() => window.platform);
    await settle();
    assert.equal(await page.inputValue('#theme'), 'dark');
    assert.ok(near(await windowPixel(), SURFACE.dark), String(await windowPixel()));
  });

  await step('ダークで Calendar と Terminal を開いた画面', async () => {
    await page.evaluate(() => {
      window.platform.launch('calendar');
      window.platform.launch('terminal');
    });
    await page.waitForFunction(() => /libghostty-vt/.test(window.shell.tree('terminal').get('screen')?.value ?? ''));
    await settle();
    if (shots) await page.screenshot({ path: path.join(shotsDir, 'theme-dark.png') });
  });

  await step('ページエラーなし', async () => assert.deepEqual(errors, []));
  await browser.close();
  server.close();
  console.log(failures ? `${failures} failed` : 'all passed');
  process.exit(failures ? 1 : 0);
})();
