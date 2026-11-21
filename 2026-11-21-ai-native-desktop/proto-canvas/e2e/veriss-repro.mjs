// mizchi/veriss に報告した Unknown の最小再現。この試作のコードに依存しない。
// 使い方: node e2e/veriss-repro.mjs   (Node 24 以上)
import { analyzeHTML } from '@mizchi/veriss/html';

const T = (w) => ({ minInlineSize: w, maxInlineSize: w, intrinsicBlockSize: 20, wrap: 'nowrap' });
const font = 'font: 400 14px/20px Arial; white-space: nowrap;';
const run = (label, html, css, boxes, extra = {}) => {
  const r = analyzeHTML(html, {
    name: 'Repro',
    css: `* { box-sizing: border-box; margin: 0; padding: 0; }\n${css}`,
    viewport: { min: 100, max: 600, height: { min: 200, max: 800 } },
    boxes,
    ...extra,
  });
  console.log(`\n### ${label}: ${r.status}`);
  for (const d of r.diagnostics) console.log(' ', d.severity, d.code, d.box ?? '', d.range ? `[${d.range.min}, ${d.range.max})` : '', d.message);
};

// A: column の伸長しない item。文字の契約は min = max (max-content が確定している)
run('A column + align-items:flex-start + 文字の契約', '<main class="col"><div class="t">hello</div></main>',
  `.col { display: flex; flex-direction: column; align-items: flex-start; } .t { ${font} }`, { '.t': T(40) });
run('A 対照: width: 40px を明示', '<main class="col"><div class="t">hello</div></main>',
  `.col { display: flex; flex-direction: column; align-items: flex-start; } .t { ${font} width: 40px; }`, { '.t': T(40) });
run('A2 align-self: center', '<main class="col"><div class="t">hello</div></main>',
  `.col { display: flex; flex-direction: column; } .t { ${font} align-self: center; }`, { '.t': T(40) });

// B: row の item、flex-basis: auto、子もテキストもない箱 + min-width
run('B row + 中身のない item (flex: 1 0 auto; min-width: 160px)', '<main class="row"><div class="f"></div></main>',
  '.row { display: flex; } .f { flex: 1 0 auto; min-width: 160px; height: 32px; }', {});
run('B 対照: flex: 1 0 160px', '<main class="row"><div class="f"></div></main>',
  '.row { display: flex; } .f { flex: 1 0 160px; min-width: 160px; height: 32px; }', {});

// 参考: row の item で、中身が px の箱 + 文字の契約の入れ子 flex なら verified になる
run('参考 row + flex: none の入れ子 (px + 文字)', '<main class="row"><div class="cb"><div class="i"></div><div class="l">label</div></div></main>',
  `.row { display: flex; } .cb { flex: none; display: flex; align-items: center; gap: 8px; height: 26px; } .i { flex: none; width: 18px; height: 18px; } .l { flex: none; ${font} }`,
  { '.l': T(40) });
