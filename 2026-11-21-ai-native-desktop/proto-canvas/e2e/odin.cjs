// Odin backend (js_wasm32) の確認: 同じ描画命令を Odin が描けるか、Odin の文字幅が mizchi/font と一致するか。
// 使い方: pnpm fonts && pnpm build:odin && pnpm build && node e2e/odin.cjs
// 状態: 下書き。Odin のコンパイラが使えるようになったら動かす。
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

const root = path.join(__dirname, '..', 'www');
const types = { '.js': 'text/javascript', '.html': 'text/html', '.ttf': 'font/ttf', '.wasm': 'application/wasm', '.map': 'application/json' };
const server = http.createServer((req, res) => {
  const file = path.join(root, req.url === '/' || req.url.startsWith('/?') ? 'index.html' : req.url.split('?')[0]);
  if (!file.startsWith(root) || !fs.existsSync(file)) return res.writeHead(404).end();
  res.writeHead(200, { 'content-type': types[path.extname(file)] ?? 'application/octet-stream' }).end(fs.readFileSync(file));
});
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
  for (const f of ['raster.wasm', 'odin.js', 'fonts/ipaexg.ttf']) if (!fs.existsSync(path.join(root, f))) throw new Error(`www/${f} がない (pnpm build:odin / pnpm fonts)`);
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const base = `http://127.0.0.1:${server.address().port}/`;
  const { loadTextMetrics } = await import('@mizchi/veriss/text/node');
  const metrics = await loadTextMetrics({ fonts: { F: { file: path.join(root, 'fonts', 'ipaexg.ttf'), weight: 400 } } });
  const browser = await chromium.launch();
  const shots = {};
  for (const [name, q] of [['canvas', '?font=fonts/ipaexg.ttf'], ['odin', '?backend=odin&font=fonts/ipaexg.ttf']]) {
    const page = await browser.newPage({ viewport: { width: 1680, height: 900 } });
    const errors = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    await page.goto(base + q);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await page.waitForFunction(() => window.lastFrame);
    await page.waitForTimeout(300);
    shots[name] = await page.locator('#screen').screenshot();
    fs.mkdirSync(path.join(__dirname, '..', 'screenshots'), { recursive: true });
    fs.writeFileSync(path.join(__dirname, '..', 'screenshots', `backend-${name}.png`), shots[name]);
    if (name === 'odin') {
      await step('Odin backend: ページエラーなしで描ける', async () => assert.deepEqual(errors, []));
      await step('Odin の文字幅 (stb_truetype) が mizchi/font と 1/64px 以内で一致する', async () => {
        const list = await page.evaluate(() => {
          const c = document.getElementById('screen').getBoundingClientRect();
          return window.shell.frame(c.width, c.height, performance.now()).ops.filter((o) => o[0] === 'text' && o[1].str).map((o) => ({ str: o[1].str, size: o[1].size, odin: window.odinMeasure(o[1].str, o[1].size, 400) }));
        });
        let max = 0;
        let worst = '';
        for (const t of list) {
          const m = metrics.measureText({ font: { family: 'F', size: t.size, lineHeight: Math.ceil(t.size * 1.4), weight: 400 }, text: t.str }).width;
          const d = Math.abs(m - t.odin);
          if (d > max) [max, worst] = [d, `${t.str} (${t.size}px): odin ${t.odin} / font ${m}`];
        }
        console.log(`     ${list.length} 個、最大差 ${max.toFixed(4)}px`);
        assert.ok(max <= 1 / 64, worst);
      });
      await step('Odin backend でもクリックで操作できる (入力は canvas backend のまま)', async () => {
        const op = await page.evaluate(() => {
          const c = document.getElementById('screen').getBoundingClientRect();
          const o = window.shell.frame(c.width, c.height, performance.now()).ops.find((o) => o[0] === 'text' && o[1].str === '牛乳を買う');
          return { x: c.left + o[1].x - 14, y: c.top + o[1].y };
        });
        await page.mouse.click(op.x, op.y);
        assert.equal(await page.evaluate(() => window.shell.tree('todo').get('i1').checked), true);
      });
    }
    await page.close();
  }
  await step('Canvas 版と Odin 版の画面は、ほぼ同じ (画素の差が小さい)', async () => {
    const page = await browser.newPage();
    const diff = await page.evaluate(
      async ([a, b]) => {
        const load = (u) => new Promise((ok) => { const i = new Image(); i.onload = () => ok(i); i.src = u; });
        const [ia, ib] = await Promise.all([load(a), load(b)]);
        const c = document.createElement('canvas');
        c.width = ia.width;
        c.height = ia.height;
        const g = c.getContext('2d');
        g.drawImage(ia, 0, 0);
        const da = g.getImageData(0, 0, c.width, c.height).data;
        g.drawImage(ib, 0, 0);
        const db = g.getImageData(0, 0, c.width, c.height).data;
        let n = 0;
        for (let i = 0; i < da.length; i += 4) if (Math.abs(da[i] - db[i]) + Math.abs(da[i + 1] - db[i + 1]) + Math.abs(da[i + 2] - db[i + 2]) > 96) n++;
        return n / (da.length / 4);
      },
      [`data:image/png;base64,${shots.canvas.toString('base64')}`, `data:image/png;base64,${shots.odin.toString('base64')}`],
    );
    console.log(`     大きく違う画素の割合 ${(diff * 100).toFixed(2)}%`);
    assert.ok(diff < 0.02, `${(diff * 100).toFixed(2)}%`);
  });
  await browser.close();
  server.close();
  console.log(failures ? `${failures} failed` : 'all passed');
  process.exit(failures ? 1 : 0);
})();
