// 実ブラウザでの確認: プラットフォームのアプリの例としての Terminal (libghostty-vt の wasm)。
// ランチャー (Alt+L) から開く → キーボードと IME で入力 → 画面の文字と色 → 能力の許可 → agent の提案 → セッションの復元 → ホストの擬似端末。
// 使い方: pnpm build && node e2e/terminal.cjs [--shots]
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

const root = path.join(__dirname, '..', 'www');
const shotsDir = path.join(__dirname, '..', 'screenshots');
const types = { '.html': 'text/html', '.js': 'text/javascript', '.map': 'application/json', '.wasm': 'application/wasm' };
const server = http.createServer((req, res) => {
  const p = req.url.split('?')[0];
  const file = path.join(root, p === '/' ? 'index.html' : p);
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
  const base = `http://127.0.0.1:${server.address().port}/`;
  const browser = await chromium.launch();
  const context = await browser.newContext({ viewport: { width: 1500, height: 860 }, deviceScaleFactor: shots ? 2 : 1 });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  await page.goto(base);
  await page.evaluate(() => localStorage.clear());
  await page.reload();
  await page.waitForFunction(() => window.platform);
  if (shots) fs.mkdirSync(shotsDir, { recursive: true });
  const shot = async (name) => shots && (await page.screenshot({ path: path.join(shotsDir, `${name}.png`) }));

  const textPos = (str) =>
    page.evaluate((str) => {
      const c = document.getElementById('screen').getBoundingClientRect();
      const op = window.shell.frame(c.width, c.height).ops.find((o) => o[0] === 'text' && o[1].str === str);
      return op ? { x: c.left + op[1].x + 4, y: c.top + op[1].y } : null;
    }, str);
  const clickText = async (str) => {
    const p = await textPos(str);
    assert.ok(p, `「${str}」が描かれていない`);
    await page.mouse.click(p.x, p.y);
  };
  const ids = () => page.evaluate(() => window.shell.windows().map((w) => w.id));
  const screenText = (id = 'terminal') => page.evaluate((id) => window.shell.tree(id).get('screen')?.value ?? '', id);
  const waitScreen = (re, id = 'terminal') =>
    page.waitForFunction(([src, id]) => new RegExp(src).test(window.shell.tree(id).get('screen')?.value ?? ''), [re.source, id], { timeout: 5000 });
  // 2 フレーム待ってから (アニメーションが始まる前の古いフレームを見ないように)、止まるのを待つ
  const settle = async () => {
    await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
    await page.waitForFunction(() => window.lastFrame && !window.lastFrame.animating);
  };
  const typeLine = async (s) => {
    await page.keyboard.insertText(s);
    await page.keyboard.press('Enter');
  };

  await step('Alt+L でランチャーを開き、そこから Terminal を開く', async () => {
    await page.keyboard.press('Alt+KeyL');
    await settle();
    assert.ok((await ids()).includes('launcher'));
    await clickText('Terminal を開く');
    await settle();
    assert.ok((await ids()).includes('terminal'));
    await waitScreen(/libghostty-vt/);
  });

  await step('キーボードで打ったコマンドを実行し、画面 (意味ツリーの log) に出る', async () => {
    await typeLine('echo hello from vt');
    await waitScreen(/\$ echo hello from vt\nhello from vt\n/);
    await shot('terminal-01-echo');
  });

  await step('IME の確定文字もコマンド行に入る (変換中は入らない)', async () => {
    const cdp = await context.newCDPSession(page);
    await page.keyboard.insertText('echo ');
    await cdp.send('Input.imeSetComposition', { text: 'にほんご', selectionStart: 4, selectionEnd: 4 });
    assert.doesNotMatch((await screenText()).split('\n').at(-1), /にほんご/);
    await cdp.send('Input.insertText', { text: '日本語' });
    await page.keyboard.press('Enter');
    await waitScreen(/\n日本語\n/);
    const wide = await page.evaluate(() => window.shell.tree('terminal').get('screen').screen.lines.flat().filter((r) => r.wide).map((r) => r.text).join(''));
    assert.match(wide, /日本語/);
  });

  await step('colors: libghostty-vt が解釈した色が、その色のまま Canvas に描かれる', async () => {
    await typeLine('colors');
    await waitScreen(/太字 薄い/);
    await settle();
    // 16 色の見本 (行の左端にあり、狭いタイルでも切り取られない) の背景の矩形を 1 つ選び、その中心の画素を読む
    const probe = await page.evaluate(() => {
      const c = document.getElementById('screen');
      const r = c.getBoundingClientRect();
      const ops = window.shell.frame(r.width, r.height).ops.filter((o) => o[0] === 'rect' && o[1].rgb && o[1].w < 20);
      const op = ops[3][1];
      const dpr = c.width / r.width;
      const px = c.getContext('2d').getImageData(Math.round((op.x + op.w / 2) * dpr), Math.round((op.y + op.h / 2) * dpr), 1, 1).data;
      return { want: op.rgb, got: [px[0], px[1], px[2]], n: ops.length };
    });
    assert.ok(probe.n > 40, `色の矩形 ${probe.n}`);
    const want = [1, 3, 5].map((i) => parseInt(probe.want.slice(i, i + 2), 16));
    assert.ok(want.every((v, i) => Math.abs(v - probe.got[i]) <= 2), `${probe.want} != rgb(${probe.got})`);
    await shot('terminal-02-colors');
  });

  await step('ファイルはアプリの保存領域に書かれる (storage 能力)', async () => {
    await typeLine('write todo.txt 卵を買う');
    await typeLine('cat todo.txt');
    await waitScreen(/\$ cat todo\.txt\n卵を買う\n/);
    const raw = await page.evaluate(() => localStorage.getItem('ai-native-desktop.kv.app.terminal.fs'));
    assert.match(raw, /卵を買う/);
  });

  await step('paste は許可の要求をホストの UI に出し、人が許可すると読める', async () => {
    await typeLine('copy クリップボードの中身');
    await typeLine('paste');
    await waitScreen(/許可を待っている/);
    await page.waitForSelector('#prompts [data-allow="1"]');
    assert.match(await page.textContent('#prompts'), /Terminal が クリップボードの読み取り の許可を求めている/);
    await shot('terminal-03-grant');
    await page.click('#prompts [data-allow="1"]');
    // 画面を押すとコマンド行にフォーカスが戻る (log ノードの focus をアプリが入力欄に回す)
    const r = await page.evaluate(() => window.shell.accessibility().rects.get('terminal/screen'));
    const c = await page.locator('#screen').boundingBox();
    await page.mouse.click(c.x + r.x + 40, c.y + r.y + 20);
    await typeLine('paste');
    await waitScreen(/\$ paste\nクリップボードの中身\n/);
  });

  await step('cal はカレンダーの予定を読む。許可の要求に、提供するアプリと中身が出る', async () => {
    const now = new Date();
    const key = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-07`;
    // カレンダーの保存領域に予定を置く (カレンダーの窓は開いていない)
    await page.evaluate((key) => window.platform.services.servicesFor('calendar', 'calendar').storage.set('events', JSON.stringify({ [key]: ['定例会議'] })), key);
    await typeLine('cal');
    await waitScreen(/カレンダーの許可を待っている/);
    await page.waitForSelector('#prompts [data-allow="1"]');
    assert.match(await page.textContent('#prompts'), /Terminal が Calendar の「月の予定 \(日付と題名。読み取りだけ\)」の読み取り の許可を求めている/);
    await page.click('#prompts [data-allow="1"]');
    await page.evaluate(() => window.shell.activate('terminal', 'input'));
    await typeLine('cal');
    await waitScreen(/\n\d\d\/07 定例会議/);
  });

  await step('agent のコマンドは承認するまで実行されず、承認後は agent として記録される', async () => {
    const r = await page.evaluate(() =>
      window.agent.propose('whoami を実行する', [
        { app: 'terminal', target: 'input', action: 'setValue', data: 'whoami' },
        { app: 'terminal', target: 'input', action: 'click' },
      ]),
    );
    assert.ok(r.ok);
    assert.equal(r.proposal.status, 'pending');
    assert.doesNotMatch(await screenText(), /agent \(/);
    await page.click('#proposals button[data-op="approve"]');
    await waitScreen(/agent \(人が承認した提案で入力された\)/);
    const tree = await page.evaluate(() => window.agent.getTree('terminal')[0].nodes.find((n) => n.id === 'screen'));
    assert.equal(tree.screen, undefined, 'agent には描画用の格子を渡さない');
  });

  await step('スクリーンリーダー向けのミラーに role=log として画面の文字が出る', async () => {
    await page.waitForFunction(() => /whoami/.test(document.querySelector('#a11y [role=log]')?.textContent ?? ''));
    assert.equal(await page.getAttribute('#a11y [role=log]', 'aria-label'), '画面');
  });

  await step('2 枚目の Terminal は別の端末。閉じると id が戻る', async () => {
    await page.click('#installed [data-launch="terminal"]');
    await settle();
    await waitScreen(/libghostty-vt/, 'terminal-2');
    assert.doesNotMatch(await screenText('terminal-2'), /hello from vt/);
    await page.click('#openWins [data-close-win="terminal-2"]');
    await settle();
    assert.ok(!(await ids()).includes('terminal-2'));
  });

  await step('再読み込みすると開いていた窓 (Terminal・ランチャー) と配置が戻る', async () => {
    const before = await page.evaluate(() => ({ ids: window.shell.windows().map((w) => w.id), layout: window.shell.getLayout() }));
    await page.reload();
    await page.waitForFunction(() => window.platform);
    await waitScreen(/libghostty-vt/);
    const after = await page.evaluate(() => ({ ids: window.shell.windows().map((w) => w.id), layout: window.shell.getLayout() }));
    assert.deepEqual(after, before);
    await page.keyboard.press('Tab'); // フォーカスを戻す
  });

  // ---- ホストの擬似端末 (pty/server.ts) ----
  const token = 'e2e' + Math.random().toString(16).slice(2);
  const ptyPort = 18140 + Math.floor(Math.random() * 500);
  const pty = spawn(process.execPath, [path.join(__dirname, '..', 'pty', 'server.ts')], { env: { ...process.env, PTY_TOKEN: token, PTY_PORT: String(ptyPort) }, stdio: ['ignore', 'pipe', 'ignore'] });
  await new Promise((ok) => pty.stdout.on('data', (d) => /pty: ws/.test(String(d)) && ok()));
  const ptyUrl = `ws://127.0.0.1:${ptyPort}/?token=${token}`;

  await step('擬似端末の橋は token の無い接続を断る', async () => {
    const r = await page.evaluate(
      (u) =>
        new Promise((ok) => {
          const ws = new WebSocket(u);
          ws.onopen = () => ok('open');
          ws.onerror = () => ok('refused');
        }),
      ptyUrl.replace(/token=.*/, 'token=wrong'),
    );
    assert.equal(r, 'refused');
  });

  await step('ホストのシェル: terminal.pty を許可すると本物の bash が libghostty-vt の上で動く', async () => {
    await page.goto(`${base}?session=off&pty=${encodeURIComponent(ptyUrl)}`);
    await page.waitForFunction(() => window.platform);
    await page.evaluate(() => window.platform.launch('terminal'));
    await waitScreen(/libghostty-vt/);
    await page.keyboard.press('Alt+Enter'); // 全体に広げる
    await settle();
    await clickText('ホストのシェルにつなぐ');
    await page.waitForSelector('#prompts [data-allow="1"]');
    assert.match(await page.textContent('#prompts'), /ホストのシェル/);
    await page.click('#prompts [data-allow="1"]');
    await waitScreen(/[#$]$/); // bash のプロンプト
    await page.evaluate(() => window.shell.activate('terminal', 'input'));
    await typeLine('echo $((6*7)); printf "\\033[31mRED\\033[0m\\n"; stty size');
    // stty size は、窓に合わせた端末の大きさ (行 桁) を返す
    const [cols, rows] = await page.evaluate(() => { const s = window.shell.tree('terminal').get('screen').screen; return [s.cols, s.rows]; });
    assert.ok(cols > 72, `全体に広げた窓の桁 ${cols}`);
    await waitScreen(new RegExp(`\\n42\\nRED\\n${rows} ${cols}\\n`));
    const red = await page.evaluate(() => window.shell.tree('terminal').get('screen').screen.lines.flat().find((r) => r.text === 'RED'));
    assert.ok(red?.fg && red.fg !== '#e6e8ef', `RED の色 ${red?.fg}`);
    await shot('terminal-04-host-shell');
    // 窓を戻すと狭くなり、動いているシェルにも新しい大きさが届く (SIGWINCH)
    await page.keyboard.press('Escape');
    await settle();
    await page.waitForFunction((c) => window.shell.tree('terminal').get('screen').screen.cols < c, cols);
    const [c2, r2] = await page.evaluate(() => { const s = window.shell.tree('terminal').get('screen').screen; return [s.cols, s.rows]; });
    await page.evaluate(() => window.shell.activate('terminal', 'input'));
    await typeLine('stty size');
    await waitScreen(new RegExp(`\\n${r2} ${c2}\\n`));
    await typeLine('exit');
    await waitScreen(/ホストのシェルが終わった/);
  });
  pty.kill();

  // token の違う接続をわざと断らせたときのコンソールのエラーは除く
  await step('最後までページエラーなし', async () => assert.deepEqual(errors.filter((e) => !e.includes('token=wrong')), []));
  await browser.close();
  server.close();
  console.log(failures ? `${failures} failed` : 'all passed');
  process.exit(failures ? 1 : 0);
})();
