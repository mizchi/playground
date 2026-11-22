// スクリーンリーダー向けのミラー DOM の検証: アクセシビリティツリー (Playwright の ariaSnapshot)、
// 支援技術からの操作 (click)、提案の読み上げ (aria-live)、axe-core の規則。
// 使い方: pnpm build && node e2e/a11y.cjs
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

const root = path.join(__dirname, '..', 'www');
const axePath = require.resolve('axe-core/axe.min.js');
const server = http.createServer((req, res) => {
  const file = path.join(root, req.url === '/' ? 'index.html' : req.url.split('?')[0]);
  if (!file.startsWith(root) || !fs.existsSync(file)) return res.writeHead(404).end();
  res.writeHead(200, { 'content-type': file.endsWith('.js') ? 'text/javascript' : 'text/html' }).end(fs.readFileSync(file));
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
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1680, height: 900 } });
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  await page.evaluate(() => localStorage.clear());
  await page.reload();
  await page.waitForFunction(() => document.querySelector('#a11y [role=checkbox]'));
  const mirror = page.locator('#a11y');
  const node = (app, id) => page.evaluate(([a, i]) => window.shell.tree(a).get(i), [app, id]);
  const settle = async () => {
    await page.waitForTimeout(100);
    await page.waitForFunction(() => !window.lastFrame.animating);
    await page.waitForTimeout(50);
  };

  await step('アクセシビリティツリーに、窓・見出し・部品が役割と名前で出る', async () => {
    const snap = await mirror.ariaSnapshot();
    for (const want of ['region "Todo"', 'heading "やること"', 'checkbox "牛乳を買う"', 'checkbox "請求書を送る" [checked]', 'switch "利用状況を送信"', 'textbox "新しいタスク"', 'button "追加"', 'button "全データを削除"', 'region "Notes"'])
      assert.ok(snap.includes(want), `"${want}" がない\n${snap}`);
    fs.mkdirSync(path.join(__dirname, '..', 'screenshots'), { recursive: true });
    fs.writeFileSync(path.join(__dirname, '..', 'screenshots', 'aria-snapshot.yml'), snap);
  });

  await step('ミラーの要素は、Canvas 上の widget の位置に重なる', async () => {
    const box = await page.getByRole('checkbox', { name: '牛乳を買う' }).boundingBox();
    const canvas = await page.locator('#screen').boundingBox();
    const op = await page.evaluate(() => {
      const r = document.getElementById('screen').getBoundingClientRect();
      const o = window.shell.frame(r.width, r.height, performance.now()).ops.find((o) => o[0] === 'text' && o[1].str === '牛乳を買う');
      return o[1];
    });
    const lx = canvas.x + op.x;
    const ly = canvas.y + op.y;
    assert.ok(lx >= box.x && lx <= box.x + box.width && ly >= box.y && ly <= box.y + box.height, `ラベル (${lx},${ly}) が枠 ${JSON.stringify(box)} の中`);
  });

  await step('支援技術からの操作 (click) は、人の意味的な操作になる', async () => {
    await page.getByRole('checkbox', { name: '牛乳を買う' }).dispatchEvent('click');
    assert.equal((await node('todo', 'i1')).checked, true);
    await settle();
    assert.ok((await mirror.ariaSnapshot()).includes('checkbox "牛乳を買う" [checked]'), '状態がミラーに反映される');
    const audit = await page.evaluate(() => window.shell.audit.at(-1));
    assert.equal(audit.by, 'human');
  });

  await step('入力欄を選ぶと IME の受け口にフォーカスが移り、入力した値がミラーに出る', async () => {
    await page.getByRole('textbox', { name: '新しいタスク' }).dispatchEvent('click');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'sink');
    await page.keyboard.insertText('卵を買う');
    await settle();
    assert.equal(await page.getByRole('textbox', { name: '新しいタスク' }).textContent(), '卵を買う');
  });

  await step('AI の提案と拒否は aria-live で読み上げられる', async () => {
    await page.evaluate(() => window.agent.propose('全データを削除する', [{ app: 'settings', target: 'reset', action: 'click' }]));
    await page.waitForFunction(() => document.querySelector('#a11y .live').textContent.includes('AI の提案'));
    assert.match(await page.locator('#a11y .live').textContent(), /承認が必要です/);
    await page.evaluate(() => window.shell.dispatch({ app: 'todo', target: 'i2', action: 'click', by: 'agent' }));
    await page.waitForFunction(() => document.querySelector('#a11y .live').textContent.includes('拒否'));
  });

  await step('最小化した窓はドックのボタンになり、押すと戻る', async () => {
    await page.evaluate(() => window.shell.minimizeApp('settings'));
    await settle();
    const restore = page.getByRole('button', { name: 'Settings を元に戻す' });
    assert.equal(await restore.count(), 1);
    assert.equal(await page.getByRole('region', { name: 'Settings' }).count(), 0, '最小化した窓のツリーは出さない');
    await restore.dispatchEvent('click');
    await settle();
    assert.equal(await page.getByRole('region', { name: 'Settings' }).count(), 1);
  });

  await step('axe-core の規則に違反しない (見た目は Canvas なので color-contrast は除く)', async () => {
    await page.addScriptTag({ path: axePath });
    const r = await page.evaluate(async () => {
      const res = await window.axe.run('#a11y', { rules: { 'color-contrast': { enabled: false } } });
      return res.violations.map((v) => `${v.id}: ${v.help} (${v.nodes.length})`);
    });
    assert.deepEqual(r, []);
  });

  await step('ページエラーなし', async () => assert.deepEqual(errors, []));
  await browser.close();
  server.close();
  console.log(failures ? `${failures} failed` : 'all passed');
  process.exit(failures ? 1 : 0);
})();
