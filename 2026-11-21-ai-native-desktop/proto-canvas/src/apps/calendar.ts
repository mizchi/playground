// カレンダー: プラットフォームのアプリの例。月の表は ARIA の table / row / columnheader / cell で出す。
// - 日のセルは click で選ぶ (selected)。予定のある日は「•」が付く
// - 予定はアプリの保存領域 (storage)。今日の予定があれば、開いたときに通知する (notifications)
// - 予定の削除は sensitive: agent からは毎回、人の承認が要る
import type { AppManifest, AppServices } from '../platform/types.ts';
import type { ActionRequest, NodeId, SemanticNode, TreeUpdate } from '../protocol.ts';
import type { App } from './app.ts';

const WEEK = ['日', '月', '火', '水', '木', '金', '土'];
const pad = (n: number) => String(n).padStart(2, '0');
export const dateKey = (y: number, m: number, d: number) => `${y}-${pad(m + 1)}-${pad(d)}`;

/** 月の格子: 週ごとに 7 日 (前後の月の日は 0) */
export function monthGrid(year: number, month: number): number[][] {
  const first = new Date(year, month, 1).getDay();
  const days = new Date(year, month + 1, 0).getDate();
  const cells = [...Array(first).fill(0), ...Array.from({ length: days }, (_, i) => i + 1)];
  while (cells.length % 7) cells.push(0);
  return Array.from({ length: cells.length / 7 }, (_, w) => cells.slice(w * 7, w * 7 + 7));
}

export function createCalendarApp(opts: { id?: string; services?: AppServices; now?: () => Date } = {}): App {
  const appId = opts.id ?? 'calendar';
  const sv = opts.services;
  const now = opts.now ?? (() => new Date());
  const today = now();
  let year = today.getFullYear();
  let month = today.getMonth();
  let selected = today.getDate();
  let events: Record<string, string[]> = {};
  let draft = '';
  let status = '';
  let focus: NodeId | null = null;
  let sent = new Set<NodeId>();
  let emit: (u: TreeUpdate) => void = () => {};
  if (sv) {
    const r = sv.storage.get('events');
    if (r.ok && r.value) events = JSON.parse(r.value) as Record<string, string[]>;
    sv.storage.onChange((key) => {
      if (key !== 'events') return;
      const r2 = sv.storage.get('events');
      if (r2.ok) events = r2.value ? (JSON.parse(r2.value) as Record<string, string[]>) : {};
      publish();
    });
  }
  const save = () => sv?.storage.set('events', JSON.stringify(events));
  const selKey = () => dateKey(year, month, selected);

  const publish = (root?: string) => {
    const grid = monthGrid(year, month);
    const isToday = (d: number) => d === today.getDate() && month === today.getMonth() && year === today.getFullYear();
    const list = events[selKey()] ?? [];
    const wd = WEEK[new Date(year, month, selected).getDay()];
    const nodes: SemanticNode[] = [
      { id: 'root', role: 'x-window', label: appId === 'calendar' ? 'Calendar' : `Calendar (${appId})`, children: ['title', 'nav', 'grid', 'dayTitle', 'events', 'entry', 'status'] },
      { id: 'title', role: 'heading', label: `${year}年${month + 1}月` },
      { id: 'nav', role: 'group', label: '月の移動', children: ['prev', 'today', 'next'] },
      { id: 'prev', role: 'button', label: '前の月', actions: ['click', 'focus'] },
      { id: 'today', role: 'button', label: '今日', actions: ['click', 'focus'] },
      { id: 'next', role: 'button', label: '次の月', actions: ['click', 'focus'] },
      { id: 'grid', role: 'table', label: `${year}年${month + 1}月`, children: ['head', ...grid.map((_, w) => `w${w}`)] },
      { id: 'head', role: 'row', children: WEEK.map((_, i) => `h${i}`) },
      ...WEEK.map((d, i): SemanticNode => ({ id: `h${i}`, role: 'columnheader', label: d })),
    ];
    grid.forEach((week, w) => {
      nodes.push({ id: `w${w}`, role: 'row', children: week.map((_, i) => `c${w}-${i}`) });
      week.forEach((d, i) => {
        const id = `c${w}-${i}`;
        if (!d) nodes.push({ id, role: 'cell', label: '' });
        else {
          const has = (events[dateKey(year, month, d)] ?? []).length > 0;
          // 日付と、予定があれば「•」(セルは等幅なので、名前を長くすると表全体が広がる)
          nodes.push({ id, role: 'cell', label: `${d}${has ? '•' : ''}`, selected: d === selected, actions: ['click', 'focus'] });
        }
      });
    });
    nodes.push(
      { id: 'dayTitle', role: 'heading', label: `${month + 1}月${selected}日 (${wd})${isToday(selected) ? ' 今日' : ''} の予定` },
      { id: 'events', role: 'list', label: '予定', children: list.length ? list.map((_, i) => `e${i}`) : ['none'] },
    );
    if (!list.length) nodes.push({ id: 'none', role: 'paragraph', label: '予定はない' });
    list.forEach((t, i) => {
      nodes.push(
        { id: `e${i}`, role: 'listitem', label: t, children: [`e${i}t`, `e${i}x`] },
        { id: `e${i}t`, role: 'paragraph', label: t },
        { id: `e${i}x`, role: 'button', label: '削除', sensitive: true, actions: ['click', 'focus'] },
      );
    });
    nodes.push(
      { id: 'entry', role: 'group', label: '予定を追加', children: ['input', 'add'] },
      // Enter (click) でも追加できる
      { id: 'input', role: 'textbox', label: '新しい予定', value: draft, actions: ['focus', 'setValue', 'replaceSelectedText', 'click'] },
      { id: 'add', role: 'button', label: '追加', actions: ['click', 'focus'] },
      { id: 'status', role: 'paragraph', label: status || `${Object.values(events).reduce((s, l) => s + l.length, 0)} 件の予定` },
    );
    const ids = new Set(nodes.map((n) => n.id));
    const remove = [...sent].filter((id) => !ids.has(id));
    sent = ids;
    emit({ t: 'tree-update', app: appId, root, focus, nodes, remove });
  };

  const move = (delta: number) => {
    const d = new Date(year, month + delta, 1);
    year = d.getFullYear();
    month = d.getMonth();
    selected = Math.min(selected, new Date(year, month + 1, 0).getDate());
  };

  return {
    id: appId,
    start(e) {
      emit = e;
      publish('root');
      const t = events[dateKey(today.getFullYear(), today.getMonth(), today.getDate())] ?? [];
      if (sv && t.length) sv.notify('Calendar', `今日の予定 ${t.length} 件: ${t.join(' / ')}`);
    },
    onAction(req: ActionRequest) {
      const t = req.target;
      if (req.action === 'focus') focus = t;
      else if (t === 'input' && req.action === 'setValue') draft = req.data ?? '';
      else if (t === 'input' && req.action === 'replaceSelectedText') draft += req.data ?? '';
      else if ((t === 'add' || t === 'input') && req.action === 'click') {
        const text = draft.trim();
        if (text) {
          (events[selKey()] ??= []).push(text);
          save();
          status = `${month + 1}月${selected}日に「${text}」を追加した${req.by === 'agent' ? ' (agent)' : ''}`;
        }
        draft = '';
      } else if (t === 'prev') move(-1);
      else if (t === 'next') move(1);
      else if (t === 'today') {
        const n = now();
        year = n.getFullYear();
        month = n.getMonth();
        selected = n.getDate();
      } else if (/^c\d+-\d$/.test(t)) {
        const [w, i] = t.slice(1).split('-').map(Number);
        const d = monthGrid(year, month)[w]?.[i];
        if (d) selected = d;
      } else if (/^e\d+x$/.test(t)) {
        const list = events[selKey()] ?? [];
        const i = Number(t.slice(1, -1));
        const [gone] = list.splice(i, 1);
        if (!list.length) delete events[selKey()];
        save();
        status = `「${gone}」を削除した`;
      }
      publish();
    },
  };
}

export interface CalendarEvent {
  date: string;
  title: string;
}

/** data:calendar.events の答え: 指定した月 (省略で全部) の予定。窓を開いていなくても、保存領域から読む */
export function calendarEvents(services: AppServices, args: { year?: number; month?: number }): CalendarEvent[] {
  const r = services.storage.get('events');
  const events = r.ok && r.value ? (JSON.parse(r.value) as Record<string, string[]>) : {};
  const prefix = typeof args.year === 'number' && typeof args.month === 'number' ? `${args.year}-${pad(args.month + 1)}-` : '';
  return Object.keys(events)
    .filter((d) => d.startsWith(prefix))
    .sort()
    .flatMap((date) => events[date].map((title) => ({ date, title })));
}

export const calendarManifest: AppManifest = {
  id: 'calendar',
  title: 'Calendar',
  description: '月の予定。予定を保存し、今日の予定を通知する',
  kind: 'js',
  capabilities: ['storage', 'notifications'],
  multiInstance: false,
  create: (id, services) => createCalendarApp({ id, services }),
  provides: { 'calendar.events': { description: '月の予定 (日付と題名。読み取りだけ)' } },
  provide: (topic, args, services) => (topic === 'calendar.events' ? calendarEvents(services, args as { year?: number; month?: number }) : null),
};
