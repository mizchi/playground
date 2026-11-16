// 全曲をオフラインレンダリングして、楽譜の検算エラーと音量を確認する。
// 実行: node verify.cjs  (playwright と chromium が必要)
const { chromium } = require('playwright');
const path = require('path');

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
  const logs = [];
  page.on('console', m => m.type() === 'error' && logs.push(m.text()));
  page.on('pageerror', e => logs.push(String(e)));
  await page.goto('file://' + path.join(__dirname, 'index.html'));
  const res = await page.evaluate(() => window.__verify());
  console.table(res.songs);
  if (process.argv[2]) await page.screenshot({ path: process.argv[2], fullPage: true });
  await browser.close();
  const bad = res.songs.filter(s => !(s.rms > 0.005) || s.peak > 1);
  if (res.errors.length || logs.length || bad.length) {
    console.error({ errors: res.errors, logs, bad });
    process.exit(1);
  }
  console.log('ok');
})();
