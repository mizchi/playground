// 実ブラウザでの確認: アプリのパッケージ (app.json + WIT のコンポーネント) をパネルから入れ、ランチャーから開き、保存領域に残す。
// 使い方: pnpm build && node e2e/packages.cjs
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

const root = path.join(__dirname, '..', 'www');
const types = { '.html': 'text/html', '.js': 'text/javascript', '.map': 'application/json', '.wasm': 'application/wasm', '.json': 'application/json' };
const server = http.createServer((req, res) => {
  const p = req.url.split('?')[0];
  const file = path.join(root, p === '/' ? 'index.html' : p);
  if (!file.startsWith(root) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) return res.writeHead(404).end();
  res.writeHead(200, { 'content-type': types[path.extname(file)] ?? 'application/octet-stream' }).end(fs.readFileSync(file));
});
// 別の origin のパッケージの置き場 (CORS を許す)。パッケージの JS は動かさないので、ほかの origin からも入れられる
const other = http.createServer((req, res) => {
  const p = req.url.split('?')[0];
  const file = path.join(root, p);
  if (!file.startsWith(path.join(root, 'packages')) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) return res.writeHead(404, { 'access-control-allow-origin': '*' }).end();
  res.writeHead(200, { 'content-type': types[path.extname(file)] ?? 'application/octet-stream', 'access-control-allow-origin': '*' }).end(fs.readFileSync(file));
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
  await new Promise((r) => other.listen(0, '127.0.0.1', r));
  const otherUrl = `http://127.0.0.1:${other.address().port}/packages/todo-rs/app.json`;
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1500, height: 860 } });
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  await page.evaluate(() => localStorage.clear());
  await page.reload();
  await page.waitForFunction(() => window.platform);
  const settle = async () => {
    await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
    await page.waitForFunction(() => window.lastFrame && !window.lastFrame.animating);
  };
  const tree = (id) => page.evaluate((id) => window.shell.tree(id).get(id === 'todo-rs' ? 'list' : 'root'), id);

  await step('http(s) でない URL は断る', async () => {
    await page.fill('#pkgUrl', 'data:application/json,{}');
    await page.click('#pkgInstall');
    await page.waitForFunction(() => document.getElementById('pkgErr').textContent !== '読み込み中…');
    assert.match(await page.textContent('#pkgErr'), /http\(s\) の URL だけ/);
  });

  await step('ほかの origin のパッケージを、パネルから入れると、ランチャーに出て、開ける (glue はホストが .wasm から作る)', async () => {
    await page.fill('#pkgUrl', otherUrl);
    await page.click('#pkgInstall');
    await page.waitForFunction(() => document.getElementById('pkgErr').textContent === '');
    await page.waitForSelector('#installed [data-launch="todo-rs"]');
    // フォーカスはパネルのボタンにある。F6 でデスクトップへ戻ってから Alt+L
    await page.keyboard.press('F6');
    await page.keyboard.press('Alt+KeyL');
    await settle();
    await page.evaluate(() => window.shell.activate('launcher', 'open-todo-rs'));
    await settle();
    assert.ok(await page.evaluate(() => window.shell.windows().some((w) => w.id === 'todo-rs')));
    assert.equal(await page.evaluate(() => window.shell.tree('todo-rs').get('root').label), 'Todo (wasm)');
  });

  await step('同じ id のパッケージは、別の URL からでも二重には入らない', async () => {
    await page.fill('#pkgUrl', 'packages/todo-rs/app.json');
    await page.click('#pkgInstall');
    await page.waitForFunction(() => document.getElementById('pkgErr').textContent !== '読み込み中…');
    assert.match(await page.textContent('#pkgErr'), /すでに入っている/);
    await page.keyboard.press('F6');
  });

  await step('コンポーネントのアプリで項目を足すと、能力 storage (import) で保存される', async () => {
    await page.evaluate(() => window.shell.activate('todo-rs', 'input'));
    // フォーカスはコンポーネント (Worker) が返す木で決まる。返ってから打つ
    await page.waitForFunction(() => window.shell.tree('todo-rs').focus === 'input');
    await page.keyboard.insertText('WIT から保存');
    await page.waitForFunction(() => window.shell.tree('todo-rs').get('input').value === 'WIT から保存');
    await page.evaluate(() => window.shell.activate('todo-rs', 'add'));
    // コンポーネントは Worker で動くので、保存は少し後に届く
    await page.waitForFunction(() => /WIT から保存/.test(localStorage.getItem('ai-native-desktop.kv.app.todo-rs.items') ?? ''));
  });

  await step('再読み込みしても、パッケージも窓も項目も残る', async () => {
    await page.reload();
    await page.waitForFunction(() => window.platform && window.shell.windows().some((w) => w.id === 'todo-rs') && window.shell.tree('todo-rs').get('list'));
    const labels = await page.evaluate(() => {
      const t = window.shell.tree('todo-rs');
      return t.get('list').children.map((c) => t.get(c).label);
    });
    assert.ok(labels.includes('WIT から保存'), labels.join());
  });

  await step('保存したパッケージの一覧が壊れていても、同じ id のパッケージが二重にあっても、起動する', async () => {
    for (const v of ['null', '{}', JSON.stringify(['packages/todo-rs/app.json', 'packages/todo-rs/app.json?again'])]) {
      await page.evaluate((v) => localStorage.setItem('ai-native-desktop.kv.platform.packages', v), v);
      await page.reload();
      await page.waitForFunction(() => window.platform, null, { timeout: 10000 });
    }
    assert.equal(await page.evaluate(() => window.platform.registry.list().filter((m) => m.id === 'todo-rs').length), 1);
  });

  await step('前回の起動が終わらなかったパッケージは隔離して読み込まず、デスクトップは起動する。?safe では 1 つも読み込まない', async () => {
    const url = 'packages/todo-rs/app.json';
    // 読み込みの途中で固まった (見張りが外れていない) 状態を作る
    await page.evaluate((url) => {
      localStorage.setItem('ai-native-desktop.kv.platform.packages', JSON.stringify([url]));
      localStorage.setItem('ai-native-desktop.kv.platform.packages.loading', JSON.stringify([url]));
    }, url);
    await page.reload();
    await page.waitForFunction(() => window.platform);
    assert.match(await page.textContent('#pkgErr'), /前回の起動が終わらなかったので、パッケージ packages\/todo-rs\/app.json を隔離した/);
    assert.ok(!(await page.evaluate(() => window.platform.registry.manifest('todo-rs'))));
    // 隔離は残る (もう一度起動しても読み込まない)
    await page.reload();
    await page.waitForFunction(() => window.platform);
    assert.ok(!(await page.evaluate(() => window.platform.registry.manifest('todo-rs'))));
    // 人が入れ直すと隔離が解け、次の起動から読み込む
    await page.fill('#pkgUrl', url);
    await page.click('#pkgInstall');
    await page.waitForFunction(() => document.getElementById('pkgErr').textContent === '');
    await page.reload();
    await page.waitForFunction(() => window.platform && window.platform.registry.manifest('todo-rs'));
    // ?safe
    await page.goto(page.url().split('?')[0] + '?safe');
    await page.waitForFunction(() => window.platform);
    assert.ok(!(await page.evaluate(() => window.platform.registry.manifest('todo-rs'))));
    assert.match(await page.textContent('#pkgErr'), /安全モード/);
  });

  await step('コンポーネントは Worker で動く: 止まらないコンポーネントは期限で止められ、デスクトップは固まらない。再起動できる', async () => {
    await page.goto(page.url().split('?')[0] + '?session=off');
    await page.waitForFunction(() => window.platform);
    await page.fill('#pkgUrl', 'packages/spin-rs/app.json');
    await page.click('#pkgInstall');
    await page.waitForFunction(() => document.getElementById('pkgErr').textContent === '');
    await page.click('#installed [data-launch="spin-rs"]');
    await page.waitForFunction(() => window.shell.tree('spin-rs').get('count'));
    const label = () => page.evaluate(() => window.shell.tree('spin-rs').get('root').label);
    await page.evaluate(() => window.shell.activate('spin-rs', 'count'));
    await page.waitForFunction(() => window.shell.tree('spin-rs').get('count').label === '数える (1)');
    // 固まる: on-action から戻らない。メインスレッドは動き続ける (evaluate がすぐ返り、ほかのアプリも操作できる)
    await page.evaluate(() => window.shell.activate('spin-rs', 'spin'));
    const t0 = Date.now();
    await page.evaluate(() => window.shell.dispatch({ app: 'todo', target: 'i1', action: 'click', by: 'human' }));
    assert.ok(Date.now() - t0 < 500, 'メインスレッドは固まらない');
    assert.equal(await page.evaluate(() => window.shell.tree('todo').get('i1').checked), true);
    // 期限 (2 秒) で止められ、窓は「応答しない」になる
    await page.waitForFunction(() => /応答しない/.test(window.shell.tree('spin-rs').get('root').label), null, { timeout: 5000 });
    assert.match(await page.evaluate(() => window.shell.tree('spin-rs').get('x-stopped').label), /2 秒で返らなかった/);
    // 再起動すると、新しい Worker で動く
    await page.evaluate(() => window.shell.activate('spin-rs', 'x-restart'));
    await page.waitForFunction(() => window.shell.tree('spin-rs').get('count')?.label === '数える (0)');
    assert.equal(await label(), 'Spin (wasm)');
    assert.ok(!(await page.evaluate(() => window.shell.tree('spin-rs').get('x-restart'))), '止めたときの表示は消える');
  });

  await step('ページエラーなし', async () => assert.deepEqual(errors, []));
  await browser.close();
  server.close();
  other.close();
  console.log(failures ? `${failures} failed` : 'all passed');
  process.exit(failures ? 1 : 0);
})();
