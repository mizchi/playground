// 実ブラウザでの確認: カレンダー (プラットフォームのアプリの例)。ランチャーから開く → 日を押す → 予定を入れて Enter → 再読み込みで残る。
// 使い方: pnpm build && node e2e/calendar.cjs [--shots]
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
  const context = await browser.newContext({ viewport: { width: 1500, height: 860 }, deviceScaleFactor: shots ? 2 : 1 });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  await page.evaluate(() => localStorage.clear());
  await page.reload();
  await page.waitForFunction(() => window.platform);
  if (shots) fs.mkdirSync(shotsDir, { recursive: true });
  const shot = async (name) => shots && (await page.screenshot({ path: path.join(shotsDir, `${name}.png`) }));
  // 2 フレーム待ってから (アニメーションが始まる前の古いフレームを見ないように)、止まるのを待つ
  const settle = async () => {
    await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
    await page.waitForFunction(() => window.lastFrame && !window.lastFrame.animating);
  };
  const node = (id) => page.evaluate((id) => window.shell.tree('calendar').get(id), id);
  const clickNode = async (id) => {
    const r = await page.evaluate((id) => window.shell.accessibility().rects.get(`calendar/${id}`), id);
    assert.ok(r, `${id} が描かれていない`);
    const c = await page.locator('#screen').boundingBox();
    await page.mouse.click(c.x + r.x + r.w / 2, c.y + r.y + r.h / 2);
  };
  const today = new Date();

  await step('ランチャーから Calendar を開くと、今月の表で今日が選ばれている', async () => {
    await page.keyboard.press('Alt+KeyL');
    await settle();
    await page.evaluate(() => window.shell.activate('launcher', 'open-calendar'));
    await settle();
    await page.keyboard.press('Alt+Enter'); // 全体に広げる
    await settle();
    assert.equal((await node('title')).label, `${today.getFullYear()}年${today.getMonth() + 1}月`);
    const sel = await page.evaluate(() => [...window.shell.tree('calendar').nodes.values()].filter((n) => n.selected).map((n) => n.label));
    assert.deepEqual(sel, [String(today.getDate())]);
  });

  await step('列が揃っている (各行の同じ曜日のセルの x が同じ)', async () => {
    const xs = await page.evaluate(() => {
      const a = window.shell.accessibility();
      const t = window.shell.tree('calendar');
      return t.get('grid').children.slice(1).map((w) => t.get(w).children.map((c) => a.rects.get(`calendar/${c}`)?.x ?? null));
    });
    const col0 = xs.map((r) => r[6]).filter((x) => x !== null);
    assert.ok(col0.length >= 3);
    assert.ok(col0.every((x) => Math.abs(x - col0[0]) < 0.5), `土曜の列 ${col0}`);
  });

  await step('日を押して予定を入れ、Enter で追加すると • が付く', async () => {
    const id = await page.evaluate(() => [...window.shell.tree('calendar').nodes.values()].find((n) => n.role === 'cell' && n.label === '15').id);
    await clickNode(id);
    assert.match((await node('dayTitle')).label, /15日/);
    await clickNode('input');
    await page.keyboard.insertText('歯医者 10:00');
    await page.keyboard.press('Enter');
    assert.equal((await node(id)).label, '15•');
    assert.equal((await node('e0t')).label, '歯医者 10:00');
    await shot('calendar-01');
  });

  await step('再読み込みしても予定と窓が残る', async () => {
    await page.reload();
    await page.waitForFunction(() => window.platform && window.shell.tree('calendar').get('grid'));
    const cell = await page.evaluate(() => [...window.shell.tree('calendar').nodes.values()].find((n) => n.role === 'cell' && n.label?.startsWith('15')).label);
    assert.equal(cell, '15•');
  });

  await step('スクリーンリーダー向けのミラーは表として出る', async () => {
    await page.waitForSelector('#a11y [role=table] [role=columnheader]');
    assert.equal(await page.locator('#a11y [role=table] [role=row]').first().locator('[role=columnheader]').count(), 7);
  });

  await step('ページエラーなし', async () => assert.deepEqual(errors, []));
  await browser.close();
  server.close();
  console.log(failures ? `${failures} failed` : 'all passed');
  process.exit(failures ? 1 : 0);
})();
