// アプリどうしの書き込みの依頼 (intent): 受ける側が handles で宣言し、頼む側は intent:<topic> の能力を宣言する。
// 頼むたびに人が答える。人に見せる説明は受ける側が作る。実行は受ける側の能力付きのサービスで行い、開いている窓は読み直す
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { calendarManifest, parseAddEvent } from '../src/apps/calendar.ts';
import { builtinManifests } from '../src/apps/manifests.ts';
import { terminalManifest } from '../src/apps/terminal/terminal.ts';
import { createVt, instantiateGhostty } from '../src/apps/terminal/vt.ts';
import { memoryStorage } from '../src/cache.ts';
import { createPlatform } from '../src/platform/platform.ts';
import { memoryKeyValue } from '../src/platform/services.ts';
import type { AppManifest, ServiceResult } from '../src/platform/types.ts';

const measure = (s: string, size: number) => [...s].length * size * 0.6;
const ghostty = instantiateGhostty(await readFile(new URL('../node_modules/ghostty-web/ghostty-vt.wasm', import.meta.url)));
const terminal = terminalManifest(async () => {
  const g = await ghostty;
  return (c, r) => createVt(g, c, r);
});
const tick = () => new Promise((r) => setTimeout(r, 0));
/** 頼む側の見本 (intent:calendar.add-event を宣言する) */
const asker = (caps: AppManifest['capabilities']): AppManifest => ({
  id: 'asker',
  title: 'Asker',
  description: '',
  kind: 'js',
  capabilities: caps,
  multiInstance: false,
  create: (id) => ({ id, start() {}, onAction() {} }),
});
const make = async (extra: AppManifest[] = [], initial = ['calendar']) => {
  const kv = memoryKeyValue();
  const p = createPlatform({ apps: [...builtinManifests, terminal, ...extra], initial, measure, templates: memoryStorage(), kv });
  p.shell.animations = false;
  await tick();
  p.shell.frame(1400, 800);
  return { p, kv };
};

test('引数の形: 実在する YYYY-MM-DD と、1..100 文字で制御文字の無い題名だけ', () => {
  assert.deepEqual(parseAddEvent({ date: '2026-11-24', title: ' 会議 ' }), { date: '2026-11-24', title: '会議' });
  assert.equal(parseAddEvent({ date: '2026-02-30', title: 'x' }), null);
  assert.equal(parseAddEvent({ date: '2026-11-24', title: '' }), null);
  assert.equal(parseAddEvent({ date: '2026-11-24', title: 'a\u001b[31m' }), null);
  assert.equal(parseAddEvent({ date: '2026-11-24', title: 'あ'.repeat(101) }), null);
});

test('宣言していないアプリは頼めない。受けるアプリが無ければ unavailable、引数が合わなければ人に出さずに invalid', async () => {
  const { p } = await make([asker([])]);
  const undeclared = p.services.servicesFor('asker', 'asker').intents.request('calendar.add-event', { date: '2026-11-24', title: '会議' });
  assert.deepEqual(undeclared, { ok: false, reason: 'undeclared' });
  const { p: p2 } = await make([asker(['intent:calendar.add-event', 'intent:nothing'])]);
  const sv = p2.services.servicesFor('asker', 'asker');
  assert.deepEqual(sv.intents.request('nothing', {}), { ok: false, reason: 'unavailable' });
  assert.deepEqual(sv.intents.request('calendar.add-event', { date: 'あした', title: '会議' }), { ok: false, reason: 'invalid' });
  assert.equal(p2.services.intents.length, 0, '人には出さない');
});

test('頼むと人の答えを待つ。説明は受ける側が作る。答えられるのは人だけ。実行すると開いている Calendar が読み直す', async () => {
  const { p, kv } = await make([asker(['intent:calendar.add-event'])]);
  const results: ServiceResult<unknown>[] = [];
  // 今日の日付で頼む (Calendar は今日を選んでいるので、読み直すと選んだ日の予定に出る)
  const d = new Date();
  const today = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  const r = p.services.servicesFor('asker', 'asker').intents.request('calendar.add-event', { date: today, title: '会議', summary: '何もしない' }, (x) => results.push(x));
  assert.ok(r.ok);
  const req = p.services.intents[0];
  assert.equal(req.summary, `${today} に予定「会議」を追加する`, '頼む側の summary は使わない');
  assert.equal(req.status, 'pending');
  assert.equal(kv.get('app.calendar.events'), undefined, '答えるまでは書かない');
  assert.equal(p.services.answerIntent(req.id, true, 'agent'), '依頼に答えられるのは人だけ');
  assert.equal(p.services.answerIntent(req.id, true, 'human'), '');
  assert.equal(req.status, 'done');
  assert.deepEqual(results, [{ ok: true, value: { date: today, title: '会議' } }]);
  assert.deepEqual(JSON.parse(kv.get('app.calendar.events')!), { [today]: ['会議'] });
  // 開いている Calendar の窓が読み直した (選んでいる今日の予定に出る)
  assert.ok(p.shell.tree('calendar').walk().some((n) => n.label === '会議'), 'Calendar の窓に予定が出る');
  assert.ok(p.services.audit.some((a) => a.op.startsWith('handle calendar.add-event from asker')));
  assert.equal(p.services.answerIntent(req.id, true, 'human'), '依頼が無い', '二度は実行しない');
});

test('人が断ると、頼んだ側に denied が届き、何も書かない', async () => {
  const { p, kv } = await make([asker(['intent:calendar.add-event'])]);
  const results: ServiceResult<unknown>[] = [];
  p.services.servicesFor('asker', 'asker').intents.request('calendar.add-event', { date: '2026-11-24', title: '会議' }, (x) => results.push(x));
  p.services.answerIntent(p.services.intents[0].id, false, 'human');
  assert.deepEqual(results, [{ ok: false, reason: 'denied' }]);
  assert.equal(p.services.intents[0].status, 'rejected');
  assert.equal(kv.get('app.calendar.events'), undefined);
});

test('写せない引数や、長すぎる・制御文字を含む説明は、人に出さずに invalid', async () => {
  const long: AppManifest = {
    ...asker([]),
    id: 'receiver',
    title: 'Receiver',
    handles: { 'x.do': { description: '' } },
    describe: (_t, a) => String(a.text),
    handle: () => ({ ok: true, value: null }),
  };
  const { p } = await make([asker(['intent:calendar.add-event', 'intent:x.do']), long]);
  const sv = p.services.servicesFor('asker', 'asker');
  assert.deepEqual(sv.intents.request('calendar.add-event', { date: '2026-11-24', title: Symbol('x') as unknown as string }), { ok: false, reason: 'invalid' });
  assert.deepEqual(sv.intents.request('x.do', { text: 'あ'.repeat(201) }), { ok: false, reason: 'invalid' });
  assert.deepEqual(sv.intents.request('x.do', { text: '1行目\n2行目' }), { ok: false, reason: 'invalid' });
  assert.ok(sv.intents.request('x.do', { text: 'あ'.repeat(200) }).ok);
});

test('agent のワークスペースからは頼めない (外へ出る能力と同じ)', async () => {
  const { p } = await make([asker(['intent:calendar.add-event'])]);
  const r = p.services.servicesFor('asker', 'asker-2', 'agent').intents.request('calendar.add-event', { date: '2026-11-24', title: '会議' });
  assert.deepEqual(r, { ok: false, reason: 'denied' });
  assert.equal(p.services.intents.length, 0);
});

test('Terminal の cal add: 依頼を出し、人が答えると結果を端末に書く。Calendar を開いていなくても受けられる', async () => {
  const { p, kv } = await make([], ['terminal']);
  // libghostty-vt を読み終えるまで、Terminal はコマンドを受けない
  for (let i = 0; i < 100 && !/libghostty-vt/.test(p.shell.tree('terminal').get('screen')?.value ?? ''); i++) await new Promise((r) => setTimeout(r, 20));
  const run = async (cmd: string) => {
    p.shell.dispatch({ app: 'terminal', target: 'input', action: 'setValue', data: cmd, by: 'human' });
    p.shell.dispatch({ app: 'terminal', target: 'input', action: 'click', by: 'human' });
    await tick();
  };
  const screen = () => p.shell.tree('terminal').get('screen')!.value ?? '';
  await run('cal add 2026-11-24 チームの会議');
  assert.match(screen(), /カレンダーに頼んだ/);
  assert.equal(p.services.intents[0].summary, '2026-11-24 に予定「チームの会議」を追加する');
  p.services.answerIntent(p.services.intents[0].id, true, 'human');
  assert.match(screen(), /カレンダーに追加した: 2026-11-24 チームの会議/);
  assert.deepEqual(JSON.parse(kv.get('app.calendar.events')!), { '2026-11-24': ['チームの会議'] });
  await run('cal add 2026-13-01 x');
  assert.match(screen(), /使い方: cal add/);
});

test('Calendar の manifest: handles と describe / handle を持つ', () => {
  assert.ok(calendarManifest.handles?.['calendar.add-event']);
  assert.equal(calendarManifest.describe!('calendar.add-event', { date: 'x' }), null);
});
