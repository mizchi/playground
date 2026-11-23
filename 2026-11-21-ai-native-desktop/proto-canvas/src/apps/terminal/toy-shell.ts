// ターミナルの中で動く小さなシェル (ホストの擬似端末が無いときの既定)。
// 出力は ANSI のエスケープ列で、描画は libghostty-vt が解釈した画面から行う。
// ファイルはアプリの保存領域 (storage)、copy / paste はクリップボード、open はアプリの起動。どれも能力の確認を通る。
import type { Actor } from '../../protocol.ts';
import type { AppServices, ServiceResult } from '../../platform/types.ts';

const ESC = '\x1b[';
const c = (code: string, s: string) => `${ESC}${code}m${s}${ESC}0m`;
export const PROMPT = `${c('1;32', 'you@ai-desktop')}:${c('1;34', '~')}$ `;

export interface ToyShellEnv {
  services?: AppServices;
  /** 最後に入力した actor */
  actor: Actor;
  history: string[];
  /** 今の日時 (テスト用) */
  now?: () => Date;
}

const nl = (s: string) => s.replace(/\r?\n/g, '\r\n');
const why = (r: ServiceResult<unknown>) =>
  r.ok ? '' : r.reason === 'pending' ? c('33', '許可を待っている (右のパネルで答える)。答えたら、もう一度実行する') : c('31', `できない (${r.reason})`);

type Fs = Record<string, string>;
const loadFs = (sv: AppServices): ServiceResult<Fs> => {
  const r = sv.storage.get('fs');
  if (!r.ok) return r;
  return { ok: true, value: r.value ? (JSON.parse(r.value) as Fs) : { 'readme.txt': 'このファイルはアプリの保存領域にある。\nwrite <名前> <内容> で書ける。' } };
};

const HELP = [
  ['help', 'この一覧'],
  ['echo <文字>', '表示する'],
  ['clear', '画面を消す'],
  ['date', '今の日時'],
  ['cal', '今月のカレンダー (今日は反転、予定はカレンダーのアプリから)'],
  ['colors', '色の見本 (16 色・256 色・24bit)'],
  ['ls / cat <名前> / write <名前> <内容> / rm <名前>', 'ファイル (storage 能力)'],
  ['copy <文字> / paste', 'クリップボード (clipboard 能力)'],
  ['notify <文字>', '通知 (notifications 能力)'],
  ['apps / open <アプリ>', 'アプリの一覧と起動 (apps.launch 能力)'],
  ['whoami', '最後に入力したのは人か agent か'],
  ['history', '入力の履歴'],
  ['seq <n>', '1..n を表示 (スクロールの確認)'],
];

/** 1 行のコマンドを実行し、端末に書く文字列 (ANSI) を返す */
export function runToyCommand(line: string, env: ToyShellEnv): string {
  const [cmd = '', ...args] = line.trim().split(/\s+/);
  const rest = line.trim().slice(cmd.length).trim();
  const sv = env.services;
  const need = () => c('31', 'このコマンドにはシステムのサービスが要る');
  switch (cmd) {
    case '':
      return '';
    case 'help':
      return HELP.map(([k, v]) => `  ${c('1', k.padEnd(44))} ${c('2', v)}`).join('\r\n');
    case 'echo':
      return rest;
    case 'clear':
      return `${ESC}2J${ESC}3J${ESC}H`;
    case 'date':
      return new Date().toString();
    case 'cal': {
      const n = env.now?.() ?? new Date();
      const y = n.getFullYear();
      const m = n.getMonth();
      // 予定はカレンダーのアプリから読む (data:calendar.events の能力。初めては人に尋ねる)
      const q = sv ? sv.data.query('calendar.events', { year: y, month: m }) : null;
      const events = q?.ok ? (q.value as { date: string; title: string }[]) : [];
      const busy = new Set(events.map((e) => Number(e.date.slice(8))));
      const first = new Date(y, m, 1).getDay();
      const days = new Date(y, m + 1, 0).getDate();
      const cell = (d: number) => {
        const s = String(d).padStart(2);
        const today = d === n.getDate();
        return today && busy.has(d) ? c('7;1;33', s) : today ? c('7', s) : busy.has(d) ? c('1;33', s) : s;
      };
      const cells = [...Array(first).fill('  '), ...Array.from({ length: days }, (_, i) => cell(i + 1))];
      const weeks = Array.from({ length: Math.ceil(cells.length / 7) }, (_, w) => cells.slice(w * 7, w * 7 + 7).join(' '));
      const out = [`      ${y}年${m + 1}月`, `${c('31', '日')} 月 火 水 木 金 ${c('34', '土')}`, ...weeks];
      if (q && !q.ok && q.reason !== 'unavailable') out.push(q.reason === 'pending' ? c('33', '予定はカレンダーの許可を待っている (右のパネルで答える)') : c('2', `予定は読めない (${q.reason})`));
      for (const e of events) out.push(`${c('1;33', e.date.slice(5).replace('-', '/'))} ${e.title}`);
      return out.join('\r\n');
    }
    case 'colors': {
      const basic = Array.from({ length: 16 }, (_, i) => `${ESC}${i < 8 ? 40 + i : 100 + i - 8}m  ${ESC}0m`).join('');
      const cube = Array.from({ length: 36 }, (_, i) => `${ESC}48;5;${16 + i * 6}m ${ESC}0m`).join('');
      const grad = Array.from({ length: 48 }, (_, i) => `${ESC}48;2;${Math.round((i / 47) * 255)};80;${255 - Math.round((i / 47) * 255)}m ${ESC}0m`).join('');
      const styles = `${c('1', '太字')} ${c('2', '薄い')} ${c('3', '斜体')} ${c('4', '下線')} ${c('7', '反転')} ${c('31', '赤')} ${c('32', '緑')} ${c('33', '黄')} ${c('34', '青')} ${c('35', '紫')} ${c('36', '水色')}`;
      return [basic, cube, grad, styles].join('\r\n');
    }
    case 'ls': {
      if (!sv) return need();
      const fs = loadFs(sv);
      if (!fs.ok) return why(fs);
      return Object.keys(fs.value).sort().map((n) => c('36', n)).join('  ') || c('2', '(空)');
    }
    case 'cat': {
      if (!sv) return need();
      const fs = loadFs(sv);
      if (!fs.ok) return why(fs);
      const f = fs.value[args[0] ?? ''];
      return f === undefined ? c('31', `cat: ${args[0] ?? ''}: ありません`) : nl(f);
    }
    case 'write':
    case 'rm': {
      if (!sv) return need();
      const fs = loadFs(sv);
      if (!fs.ok) return why(fs);
      const name = args[0];
      if (!name || !/^[\w.-]+$/.test(name)) return c('31', `${cmd}: 名前は英数字と . _ - だけ`);
      if (cmd === 'write') fs.value[name] = rest.slice(name.length).trim();
      else if (!(name in fs.value)) return c('31', `rm: ${name}: ありません`);
      else delete fs.value[name];
      const w = sv.storage.set('fs', JSON.stringify(fs.value));
      return w.ok ? '' : why(w);
    }
    case 'copy': {
      if (!sv) return need();
      const r = sv.clipboard.write(rest);
      return r.ok ? c('2', `${[...rest].length} 文字をコピーした`) : why(r);
    }
    case 'paste': {
      if (!sv) return need();
      const r = sv.clipboard.read();
      return r.ok ? nl(r.value) || c('2', '(クリップボードは空)') : why(r);
    }
    case 'notify': {
      if (!sv) return need();
      const r = sv.notify('Terminal', rest || '(本文なし)');
      return r.ok ? c('2', '通知した') : why(r);
    }
    case 'apps': {
      if (!sv) return need();
      const r = sv.apps.list();
      return r.ok ? r.value.map((a) => `  ${c('1', a.id.padEnd(10))} ${a.description}`).join('\r\n') : why(r);
    }
    case 'open': {
      if (!sv) return need();
      const r = sv.apps.launch(args[0] ?? '');
      return r.ok ? c('2', `${args[0]} を開いた`) : why(r);
    }
    case 'whoami':
      return env.actor === 'agent' ? c('35', 'agent (人が承認した提案で入力された)') : 'human';
    case 'history':
      return env.history.map((h, i) => `${String(i + 1).padStart(4)}  ${h}`).join('\r\n');
    case 'seq': {
      const n = Math.min(500, Math.max(0, Number(args[0] ?? 10) | 0));
      return Array.from({ length: n }, (_, i) => String(i + 1)).join('\r\n');
    }
    default:
      return c('31', `${cmd}: コマンドが見つかりません (help で一覧)`);
  }
}
