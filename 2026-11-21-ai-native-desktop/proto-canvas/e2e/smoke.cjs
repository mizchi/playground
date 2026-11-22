// 実ブラウザでの通し確認: 意味的な入力、IME (CDP の imeSetComposition)、agent の提案と承認、template とキャッシュ、はみ出し。
// 使い方: pnpm build && node e2e/smoke.cjs [--shots]
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

const root = path.join(__dirname, '..', 'www');
const shotsDir = path.join(__dirname, '..', 'screenshots');
const types = { '.html': 'text/html', '.js': 'text/javascript', '.map': 'application/json' };
const server = http.createServer((req, res) => {
  const file = path.join(root, req.url === '/' ? 'index.html' : req.url.split('?')[0]);
  if (!file.startsWith(root) || !fs.existsSync(file)) return res.writeHead(404).end();
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
  const url = `http://127.0.0.1:${server.address().port}/`;
  const browser = await chromium.launch();
  const context = await browser.newContext({ viewport: { width: 1280, height: 720 }, deviceScaleFactor: shots ? 2 : 1 });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  await page.goto(url);
  await page.waitForFunction(() => window.shell);
  const cdp = await context.newCDPSession(page);

  // canvas 上の文字列の位置 (shell の display list から引く)
  const textPos = (str) =>
    page.evaluate((str) => {
      const c = document.getElementById('screen').getBoundingClientRect();
      const op = window.shell.frame(c.width, c.height).ops.find((o) => o[0] === 'text' && o[1].str === str);
      return op ? { x: c.left + op[1].x, y: c.top + op[1].y } : null;
    }, str);
  const tree = (app) => page.evaluate((app) => window.agent.getTree(app)[0].nodes, app);
  const nodeOf = async (app, id) => (await tree(app)).find((n) => n.id === id);
  const shot = async (name) => shots && (await page.screenshot({ path: path.join(shotsDir, `${name}.png`) }));
  if (shots) fs.mkdirSync(shotsDir, { recursive: true });

  await step('起動してエラーがない', async () => {
    assert.deepEqual(errors, []);
    assert.ok(await textPos('やること'));
    await shot('01-initial');
  });

  await step('人間のクリック -> hit test -> click action', async () => {
    const p = await textPos('牛乳を買う');
    await page.mouse.click(p.x - 14, p.y);
    assert.equal((await nodeOf('todo', 'i1')).checked, true);
  });

  await step('Space キーはフォーカス中のチェックボックスを click する', async () => {
    await page.keyboard.press('Space');
    assert.equal((await nodeOf('todo', 'i1')).checked, false);
  });

  await step('IME: 変換中は木に入らず、確定だけが値になる (CDP の実 composition)', async () => {
    const p = await textPos('新しいタスク');
    await page.mouse.click(p.x + 20, p.y);
    const ime = await page.evaluate(() => {
      const c = document.getElementById('screen').getBoundingClientRect();
      return window.shell.frame(c.width, c.height).ime.t;
    });
    assert.equal(ime, 'ime-enable');
    await cdp.send('Input.imeSetComposition', { text: 'たまご', selectionStart: 3, selectionEnd: 3 });
    assert.equal((await nodeOf('todo', 'input')).value, '');
    assert.ok(await textPos('たまご'), 'preedit が描かれている');
    await shot('02-preedit');
    await cdp.send('Input.insertText', { text: '卵' });
    assert.equal((await nodeOf('todo', 'input')).value, '卵');
    await page.keyboard.type('wo kau');
    assert.equal((await nodeOf('todo', 'input')).value, '卵wo kau');
    for (let i = 0; i < 6; i++) await page.keyboard.press('Backspace');
    await page.keyboard.insertText('を買う');
    assert.equal((await nodeOf('todo', 'input')).value, '卵を買う');
  });

  await step('Tab でフォーカス移動、Enter で追加ボタン', async () => {
    await page.keyboard.press('Tab');
    assert.equal(await page.evaluate(() => window.shell.tree('todo').focus), 'add');
    await page.keyboard.press('Enter');
    const items = (await tree('todo')).filter((n) => n.role === 'checkbox').map((n) => n.label);
    assert.deepEqual(items, ['牛乳を買う', '請求書を送る', 'PR をレビューする', '卵を買う']);
    assert.equal((await nodeOf('todo', 'input')).value, '');
  });

  await step('agent: 承認なしの直接操作は拒否、提案はプレビューされ、承認で実行', async () => {
    const denied = await page.evaluate(() => window.shell.dispatch({ app: 'todo', target: 'i3', action: 'click', by: 'agent' }).result);
    assert.equal(denied, 'denied');
    await page.click('#propose');
    assert.equal((await nodeOf('todo', 'i1')).checked, false, '承認前は副作用なし');
    await page.waitForSelector('#proposals button[data-op="approve"]');
    await shot('03-proposal-preview');
    await page.click('#proposals button[data-op="approve"]');
    assert.equal((await nodeOf('todo', 'i1')).checked, true);
    const labels = (await tree('todo')).filter((n) => n.role === 'checkbox').map((n) => n.label);
    assert.equal(labels.at(-1), '卵を買う');
    assert.equal(labels.length, 5);
  });

  await step('agent: sensitive は自動承認されず、アプリも agent の全削除を拒否する', async () => {
    await page.check('#auto');
    const r = await page.evaluate(() => window.agent.propose('全削除', [{ app: 'settings', target: 'reset', action: 'click' }]).proposal.status);
    assert.equal(r, 'pending');
    await page.click('#proposals button[data-op="approve"]');
    assert.equal((await nodeOf('settings', 'status')).label, 'エージェントからの全削除は拒否しました');
    const auto = await page.evaluate(() => window.agent.propose('コンパクト', [{ app: 'settings', target: 'compact', action: 'click' }]).proposal.status);
    assert.equal(auto, 'done');
  });

  await step('template: カードを適用し、pin すると再読み込み後も残る', async () => {
    await page.click('#sampleCard');
    await page.click('#applyTpl');
    assert.ok(await textPos('5 件のタスク'));
    await page.click('#pinTpl');
    await shot('04-card-template');
    await page.reload();
    await page.waitForFunction(() => window.shell);
    // Todo の項目はアプリの保存領域 (storage 能力) に残るので、再読み込み後も 5 件
    assert.ok(await textPos('5 件のタスク'), 'pin した template も Todo の項目も再読み込み後に残る');
  });

  await step('template: 操作できるノードを落とす template は既定に戻り、理由が出る', async () => {
    await page.click('#sampleBad');
    await page.click('#applyTpl');
    await page.waitForSelector('#wins .err');
    assert.match(await page.textContent('#wins'), /view にない/);
    assert.ok(await textPos('牛乳を買う'));
    await page.click('#clearTpl');
  });

  await step('タイルの境界を狭めるとはみ出しを報告する', async () => {
    const c = await page.evaluate(() => document.getElementById('screen').getBoundingClientRect().toJSON());
    const div = await page.evaluate(() => {
      const r = document.getElementById('screen').getBoundingClientRect();
      const t = window.shell.frame(r.width, r.height).windows[0].tile;
      return t.x + t.w + 4;
    });
    await page.mouse.move(c.left + div, c.top + c.height / 2);
    await page.mouse.down();
    await page.mouse.move(c.left + c.width * 0.16, c.top + c.height / 2, { steps: 5 });
    await page.mouse.up();
    assert.match(await page.textContent('#wins'), /はみ出し/);
    await shot('05-overflow');
  });

  await step('最後までページエラーなし', async () => assert.deepEqual(errors, []));
  await browser.close();
  server.close();
  console.log(failures ? `${failures} failed` : 'all passed');
  process.exit(failures ? 1 : 0);
})();
