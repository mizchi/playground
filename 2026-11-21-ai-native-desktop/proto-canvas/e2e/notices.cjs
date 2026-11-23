// 実ブラウザでの確認: 通知を Canvas の上に。アプリの通知が窓より上の層に出て、押すとその窓へ移り、時間が経つと消える (パネルには残る)。
// Canvas の通知は shell が描くので、アプリからは描けない。
// 使い方: pnpm build && node e2e/notices.cjs [--shots]
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
  const textDrawn = (str) => page.evaluate((str) => window.lastFrame && window.shell.frame(document.getElementById('screen').getBoundingClientRect().width, document.getElementById('screen').getBoundingClientRect().height, performance.now()).ops.some((o) => o[0] === 'text' && o[1].str === str), str);

  const size = () => page.evaluate(() => document.getElementById('screen').getBoundingClientRect());
  /** 今の画面の通知の枠 (canvas の座標) */
  const toasts = () =>
    page.evaluate(() => {
      const r = document.getElementById('screen').getBoundingClientRect();
      return window.shell.frame(r.width, r.height, performance.now()).ops.filter((o) => o[0] === 'rect' && o[1].stroke === 'accent' && o[1].radius === 10 && o[1].w === 320).map((o) => o[1]);
    });
  const human = (target) => page.evaluate((target) => window.shell.dispatch({ app: 'todo', target, action: 'click', by: 'human' }), target);

  await step('Todo をすべて完了にすると通知の許可を求め、人が許可すると Canvas の上に通知が出る', async () => {
    await human('i1');
    await human('i3');
    await page.waitForSelector('#prompts [data-allow="1"]');
    await page.click('#prompts [data-allow="1"]');
    // 許可のあとで、もう一度すべて完了にする
    await human('i1');
    await human('i1');
    await settle();
    assert.ok(await textDrawn('Todo からの通知'), '出どころの名前は manifest から');
    assert.ok(await textDrawn('すべて完了した'));
    assert.match(await page.textContent('#notices'), /すべて完了した/, 'パネルにも出る');
    const [t] = await toasts();
    const r = await size();
    assert.ok(t && t.x + t.w <= r.width && t.y + t.h <= r.height, '画面の内側 (右下)');
    if (shots) await page.screenshot({ path: path.join(shotsDir, 'notice.png') });
  });

  await step('通知の本文を押すと、出した窓 (Todo) へフォーカスして閉じる。下の窓には届かない', async () => {
    // Notes へフォーカスを移しておく
    await page.evaluate(() => window.shell.activate('notes', window.shell.tree('notes').root));
    const auditBefore = await page.evaluate(() => window.shell.audit.length);
    const [t] = await toasts();
    const r = await size();
    await page.mouse.click(r.x + t.x + 40, r.y + t.y + 40);
    await settle();
    assert.ok(!(await textDrawn('すべて完了した')));
    assert.equal(await page.evaluate(() => window.platform.services.notifications.length), 0);
    assert.equal(await page.evaluate(() => window.shell.accessibility().windows.find((w) => w.focused)?.app), 'todo');
    assert.equal(await page.evaluate(() => window.shell.audit.length), auditBefore, '通知の下の窓への操作は起きない');
  });

  await step('押さなければ 6 秒で画面から消える (パネルの一覧には残る)。消えるまでフレームは回し続けない', async () => {
    await human('i1');
    await human('i1');
    await settle();
    assert.ok(await textDrawn('すべて完了した'));
    // 出ている間は止まっている (wakeAt で起こす)
    assert.equal(await page.evaluate(() => window.lastFrame.animating), false);
    const opsBefore = await page.evaluate(() => window.lastFrame.ops);
    // 消える時刻 (6 秒) のあと、render が描き直すのを待つ (shell.frame を自分では呼ばない)
    await page.waitForFunction((before) => window.lastFrame.ops < before, opsBefore, { timeout: 10000 });
    // 何も操作しなくても、消える時刻に描き直されている
    assert.ok((await page.evaluate(() => window.lastFrame.ops)) < opsBefore, '描き直されて、通知の分の命令が減った');
    await settle();
    assert.equal((await toasts()).length, 0);
    assert.match(await page.textContent('#notices'), /すべて完了した/);
  });

  await step('Escape で閉じる', async () => {
    await human('i1');
    await human('i1');
    await settle();
    assert.equal((await toasts()).length, 1);
    await page.keyboard.press('Escape');
    await settle();
    assert.equal((await toasts()).length, 0);
  });

  await step('ページエラーなし', async () => assert.deepEqual(errors, []));
  await browser.close();
  server.close();
  console.log(failures ? `${failures} failed` : 'all passed');
  process.exit(failures ? 1 : 0);
})();
