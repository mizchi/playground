import assert from 'node:assert/strict';
import { test } from 'node:test';
import { calendarManifest, createCalendarApp, monthGrid } from '../src/apps/calendar.ts';
import { builtinManifests } from '../src/apps/manifests.ts';
import { memoryStorage } from '../src/cache.ts';
import { createPlatform } from '../src/platform/platform.ts';
import { memoryKeyValue } from '../src/platform/services.ts';
import type { KeyValue } from '../src/platform/services.ts';
import { Shell } from '../src/shell.ts';

const measure = (s: string, size: number) => [...s].length * size * 0.6;
const NOW = () => new Date(2026, 10, 22, 9, 0); // 2026-11-22 (日)
const fixed = { ...calendarManifest, create: (id: string, services: never) => createCalendarApp({ id, services, now: NOW }) };
const make = (kv: KeyValue = memoryKeyValue()) => {
  const p = createPlatform({ apps: [...builtinManifests.filter((m) => m.id !== 'calendar'), fixed as typeof calendarManifest], initial: ['calendar'], measure, templates: memoryStorage(), kv });
  p.shell.animations = false;
  p.shell.frame(1200, 800);
  return p;
};
type P = ReturnType<typeof make>;
const node = (p: P, id: string) => p.shell.tree('calendar').get(id)!;
const click = (p: P, target: string, by: 'human' | 'agent' = 'human') => p.shell.dispatch({ app: 'calendar', target, action: 'click', by });
const add = (p: P, text: string) => {
  p.shell.dispatch({ app: 'calendar', target: 'input', action: 'setValue', data: text, by: 'human' });
  click(p, 'add');
};
const cellOf = (p: P, day: string) => [...p.shell.tree('calendar').nodes.values()].find((n) => n.role === 'cell' && n.label?.replace('•', '') === day)!;

test('月の格子: 2026 年 11 月は日曜始まりで 5 週', () => {
  const g = monthGrid(2026, 10);
  assert.equal(g.length, 5);
  assert.deepEqual(g[0], [1, 2, 3, 4, 5, 6, 7]);
  assert.deepEqual(g[4], [29, 30, 0, 0, 0, 0, 0]);
});

test('月の表は ARIA の table / row / columnheader / cell。今日が選ばれている', () => {
  const p = make();
  assert.equal(node(p, 'title').label, '2026年11月');
  assert.equal(node(p, 'grid').role, 'table');
  assert.deepEqual(node(p, 'head').children!.map((id) => node(p, id).label), ['日', '月', '火', '水', '木', '金', '土']);
  const sel = [...p.shell.tree('calendar').nodes.values()].filter((n) => n.selected);
  assert.deepEqual(sel.map((n) => n.label), ['22']);
  assert.equal(node(p, 'dayTitle').label, '11月22日 (日) 今日 の予定');
});

test('列は行どうしで揃う (等幅の行)', () => {
  const p = make();
  const f = p.shell.frame(1200, 800);
  // 「日」の列: 1, 8, 15 の中心が揃う (文字幅の差は中央寄せで吸収)
  const centers = ['1', '8', '15', '22', '29'].map((d) => {
    const o = f.ops.find((x) => x[0] === 'text' && x[1].str === d)!;
    return o[0] === 'text' ? o[1].x + measure(d, 14) / 2 : NaN;
  });
  // 14 は M.fontSize。描画は文字幅を切り上げて中央に置くので、1px 未満の差は許す
  for (const c of centers) assert.ok(Math.abs(c - centers[0]) < 1, `中心 ${centers}`);
});

test('日を選んで予定を追加すると、保存され、セルに • が付く。月を移動しても残る', () => {
  const kv = memoryKeyValue();
  const p = make(kv);
  click(p, cellOf(p, '25').id);
  assert.equal(node(p, 'dayTitle').label, '11月25日 (水) の予定');
  add(p, '歯医者');
  assert.equal(cellOf(p, '25').label, '25•');
  assert.deepEqual(node(p, 'events').children!.map((id) => node(p, id).label), ['歯医者']);
  assert.match(kv.get('app.calendar.events')!, /2026-11-25/);
  click(p, 'next');
  assert.equal(node(p, 'title').label, '2026年12月');
  click(p, 'prev');
  assert.equal(cellOf(p, '25').label, '25•');
  // 次の起動でも残る
  const q = make(kv);
  click(q, cellOf(q, '25').id);
  assert.equal(node(q, 'e0t').label, '歯医者');
});

test('今日の予定があれば、開いたときに通知する (許可のあと)', () => {
  const kv = memoryKeyValue();
  const p = make(kv);
  add(p, '定例');
  const q = make(kv);
  assert.equal(q.services.prompts[0]?.capability, 'notifications');
  q.services.answer(q.services.prompts[0].id, true, 'human');
  // 許可は保存されるので、次に開いたときは通知が届く
  const r = make(kv);
  assert.equal(r.services.notifications[0]?.body, '今日の予定 1 件: 定例');
});

test('agent は予定を提案で追加できる (自動承認可)。削除は sensitive で承認が要る', () => {
  const p = make();
  p.shell.policy.autoApproveSafe = true;
  const a = p.shell.propose('明日に会議を入れる', [
    { app: 'calendar', target: cellOf(p, '23').id, action: 'click' },
    { app: 'calendar', target: 'input', action: 'setValue', data: '会議' },
    { app: 'calendar', target: 'add', action: 'click' },
  ]);
  assert.ok(a.ok);
  assert.equal(a.proposal.status, 'done');
  assert.match(node(p, 'status').label!, /「会議」を追加した \(agent\)/);
  const d = p.shell.propose('会議を消す', [{ app: 'calendar', target: 'e0x', action: 'click' }]);
  assert.ok(d.ok);
  assert.equal(d.proposal.sensitive, true);
  assert.equal(d.proposal.status, 'pending');
  p.shell.approve(d.proposal.id);
  assert.equal(node(p, 'none').label, '予定はない');
});

test('既定レンダラだけで描け、template を通しても操作できるセルを落とさない', () => {
  const shell = new Shell([createCalendarApp({ now: NOW })], measure, memoryStorage());
  const f = shell.frame(900, 700);
  assert.equal(f.windows[0].issues.length, 0);
  assert.ok(f.windows[0].minWidth < 600, `最小幅 ${f.windows[0].minWidth}`);
});
