// veriss と実ブラウザ (Chromium) と layout.ts の 3 者照合。
// 試作の各窓を lowering し、(1) veriss の判定 (2) Chromium で 1px 刻みに配置した実測 (3) layout.ts の最小幅 を比べる。
// 使い方: pnpm build && node e2e/veriss-diff.cjs   (Node 24 以上)
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('playwright');

const root = path.join(__dirname, '..', 'www');
const server = http.createServer((req, res) => {
  const file = path.join(root, req.url === '/' ? 'index.html' : req.url.split('?')[0]);
  if (!file.startsWith(root) || !fs.existsSync(file)) return res.writeHead(404).end();
  res.writeHead(200, { 'content-type': file.endsWith('.js') ? 'text/javascript' : 'text/html' }).end(fs.readFileSync(file));
});

const DOMAIN = { min: 160, max: 700 };
const HEIGHT = { min: 300, max: 1200 };
const CARD = {
  type: 'box', axis: 'vertical', gap: 2, padding: 10, background: 'bg', border: 'border', radius: 8,
  children: [
    { type: 'text', text: { bind: 'count', format: '{} 件のタスク' }, size: 12, weight: 700, color: 'muted' },
    { type: 'each', item: { type: 'box', axis: 'horizontal', padding: 4, children: [{ type: 'self' }] } },
  ],
};

/** Chromium で幅ごとに「はみ出し (子が親の外 / viewport の外)」があるかを測る */
async function browserBad(page, low) {
  await page.setContent(`<!doctype html><html><head><style>${low.css}</style></head><body>${low.html}</body></html>`);
  const bad = [];
  for (let w = DOMAIN.min; w <= DOMAIN.max; w++) {
    await page.setViewportSize({ width: w, height: 750 });
    const r = await page.evaluate((w) => {
      const out = [];
      for (const el of document.querySelectorAll('main *')) {
        const a = el.getBoundingClientRect();
        const pe = el.parentElement;
        const p = pe.getBoundingClientRect();
        const cs = getComputedStyle(pe);
        // veriss と同じく、親の content box (padding の内側) を基準にする
        const left = p.left + parseFloat(cs.paddingLeft) + parseFloat(cs.borderLeftWidth);
        const right = p.right - parseFloat(cs.paddingRight) - parseFloat(cs.borderRightWidth);
        if (a.right > right + 0.5 || a.left < left - 0.5) out.push(`protrude:${el.className}`);
        if (a.right > w + 0.5) out.push(`page:${el.className}`);
      }
      return out;
    }, w);
    if (r.length) bad.push({ w, r });
  }
  return bad;
}

function verissBad(report) {
  const errs = report.diagnostics.filter((d) => d.severity === 'error' && d.range);
  return (w) => errs.some((d) => w >= d.range.min && (d.range.maxInclusive ? w <= d.range.max : w < d.range.max));
}

(async () => {
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const { analyzeHTML } = await import('@mizchi/veriss/html');
  const browser = await chromium.launch();
  const app = await browser.newPage();
  await app.goto(`http://127.0.0.1:${server.address().port}/`);
  await app.waitForFunction(() => window.lowerFor);
  const probe = await browser.newPage();

  const cases = [];
  for (const name of ['todo', 'settings']) cases.push({ label: `${name} (既定)`, app: name });
  cases.push({ label: 'todo (カード template)', app: 'todo', card: true });

  const results = [];
  for (const c of cases) {
    await app.evaluate(({ card }) => {
      const t = window.shell;
      const sig = t.signatures('todo').find((s) => s.node === 'list').sig;
      if (card) t.cache.propose(sig, card, 'diff');
      else t.cache.remove(sig);
    }, { card: c.card ? CARD : null });
    for (const px of [false, true]) {
      const low = await app.evaluate(([a, px]) => window.lowerFor(a, px), [c.app, px]);
      const report = analyzeHTML(low.html, { name: c.app, css: low.css, viewport: { ...DOMAIN, height: HEIGHT }, boxes: low.boxes, integrity: true, accessibility: true });
      const unknown = [...new Set(report.diagnostics.filter((d) => d.severity === 'unknown').map((d) => `${d.code}: ${d.message}`))];
      const codes = [...new Set(report.diagnostics.map((d) => `${d.severity}:${d.code}`))];
      const row = { case: c.label, pxIntrinsic: px, status: report.status, layoutMin: low.minWidth, unknown, codes };
      if (report.status !== 'unknown' || report.diagnostics.some((d) => d.severity === 'error')) {
        const vb = verissBad(report);
        const bb = await browserBad(probe, low);
        const bset = new Set(bb.map((b) => b.w));
        const missed = []; // ブラウザでは崩れるのに veriss が error を出さない幅
        const extra = []; // veriss は error だがブラウザでは崩れない幅
        for (let w = DOMAIN.min; w <= DOMAIN.max; w++) {
          if (bset.has(w) && !vb(w)) missed.push(w);
          if (!bset.has(w) && vb(w)) extra.push(w);
        }
        const firstOk = (pred) => {
          for (let w = DOMAIN.max; w >= DOMAIN.min; w--) if (pred(w)) return w + 1;
          return DOMAIN.min;
        };
        row.verissMin = firstOk(vb);
        row.browserMin = firstOk((w) => bset.has(w));
        row.missed = compact(missed);
        row.extra = compact(extra);
        if (missed.length) row.missedDetail = bb.find((b) => b.w === missed[0]).r.slice(0, 4);
      }
      results.push(row);
    }
  }
  await browser.close();
  server.close();
  for (const r of results) {
    console.log(`\n## ${r.case} pxIntrinsic=${r.pxIntrinsic}`);
    console.log(`status=${r.status} layout.ts min=${r.layoutMin}` + (r.verissMin !== undefined ? ` veriss min=${r.verissMin} browser min=${r.browserMin}` : ''));
    if (r.missed?.length) console.log(`  veriss が見逃した幅: ${r.missed}  例: ${r.missedDetail}`);
    if (r.extra?.length) console.log(`  veriss だけが error にした幅: ${r.extra}`);
    for (const u of r.unknown) console.log(`  unknown: ${u}`);
    console.log(`  codes: ${r.codes.join(' ')}`);
  }
  fs.writeFileSync(path.join(__dirname, '..', 'veriss-diff.json'), JSON.stringify(results, null, 2));
})();

function compact(ws) {
  const out = [];
  for (let i = 0; i < ws.length; ) {
    let j = i;
    while (j + 1 < ws.length && ws[j + 1] === ws[j] + 1) j++;
    out.push(i === j ? `${ws[i]}` : `${ws[i]}-${ws[j]}`);
    i = j + 1;
  }
  return out.join(', ');
}
