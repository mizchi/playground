// ターミナル: プラットフォームのアプリの例。
// - 端末の状態機械は libghostty-vt (wasm)。アプリはバイト列を書き、画面を意味ツリーの log ノードとして出す
// - 既定は小さな内蔵シェル (toy-shell.ts)。ホストが擬似端末の橋を用意していれば (?pty=)、terminal.pty 能力で本物のシェルにつなぐ
// - コマンド行は sensitive: agent の入力は毎回、人の承認が要る
import type { Actor, ActionRequest, NodeId, SemanticNode, TerminalScreen, TreeUpdate } from '../../protocol.ts';
import type { AppManifest, AppServices, PtyHandle } from '../../platform/types.ts';
import type { App } from '../app.ts';
import { PROMPT, runToyCommand } from './toy-shell.ts';
import type { ToyShellEnv } from './toy-shell.ts';
import type { Vt, VtFactory } from './vt.ts';

export interface TerminalOptions {
  id?: string;
  services?: AppServices;
  /** libghostty-vt の読み込み (窓どうしで共有する) */
  vt: () => Promise<VtFactory>;
  cols?: number;
  rows?: number;
}

const WELCOME = '\x1b[1mTerminal\x1b[0m \x1b[2m— libghostty-vt (wasm) で動く。help でコマンドの一覧\x1b[0m\r\n';

export function createTerminalApp(opts: TerminalOptions): App {
  const appId = opts.id ?? 'terminal';
  // 窓に合わせて変わる (shell が onResize で知らせる)。最初は既定の大きさ
  let cols = opts.cols ?? 72;
  let rows = opts.rows ?? 18;
  const sv = opts.services;
  let emit: (u: TreeUpdate) => void = () => {};
  let vt: Vt | null = null;
  let pty: PtyHandle | null = null;
  let draft = '';
  let focus: NodeId | null = 'input';
  let status = 'libghostty-vt を読み込んでいる';
  let stopped = false;
  let sent = new Set<NodeId>();
  const env: ToyShellEnv = { services: sv, actor: 'human', history: [] };
  const empty = (): TerminalScreen => ({ cols, rows, lines: Array.from({ length: rows }, () => []), cursor: null, fg: '#e6e8ef', bg: '#15171d' });

  const promptLine = () => vt?.write(`\r\x1b[2K${PROMPT}${draft}`);
  // あとから届いた結果 (依頼の答えなど) は、今の入力行の上に書き、入力行を書き直す
  env.print = (s) => {
    // 窓を閉じたあと (stop で端末を手放したあと) に答えが届いても書かない
    if (!vt || pty || stopped) return;
    vt.write(`\r\x1b[2K${s}\r\n${PROMPT}${draft}`);
    publish();
  };

  const publish = (root?: string) => {
    const snap = vt?.snapshot();
    const canPty = !!sv?.pty.available();
    const nodes: SemanticNode[] = [
      { id: 'root', role: 'x-window', label: appId === 'terminal' ? 'Terminal' : `Terminal (${appId})`, children: ['screen', 'entry', 'status'] },
      { id: 'screen', role: 'log', label: '画面', value: snap?.text ?? '', screen: snap?.screen ?? empty(), actions: ['focus'] },
      { id: 'entry', role: 'group', label: 'コマンド行', children: ['input', 'run', ...(pty ? ['ctrlc'] : canPty ? ['host'] : [])] },
      // Enter (click) で実行。agent からの入力はシェルを動かすので、毎回承認が要る
      { id: 'input', role: 'textbox', label: pty ? 'ホストのシェルへの入力' : 'コマンド', value: draft, sensitive: true, actions: ['focus', 'setValue', 'replaceSelectedText', 'click'] },
      { id: 'run', role: 'button', label: '実行', actions: ['click', 'focus'] },
    ];
    if (pty) nodes.push({ id: 'ctrlc', role: 'button', label: 'Ctrl+C', actions: ['click', 'focus'] });
    else if (canPty) nodes.push({ id: 'host', role: 'button', label: 'ホストのシェルにつなぐ', sensitive: true, actions: ['click', 'focus'] });
    nodes.push({ id: 'status', role: 'paragraph', label: status });
    const ids = new Set(nodes.map((n) => n.id));
    const remove = [...sent].filter((id) => !ids.has(id));
    sent = ids;
    emit({ t: 'tree-update', app: appId, root, focus, nodes, remove });
  };

  const connectPty = () => {
    if (!sv || !vt || pty) return;
    const v = vt;
    const r = sv.pty.open(cols, rows, (data) => {
      v.write(data);
      publish();
    }, (why) => {
      pty = null;
      v.write(`\r\n\x1b[33m[ホストのシェルが終わった: ${why}]\x1b[0m\r\n${PROMPT}`);
      status = '内蔵シェル';
      publish();
    });
    if (r.ok) {
      pty = r.value;
      draft = '';
      v.write('\x1b[2J\x1b[3J\x1b[H');
      status = 'ホストのシェル (擬似端末) につないでいる';
    } else status = r.reason === 'pending' ? 'ホストのシェルの許可を待っている' : `ホストのシェルにつなげない (${r.reason})`;
  };

  const submit = (by: Actor) => {
    if (!vt) return;
    if (pty) {
      pty.write(`${draft}\r`);
      draft = '';
      return;
    }
    const line = draft;
    draft = '';
    env.actor = by;
    if (line.trim()) env.history.push(line);
    vt.write('\r\n');
    const out = runToyCommand(line, env);
    if (line.trim() === 'clear') vt.write(out + PROMPT);
    else vt.write(`${out}${out ? '\r\n' : ''}${PROMPT}`);
  };

  return {
    id: appId,
    start(e) {
      emit = e;
      publish('root');
      sv?.onGrant((cap, granted) => {
        if (cap !== 'terminal.pty') return;
        if (granted) connectPty();
        else status = 'ホストのシェルは拒否された。内蔵シェルで続ける';
        publish();
      });
      opts.vt().then(
        (make) => {
          if (stopped) return;
          vt = make(cols, rows);
          vt.write(WELCOME + PROMPT);
          status = `内蔵シェル`;
          publish();
        },
        (err) => {
          status = `libghostty-vt を読み込めない: ${String((err as Error).message ?? err)}`;
          publish();
        },
      );
    },
    onAction(req: ActionRequest) {
      if (req.action === 'focus') focus = req.target === 'screen' ? 'input' : req.target; // 画面を押したらコマンド行へ
      else if (req.target === 'input' && req.action === 'setValue') draft = req.data ?? '';
      else if (req.target === 'input' && req.action === 'replaceSelectedText') draft += req.data ?? '';
      else if ((req.target === 'input' || req.target === 'run') && req.action === 'click') submit(req.by);
      else if (req.target === 'ctrlc' && req.action === 'click') pty?.write('\x03');
      else if (req.target === 'host' && req.action === 'click') connectPty();
      if (!pty && (req.action === 'setValue' || req.action === 'replaceSelectedText')) promptLine();
      publish();
    },
    onResize(node, c, r) {
      if (node !== 'screen' || (c === cols && r === rows)) return;
      cols = c;
      rows = r;
      vt?.resize(c, r);
      pty?.resize(c, r);
      publish();
    },
    stop() {
      stopped = true;
      pty?.close();
      vt?.free();
    },
  };
}

/** libghostty-vt を読み込む関数を渡して manifest を作る (ブラウザは fetch、テストは readFile)。読み込みは最初の窓を開くときに 1 回だけ */
export function terminalManifest(load: () => Promise<VtFactory>): AppManifest {
  let shared: Promise<VtFactory> | null = null;
  const once = () => (shared ??= load());
  return {
    id: 'terminal',
    title: 'Terminal',
    description: 'libghostty-vt の端末。内蔵シェル、またはホストのシェル',
    kind: 'js',
    capabilities: ['storage', 'clipboard.read', 'clipboard.write', 'notifications', 'apps.launch', 'terminal.pty', 'data:calendar.events', 'intent:calendar.add-event'],
    multiInstance: true,
    create: (id, services) => createTerminalApp({ id, services, vt: once }),
  };
}
