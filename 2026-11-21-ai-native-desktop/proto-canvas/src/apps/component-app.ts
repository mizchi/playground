// WIT のコンポーネント (component/wit/app.wit の world gui-app) を、試作の App として扱う。
// アプリが受け取る能力は、ホスト (ここ) が import として渡したものだけ。
import type { ActionName, SemanticNode, TreeUpdate } from '../protocol.ts';
import type { App } from './app.ts';

type WitAction = 'click' | 'focus' | 'set-value' | 'replace-selected-text' | 'increment' | 'decrement';
interface WitNode {
  id: string;
  role: SemanticNode['role'];
  label?: string;
  value?: string;
  checked?: boolean;
  disabled: boolean;
  sensitive: boolean;
  actions: WitAction[];
  children: string[];
}
export interface WitTreeUpdate {
  root?: string;
  focus?: string;
  nodes: WitNode[];
  remove: string[];
}
export interface ComponentExports {
  app: { start(): void; onAction(req: { target: string; action: WitAction; data?: string; by: 'human' | 'agent' }): void };
}

const toWit = (a: ActionName): WitAction => a.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`) as WitAction;
const fromWit = (a: WitAction): ActionName => a.replace(/-([a-z])/g, (_, c: string) => c.toUpperCase()) as ActionName;

/** WIT の tree-update を P1 の TreeUpdate にする */
export function fromWitUpdate(app: string, u: WitTreeUpdate): TreeUpdate {
  return {
    t: 'tree-update',
    app,
    root: u.root,
    focus: u.focus ?? null,
    remove: u.remove,
    nodes: u.nodes.map((n) => {
      const out: SemanticNode = { id: n.id, role: n.role };
      if (n.label !== undefined) out.label = n.label;
      if (n.value !== undefined) out.value = n.value;
      if (n.checked !== undefined) out.checked = n.checked;
      if (n.disabled) out.disabled = true;
      if (n.sensitive) out.sensitive = true;
      if (n.actions.length) out.actions = n.actions.map(fromWit);
      if (n.children.length) out.children = n.children;
      return out;
    }),
  };
}

/**
 * instantiate には、host.emit を受け取ってコンポーネントを作る関数を渡す (WASI の import の方針は呼び出し側が決める)
 */
export function createComponentApp(id: string, instantiate: (host: { emit(u: WitTreeUpdate): void }) => ComponentExports): App {
  let emit: (u: TreeUpdate) => void = () => {};
  let inst: ComponentExports | null = null;
  return {
    id,
    start(e) {
      emit = e;
      inst = instantiate({ emit: (u) => emit(fromWitUpdate(id, u)) });
      inst.app.start();
    },
    onAction(req) {
      inst!.app.onAction({ target: req.target, action: toWit(req.action), data: req.data, by: req.by });
    },
  };
}

/**
 * preview2-shim の WASI 実装を、jco の import の名前に並べる。
 * preopens: 'none' ならファイルシステムを渡さない。'root' ならホストのルート (/) を開いて渡す (比較用)
 */
export function wasiImports(shim: Record<string, Record<string, unknown>>, opts: { preopens: 'none' | 'root'; version: string }): Record<string, unknown> {
  const fsMod = shim.filesystem as Record<string, unknown> & { _setPreopens(p: Record<string, string>): void };
  if (opts.preopens === 'root') fsMod._setPreopens({ '/': '/' });
  const v = opts.version;
  const cli = shim.cli as Record<string, unknown>;
  const out: Record<string, unknown> = {
    [`wasi:cli/environment@${v}`]: cli.environment,
    [`wasi:cli/exit@${v}`]: cli.exit,
    [`wasi:cli/stderr@${v}`]: cli.stderr,
    [`wasi:cli/stdin@${v}`]: cli.stdin,
    [`wasi:cli/stdout@${v}`]: cli.stdout,
    [`wasi:cli/terminal-input@${v}`]: cli.terminalInput,
    [`wasi:cli/terminal-output@${v}`]: cli.terminalOutput,
    [`wasi:cli/terminal-stderr@${v}`]: cli.terminalStderr,
    [`wasi:cli/terminal-stdin@${v}`]: cli.terminalStdin,
    [`wasi:cli/terminal-stdout@${v}`]: cli.terminalStdout,
    [`wasi:clocks/monotonic-clock@${v}`]: (shim.clocks as Record<string, unknown>).monotonicClock,
    [`wasi:clocks/wall-clock@${v}`]: (shim.clocks as Record<string, unknown>).wallClock,
    [`wasi:filesystem/types@${v}`]: (shim.filesystem as Record<string, unknown>).types,
    // 能力の要: 開いたディレクトリを 1 つも渡さなければ、ファイルには触れない
    [`wasi:filesystem/preopens@${v}`]: opts.preopens === 'none' ? { getDirectories: () => [] } : fsMod.preopens,
    [`wasi:io/error@${v}`]: (shim.io as Record<string, unknown>).error,
    [`wasi:io/poll@${v}`]: (shim.io as Record<string, unknown>).poll,
    [`wasi:io/streams@${v}`]: (shim.io as Record<string, unknown>).streams,
  };
  // jco 1.42 の生成物は、型定義 (.d.ts) では版番号付きの名前、JS では版番号の無い名前で import を読む。両方で渡す
  for (const k of Object.keys(out)) out[k.replace(/@[0-9.]+$/, '')] = out[k];
  return out;
}

/** host の import も、版番号の有無の両方の名前で渡す */
export function hostImports(host: { emit(u: WitTreeUpdate): void }, version = '0.1.0'): Record<string, unknown> {
  return { 'gui:app/host': host, [`gui:app/host@${version}`]: host, 'gui:app/types': {}, [`gui:app/types@${version}`]: {} };
}
