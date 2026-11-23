// 実ブラウザでの確認: 人と agent のワークスペース。Alt+2 で agent のワークスペースを見る → agent が開いた窓と作業場の差分 →
// 持ち込みの提案をパネルで承認 → Alt+1 で人のワークスペースに反映されている。
// 使い方: pnpm build && node e2e/workspace.cjs [--shots]
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

const root = path.join(__dirname, '..', 'www');
const shotsDir = path.join(__dirname, '..', 'screenshots');
const types = { '.html': 'text/html', '.js': 'text/javascript', '.map': 'application/json', '.wasm': 'application/wasm', '.json': 'application/json' };
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
  const page = await browser.newPage({ viewport: { width: 1500, height: 860 }, deviceScaleFactor: shots ? 2 : 1 });
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
  const textDrawn = (str) => page.evaluate((str) => window.lastFrame && window.shell.frame(document.getElementById('screen').getBoundingClientRect().width, document.getElementById('screen').getBoundingClientRect().height).ops.some((o) => o[0] === 'text' && o[1].str === str), str);

  await step('agent は自分のワークスペースで、人の画面を変えずに窓を開いて編集する', async () => {
    await page.evaluate(() => {
      window.agent.workspace.launch('todo');
      const id = window.agent.workspace.windows().at(-1).id;
      window.agent.workspace.act(id, 'input', 'setValue', 'agent が足した項目');
      window.agent.workspace.act(id, 'add', 'click');
    });
    // 人の画面 (今見ているワークスペース) は変わらない
    assert.ok(!(await textDrawn('agent が足した項目')));
    assert.ok(!(await textDrawn('agent のワークスペース')));
  });

  await step('Alt+2 で agent のワークスペースを見る: 札、agent の窓、作業場の差分', async () => {
    await page.keyboard.press('Alt+Digit2');
    await settle();
    assert.ok(await textDrawn('agent のワークスペース'), '右下の札');
    assert.ok(await textDrawn('agent が足した項目'));
    assert.ok(await textDrawn('agent の作業場'));
    assert.equal(await page.getAttribute('#ws2', 'aria-pressed'), 'true');
    if (shots) await page.screenshot({ path: path.join(shotsDir, 'workspace-agent.png') });
  });

  await step('作業場で持ち込みを提案すると、パネルに差分が出る。人が承認すると人のワークスペースに書かれる', async () => {
    await page.evaluate(() => window.agent.workspace.act('agent-desk', 'merge', 'click'));
    await page.waitForSelector('#merges [data-ok="1"]');
    assert.match(await page.textContent('#merges'), /todo \/ items/);
    if (shots) await page.screenshot({ path: path.join(shotsDir, 'workspace-merge.png') });
    await page.click('#merges [data-ok="1"]');
    // パネルのボタンは、押したあとフォーカスをデスクトップへ戻す
    await page.keyboard.press('Alt+Digit1');
    await settle();
    assert.equal(await page.getAttribute('#ws1', 'aria-pressed'), 'true');
    assert.ok(await textDrawn('agent が足した項目'), '人のワークスペースの Todo が読み直した');
  });

  await step('ページエラーなし', async () => assert.deepEqual(errors, []));
  await browser.close();
  server.close();
  console.log(failures ? `${failures} failed` : 'all passed');
  process.exit(failures ? 1 : 0);
})();
