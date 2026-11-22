// Odin + wgpu (ブラウザでは WebGPU) の backend の確認。WebGPU は GPU の無い環境ではソフトウェア実装 (SwiftShader) で動く。
// 使い方: pnpm fonts && pnpm build:odin-gpu && pnpm build && node e2e/odin-gpu.cjs
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
  for (const f of ['gpu.wasm', 'odin.js', 'wgpu.js', 'fonts/ipaexg.ttf']) if (!fs.existsSync(path.join(root, f))) throw new Error(`www/${f} がない (pnpm build:odin-gpu / pnpm fonts)`);
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const base = `http://127.0.0.1:${server.address().port}/`;
  const { loadTextMetrics } = await import('@mizchi/veriss/text/node');
  const metrics = await loadTextMetrics({ fonts: { F: { file: path.join(root, 'fonts', 'ipaexg.ttf'), weight: 400 } } });
  // WebGPU はフル版の Chromium で、フラグを付けたときだけ使える
  const browser = await chromium.launch({ channel: 'chromium', args: ['--enable-unsafe-webgpu'] });
  const shots = {};
  const timing = {};
  for (const [name, q] of [['canvas', '?font=fonts/ipaexg.ttf'], ['gpu', '?backend=odin-gpu&gpu-offscreen=1&font=fonts/ipaexg.ttf']]) {
    const page = await browser.newPage({ viewport: { width: 1680, height: 900 } });
    const errors = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    page.on('console', (m) => m.type() === 'error' && !m.text().startsWith('Failed to load resource') && errors.push(m.text()));
    page.on('response', (r) => r.status() >= 400 && !r.url().endsWith('/favicon.ico') && errors.push(`${r.status()} ${r.url()}`));
    await page.goto(base + q);
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await page.waitForFunction(() => window.lastFrame, null, { timeout: 30000 });
    await page.waitForTimeout(400);
    fs.mkdirSync(path.join(__dirname, '..', 'screenshots'), { recursive: true });
    timing[name] = await page.evaluate(async () => {
      const s = window.shell;
      s.tiles = { ...s.tiles, ratio: 0.25 };
      s.onChange();
      await new Promise((r) => setTimeout(r, 100));
      s.applyLayout({ type: 'equalize' }, 'human');
      const out = [];
      await new Promise((ok) => {
        const tick = () => {
          out.push(window.lastFrame.drawMs);
          if (out.length < 30) requestAnimationFrame(tick);
          else ok();
        };
        requestAnimationFrame(tick);
      });
      out.sort((a, b) => a - b);
      return { mean: out.reduce((a, b) => a + b, 0) / out.length, p95: out[Math.floor(out.length * 0.95)], instances: window.gpuInstances?.() };
    });
    if (name === 'gpu') {
      await step('Odin + wgpu: ページエラーなしで WebGPU に描ける', async () => assert.deepEqual(errors, []));
      // ヘッドレスの Chromium では、WebGPU で canvas に表示するとスクリーンショットに写らず、以降の mapAsync も失敗する
      // (素の JS の WebGPU でも同じ)。そこでオフスクリーンにだけ描き (gpu-offscreen)、GPU から読み戻して比べる
      await step('GPU から読み戻した画素が、同じ描画命令を Canvas 2D で描いたものとほぼ同じ', async () => {
        const r = await page.evaluate(() => window.compareGpu());
        const dir = path.join(__dirname, '..', 'screenshots');
        fs.writeFileSync(path.join(dir, 'backend-odin-gpu.png'), Buffer.from(r.gpuPng.split(',')[1], 'base64'));
        fs.writeFileSync(path.join(dir, 'backend-canvas-same-frame.png'), Buffer.from(r.cpuPng.split(',')[1], 'base64'));
        console.log(`     ${r.width}x${r.height}、大きく違う画素の割合 ${(r.diff * 100).toFixed(2)}%`);
        assert.ok(r.diff < 0.02, `${(r.diff * 100).toFixed(2)}%`);
      });
      await step('Odin + wgpu の文字幅が mizchi/font と 1/64px 以内で一致する', async () => {
        const list = await page.evaluate(() => {
          const c = document.getElementById('screen').getBoundingClientRect();
          return window.shell.frame(c.width, c.height, performance.now()).ops.filter((o) => o[0] === 'text' && o[1].str).map((o) => ({ str: o[1].str, size: o[1].size, odin: window.odinMeasure(o[1].str, o[1].size, 400) }));
        });
        let max = 0;
        for (const t of list) max = Math.max(max, Math.abs(metrics.measureText({ font: { family: 'F', size: t.size, lineHeight: Math.ceil(t.size * 1.4), weight: 400 }, text: t.str }).width - t.odin));
        console.log(`     ${list.length} 個、最大差 ${max.toFixed(4)}px`);
        assert.ok(max <= 1 / 64);
      });
      await step('Odin + wgpu でもクリックで操作できる', async () => {
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
  await step('1 フレームの CPU 時間 (描画命令の変換 + 提出)。GPU はソフトウェア実装なので GPU 側の時間は含めない', async () => {
    console.log(`     canvas 2D: 平均 ${timing.canvas.mean.toFixed(2)}ms / Odin + wgpu: 平均 ${timing.gpu.mean.toFixed(2)}ms (インスタンス ${timing.gpu.instances} 個を draw call 1 回)`);
    assert.ok(timing.gpu.instances > 0, 'GPU に何も描いていない');
    assert.ok(timing.gpu.p95 < 16);
  });
  await browser.close();
  server.close();
  console.log(failures ? `${failures} failed` : 'all passed');
  process.exit(failures ? 1 : 0);
})();
