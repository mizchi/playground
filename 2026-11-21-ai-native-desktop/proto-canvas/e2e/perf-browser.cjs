// Chromium での 1 フレーム (shell と canvas への描画) の時間。均等化のアニメーション中に測る。
// 使い方: pnpm build && node e2e/perf-browser.cjs
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('playwright');

const root = path.join(__dirname, '..', 'www');
const server = http.createServer((req, res) => {
  const file = path.join(root, req.url === '/' || req.url.startsWith('/?') ? 'index.html' : req.url.split('?')[0]);
  if (!fs.existsSync(file)) return res.writeHead(404).end();
  res.writeHead(200, { 'content-type': file.endsWith('.js') ? 'text/javascript' : 'text/html' }).end(fs.readFileSync(file));
});
(async () => {
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const browser = await chromium.launch();
  console.log('構成                    | shell 平均 / p95 (ms) | 描画 平均 / p95 (ms) | 命令');
  for (const [label, q] of [['窓 3 枚、3 件', ''], ['窓 3 枚、1000 件', '?todos=1000'], ['窓 10 枚、1000 件', '?todos=1000&extra=7']]) {
    const page = await browser.newPage({ viewport: { width: 1680, height: 900 } });
    await page.goto(`http://127.0.0.1:${server.address().port}/${q}`);
    await page.waitForFunction(() => window.lastFrame);
    const samples = await page.evaluate(async () => {
      const s = window.shell;
      s.tiles = { ...s.tiles, ratio: 0.2 };
      s.snapTiles = true;
      s.onChange();
      await new Promise((r) => setTimeout(r, 100));
      s.applyLayout({ type: 'equalize' }, 'human');
      const out = [];
      await new Promise((resolve) => {
        const tick = () => {
          const f = window.lastFrame;
          out.push([f.shellMs, f.drawMs, f.ops]);
          if (out.length < 40) requestAnimationFrame(tick);
          else resolve();
        };
        requestAnimationFrame(tick);
      });
      return out;
    });
    const st = (i) => {
      const xs = samples.map((x) => x[i]).sort((a, b) => a - b);
      return `${(xs.reduce((a, b) => a + b, 0) / xs.length).toFixed(2)} / ${xs[Math.floor(xs.length * 0.95)].toFixed(2)}`;
    };
    console.log(`${label.padEnd(22)} | ${st(0).padStart(20)} | ${st(1).padStart(19)} | ${samples.at(-1)[2]}`);
    await page.close();
  }
  await browser.close();
  server.close();
})();
