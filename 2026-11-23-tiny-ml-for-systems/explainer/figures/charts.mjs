// 実験の出力（checks.json が $TMP に保存したもの）から、Vega-Lite の spec を 2 枚作る。
// 値は出力からそのまま読む。手で写さない。
//
//   node figures/charts.mjs <bloom.txt> <cache.txt> [--write]
//
// --write なしでは、コミット済みの spec と一致するかだけを確かめる。
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const [bloomTxt, cacheTxt] = process.argv.slice(2).filter((a) => !a.startsWith('--')).map((p) => readFileSync(p, 'utf8'));

// ---- learned Bloom filter（目標 FPR 1%）
const bloom = [];
const plain = bloomTxt.match(/plain Bloom: ([\d.]+) KB/);
bloom.push({ name: '普通の Bloom filter', part: 'Bloom filter', kb: +plain[1] });
for (const m of bloomTxt.matchAll(/--- (.+?): (\d+) params[^\n]*\n[^\n]*\n\s*target 1\.000%: total [\d.]+ KB\n\s*model ([\d.]+) KB \+ backup ([\d.]+) KB/g)) {
  const name = m[1] === 'logistic regression' ? 'ロジスティック回帰' : m[1];
  bloom.push({ name, part: 'モデル', kb: +m[3] }, { name, part: 'backup', kb: +m[4] });
}
const order = bloom.map((d) => d.name).filter((v, i, a) => a.indexOf(v) === i);
const totals = order.map((name) => ({ name, total: +bloom.filter((d) => d.name === name).reduce((s, d) => s + d.kb, 0).toFixed(1) }));
for (const t of totals) console.log(`bloom ${t.name}: ${t.total} KB`);

// ---- キャッシュ admission
const cache = [];
const POLICY = {
  'LRU (admit all)': 'LRU（全部入れる）',
  'LRU + doorkeeper (2nd hit)': '2 回目で入れる',
  'LRU + MLP admission p>0.5': 'MLP で判断',
  'LRU + oracle admission': '上限（未来を見る）',
};
for (const sec of cacheTxt.split('=== cache = ').slice(1)) {
  const size = `キャッシュ ${sec.match(/^([\d.]+%)/)[1]}`;
  for (const [en, ja] of Object.entries(POLICY)) {
    const m = sec.match(new RegExp(`${en.replace(/[()+.]/g, '\\$&')}\\s+([\\d.]+)%`));
    cache.push({ size, policy: ja, ohr: +m[1] });
    console.log(`cache ${size} ${ja}: ${m[1]}%`);
  }
}

const config = {
  font: 'system-ui, -apple-system, "Hiragino Sans", "Noto Sans JP", sans-serif',
  axis: { labelFontSize: 13, titleFontSize: 14, grid: false },
  legend: { labelFontSize: 13, titleFontSize: 13, orient: 'bottom' },
  title: { fontSize: 15 },
  view: { stroke: null },
};
const W = 260;
// Vega は node で日本語の幅を短く見積もり、左の軸ラベルが切れる。左に余白を足す
const padding = { left: 64, right: 24, top: 8, bottom: 8 };

const bloomSpec = {
  $schema: 'https://vega.github.io/schema/vega-lite/v6.json',
  config,
  padding,
  title: '総サイズ（目標 FPR 1%） — 破線より短ければ得',
  width: W,
  height: 170,
  layer: [
    {
      data: { values: bloom },
      mark: 'bar',
      encoding: {
        y: { field: 'name', type: 'nominal', sort: order, title: null },
        x: { field: 'kb', type: 'quantitative', aggregate: 'sum', title: 'KB', scale: { domain: [0, 45] } },
        color: { field: 'part', type: 'nominal', title: null, sort: ['モデル', 'backup', 'Bloom filter'],
          scale: { domain: ['モデル', 'backup', 'Bloom filter'], range: ['#d1242f', '#0969da', '#6e7781'] } },
        order: { field: 'part', sort: 'descending' },
      },
    },
    {
      data: { values: totals },
      mark: { type: 'text', align: 'left', dx: 4, fontSize: 13 },
      encoding: { y: { field: 'name', type: 'nominal', sort: order }, x: { field: 'total', type: 'quantitative' }, text: { field: 'total', type: 'quantitative', format: '.1f' } },
    },
    {
      data: { values: [{ kb: totals[0].total }] },
      mark: { type: 'rule', strokeDash: [5, 4], color: '#6e7781' },
      encoding: { x: { field: 'kb', type: 'quantitative' } },
    },
  ],
};

const policies = Object.values(POLICY);
const cacheSpec = {
  $schema: 'https://vega.github.io/schema/vega-lite/v6.json',
  config,
  padding,
  title: 'ヒット率 — MLP が「2 回目」と上限の間のどこにあるか',
  vconcat: [...new Set(cache.map((d) => d.size))].map((size) => ({
    title: { text: size, fontSize: 14, anchor: 'start' },
    width: W,
    height: 130,
    data: { values: cache.filter((d) => d.size === size) },
    layer: [
      {
        mark: 'bar',
        encoding: {
          y: { field: 'policy', type: 'nominal', sort: policies, title: null },
          x: { field: 'ohr', type: 'quantitative', title: 'オブジェクトのヒット率（%）', scale: { domain: [0, 55] } },
          color: { field: 'policy', type: 'nominal', legend: null, sort: policies,
            scale: { domain: policies, range: ['#6e7781', '#6e7781', '#d1242f', '#0969da'] } },
        },
      },
      {
        mark: { type: 'text', align: 'left', dx: 4, fontSize: 13 },
        encoding: { y: { field: 'policy', type: 'nominal', sort: policies }, x: { field: 'ohr', type: 'quantitative' }, text: { field: 'ohr', type: 'quantitative', format: '.1f' } },
      },
    ],
  })),
};

let stale = 0;
for (const [file, spec] of [['bloom-size.vl.json', bloomSpec], ['cache-hit.vl.json', cacheSpec]]) {
  const path = join(here, file);
  const text = `${JSON.stringify(spec, null, 1)}\n`;
  if (process.argv.includes('--write')) { writeFileSync(path, text); console.log(`spec: wrote figures/${file}`); continue; }
  let cur = '';
  try { cur = readFileSync(path, 'utf8'); } catch {}
  if (cur === text) console.log(`spec: figures/${file} is up to date`);
  else { stale++; console.log(`spec: figures/${file} is STALE (run with --write)`); }
}
process.exit(stale ? 1 : 0);
