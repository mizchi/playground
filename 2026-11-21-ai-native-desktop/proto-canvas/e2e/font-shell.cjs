// shell の計測を veriss と同じ mizchi/font にしたとき (?font=)、証明と描画の計測が一致するかを実際のアプリで確かめる。
// 使い方: pnpm fonts && pnpm build && node e2e/font-shell.cjs
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

const root = path.join(__dirname, '..', 'www');
const types = { '.js': 'text/javascript', '.html': 'text/html', '.ttf': 'font/ttf', '.map': 'application/json' };
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
  if (!fs.existsSync(path.join(root, 'fonts', 'ipaexg.ttf'))) throw new Error('pnpm fonts でフォントを www/fonts に置く');
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const { analyzeHTML } = await import('@mizchi/veriss/html');
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1680, height: 900 } });
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`http://127.0.0.1:${server.address().port}/?font=fonts/ipaexg.ttf`);
  await page.evaluate(() => localStorage.clear());
  await page.reload();
  await page.waitForFunction(() => window.lastFrame && window.shellMeasure);

  await step('描いた全ての文字列で、shell の計測 (mizchi/font) と Canvas の measureText (同じフォント) が 1/64px 以内で一致する', async () => {
    const r = await page.evaluate(() => {
      const c = document.getElementById('screen').getBoundingClientRect();
      const ops = window.shell.frame(c.width, c.height, performance.now()).ops.filter((o) => o[0] === 'text' && o[1].str);
      const ctx = document.createElement('canvas').getContext('2d');
      let max = 0;
      let worst = '';
      for (const [, t] of ops) {
        ctx.font = `${t.weight ?? 400} ${t.size}px ShellFont`;
        const d = Math.abs(ctx.measureText(t.str).width - window.shellMeasure(t.str, t.size, t.weight ?? 400));
        if (d > max) {
          max = d;
          worst = t.str;
        }
      }
      return { n: ops.length, max, worst };
    });
    assert.ok(r.max <= 1 / 64, `最大差 ${r.max} ("${r.worst}")`);
    console.log(`     ${r.n} 個の文字列、最大差 ${r.max.toFixed(4)}px`);
  });

  await step('各窓の最小幅: 実際のアプリのレイアウトと、veriss の証明が一致する', async () => {
    const rows = [];
    for (const app of ['todo', 'settings', 'notes']) {
      const low = await page.evaluate((a) => window.lowerFor(a, true), app);
      const rep = analyzeHTML(low.html, { name: app, css: low.css, boxes: low.boxes, viewport: { min: 120, max: 700, height: { min: 300, max: 1200 } } });
      const errEnd = Math.max(120, ...rep.diagnostics.filter((d) => d.severity === 'error').map((d) => Number(d.range.max)));
      const appMin = await page.evaluate((a) => window.lastFrame.windows.find((w) => w.app === a).minWidth, app);
      rows.push(`${app} アプリ ${appMin} / veriss ${errEnd}`);
      assert.equal(appMin, errEnd, `${app}: アプリ ${appMin} / veriss ${errEnd}`);
    }
    console.log(`     ${rows.join('、')}`);
  });

  await step('ページエラーなし', async () => assert.deepEqual(errors, []));
  fs.mkdirSync(path.join(__dirname, '..', 'screenshots'), { recursive: true });
  await page.screenshot({ path: path.join(__dirname, '..', 'screenshots', 'font-shell.png') });
  await browser.close();
  server.close();
  console.log(failures ? `${failures} failed` : 'all passed');
  process.exit(failures ? 1 : 0);
})();
