// 窓のドラッグ&ドロップによる分割の通し確認。--video で動画 (webm と gif) を screenshots/ に保存する。
// 使い方: pnpm build && node e2e/dnd.cjs [--video]
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const { chromium } = require('playwright');

const root = path.join(__dirname, '..', 'www');
const shotsDir = path.join(__dirname, '..', 'screenshots');
const server = http.createServer((req, res) => {
  const file = path.join(root, req.url === '/' ? 'index.html' : req.url.split('?')[0]);
  if (!file.startsWith(root) || !fs.existsSync(file)) return res.writeHead(404).end();
  res.writeHead(200, { 'content-type': file.endsWith('.js') ? 'text/javascript' : 'text/html' }).end(fs.readFileSync(file));
});
const video = process.argv.includes('--video');
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
  fs.mkdirSync(shotsDir, { recursive: true });
  const browser = await chromium.launch();
  const context = await browser.newContext({
    viewport: { width: 1680, height: 900 },
    ...(video ? { recordVideo: { dir: shotsDir, size: { width: 1680, height: 900 } } } : {}),
  });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  await page.evaluate(() => localStorage.clear());
  await page.reload();
  await page.waitForFunction(() => window.lastFrame);

  const canvasBox = () => page.evaluate(() => document.getElementById('screen').getBoundingClientRect().toJSON());
  const tile = async (app) => {
    const c = await canvasBox();
    const t = await page.evaluate((app) => window.lastFrame.windows.find((w) => w.app === app).tile, app);
    return { x: c.left + t.x, y: c.top + t.y, w: t.w, h: t.h };
  };
  // 手を離した直後は次のフレームがまだ描かれていないので、少し待ってから「止まった」を確かめる
  const settle = async () => {
    await page.waitForTimeout(100);
    await page.waitForFunction(() => !window.lastFrame.animating && !window.shell.windowDrag(), null, { timeout: 3000 });
  };
  const leaves = () => page.evaluate(() => {
    const walk = (n) => (n.kind === 'leaf' ? n.app : { [n.dir]: [walk(n.a), walk(n.b)] });
    return walk(window.shell.tiles);
  });
  /** タイトルバーを掴んで (tx, ty) まで、人の手のようにゆっくり運ぶ */
  const drag = async (app, tx, ty, { release = true, shot } = {}) => {
    const t = await tile(app);
    await page.mouse.move(t.x + 70, t.y + 14);
    await page.mouse.down();
    await page.mouse.move(t.x + 80, t.y + 20, { steps: 3 });
    await page.mouse.move(tx, ty, { steps: 30 });
    await page.waitForTimeout(350); // プレビューが収まるのを待つ
    if (shot) await page.screenshot({ path: path.join(shotsDir, shot) });
    const g = await page.evaluate(() => window.shell.windowDrag());
    if (release) await page.mouse.up();
    return g;
  };

  await step('初期配置: 左に Todo、右に Settings / Notes', async () => {
    assert.deepEqual(await leaves(), { row: ['todo', { column: ['settings', 'notes'] }] });
  });

  await step('Todo を Notes の右端へ: 青いプレビュー → 離すと Notes を左右に分割', async () => {
    const n = await tile('notes');
    const g = await drag('todo', n.x + n.w - 50, n.y + n.h / 2, { shot: 'dnd-1-preview-right.png' });
    assert.deepEqual(g.drop, { kind: 'split', target: 'notes', edge: 'right' });
    assert.equal(g.valid, true);
    await settle();
    assert.deepEqual(await leaves(), { column: ['settings', { row: ['notes', 'todo'] }] });
    await page.screenshot({ path: path.join(shotsDir, 'dnd-2-after-right.png') });
  });

  await step('Settings を外周の左の帯へ: 全体の左に置く', async () => {
    const c = await canvasBox();
    const g = await drag('settings', c.left + 12, c.top + c.height / 2, { shot: 'dnd-3-preview-root-left.png' });
    assert.deepEqual(g.drop, { kind: 'split', target: null, edge: 'left' });
    assert.equal(g.valid, true, g.reason);
    await settle();
    assert.deepEqual(await leaves(), { row: ['settings', { row: ['notes', 'todo'] }] });
  });

  await step('Notes を Todo の中央へ: 入れ替え', async () => {
    const t = await tile('todo');
    const g = await drag('notes', t.x + t.w / 2, t.y + t.h / 2);
    assert.deepEqual(g.drop, { kind: 'swap', target: 'todo' });
    await settle();
    assert.deepEqual(await leaves(), { row: ['settings', { row: ['todo', 'notes'] }] });
  });

  await step('Todo を Settings の下端へ: 上下に分割', async () => {
    const s = await tile('settings');
    const g = await drag('todo', s.x + s.w / 2, s.y + s.h - 50);
    assert.deepEqual(g.drop, { kind: 'split', target: 'settings', edge: 'bottom' });
    await settle();
    assert.deepEqual(await leaves(), { row: [{ column: ['settings', 'todo'] }, 'notes'] });
  });

  await step('最小幅を下回る分割は赤いプレビューになり、離しても変わらない', async () => {
    await page.setViewportSize({ width: 1000, height: 900 }); // canvas は 620px
    await page.waitForTimeout(200);
    const before = await leaves();
    const s = await tile('notes');
    const g = await drag('todo', s.x + s.w - 40, s.y + s.h / 2, { shot: 'dnd-4-invalid.png' });
    assert.equal(g.valid, false);
    assert.match(g.reason, /最小幅/);
    assert.equal(await page.evaluate(() => window.lastFrame.cursor), 'not-allowed');
    await settle();
    assert.deepEqual(await leaves(), before);
    await page.setViewportSize({ width: 1680, height: 900 });
  });

  await step('ページエラーなし', async () => assert.deepEqual(errors, []));
  const v = page.video();
  await context.close();
  await browser.close();
  server.close();
  if (video && v) {
    const webm = path.join(shotsDir, 'dnd.webm');
    fs.renameSync(await v.path(), webm);
    const gif = path.join(shotsDir, 'dnd.gif');
    execFileSync('ffmpeg', ['-y', '-loglevel', 'error', '-i', webm, '-vf', 'fps=20,scale=900:-1:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4', gif]);
    console.log(`video: ${webm}\ngif:   ${gif}`);
  }
  console.log(failures ? `${failures} failed` : 'all passed');
  process.exit(failures ? 1 : 0);
})();
