// ブラウザ側: MCP サーバ (mcp/server.ts) からの呼び出しを shell につなぐ。?mcp=ws://127.0.0.1:8130 のときだけ使う。
// 承認の口は無い。AI ができるのは、読むことと提案することだけ。
import type { ActionName } from './protocol.ts';
import type { Shell, Step } from './shell.ts';

/** agent のワークスペースの口 (ここでは提案なしで動かせる。持ち込みは提案で、人が答える) */
export interface AgentWorkspaceApi {
  getTree(app?: string): unknown;
  windows(): unknown;
  launch(appId: string): string;
  act(app: string, target: string, action: ActionName, data?: string): string;
  proposeMerge(intent: string): { ok: true; merge: { id: string; status: string; changes: unknown[] } } | { ok: false; reason: string };
  merge(id: string): { id: string; status: string; changes: unknown[] } | undefined;
}

export function connectMcp(url: string, shell: Shell, onStatus: (s: string) => void, installed: () => unknown[] = () => [], workspace?: AgentWorkspaceApi): void {
  let ws: WebSocket;
  const open = () => {
    ws = new WebSocket(url);
    ws.onopen = () => onStatus('MCP: 接続中');
    ws.onclose = () => {
      onStatus('MCP: 切断 (再接続を待つ)');
      setTimeout(open, 1000);
    };
    ws.onmessage = (e) => {
      const { id, method, params } = JSON.parse(String(e.data)) as { id: number; method: string; params: Record<string, unknown> };
      try {
        ws.send(JSON.stringify({ id, result: handle(method, params) }));
      } catch (err) {
        ws.send(JSON.stringify({ id, error: String((err as Error).message ?? err) }));
      }
    };
  };
  const handle = (method: string, p: Record<string, unknown>): unknown => {
    switch (method) {
      case 'listApps': {
        const a = shell.accessibility();
        return { shown: a.windows.map((w) => ({ app: w.app, title: w.title, focused: w.focused })), minimized: a.minimized.map((m) => ({ app: m.app, title: m.title })) };
      }
      case 'listInstalled':
        return installed();
      case 'workspaceTree':
        if (!workspace) throw new Error('agent のワークスペースが無い');
        return { windows: workspace.windows(), trees: workspace.getTree(p.app as string | undefined) };
      case 'workspaceLaunch': {
        if (!workspace) throw new Error('agent のワークスペースが無い');
        const why = workspace.launch(String(p.app));
        if (why) throw new Error(why);
        return { windows: workspace.windows() };
      }
      case 'workspaceAct': {
        if (!workspace) throw new Error('agent のワークスペースが無い');
        const why = workspace.act(String(p.app), String(p.target), String(p.action) as ActionName, p.data === undefined ? undefined : String(p.data));
        if (why) throw new Error(`できない: ${why}`);
        return { ok: true };
      }
      case 'proposeMerge': {
        if (!workspace) throw new Error('agent のワークスペースが無い');
        const r = workspace.proposeMerge(String(p.intent ?? ''));
        if (!r.ok) throw new Error(r.reason);
        return { id: r.merge.id, status: r.merge.status, changes: r.merge.changes, note: '人がパネルで答える。wait_merge で結果を待つ' };
      }
      case 'mergeStatus': {
        const m = workspace?.merge(String(p.id));
        if (!m) throw new Error(`持ち込みの提案 ${String(p.id)} がない`);
        return { id: m.id, status: m.status };
      }
      case 'getTree':
        return shell.getTree(p.app as string | undefined);
      case 'getLayout':
        return shell.getLayout();
      case 'propose': {
        const r = shell.propose(String(p.intent ?? ''), (p.steps ?? []) as Step[]);
        if (!r.ok) throw new Error(`提案できない: ${r.errors.join(' / ')}`);
        return { id: r.proposal.id, status: r.proposal.status, sensitive: r.proposal.sensitive, note: r.proposal.status === 'pending' ? '人の承認を待っている。wait_proposal で結果を待つ' : undefined };
      }
      case 'proposalStatus': {
        const pr = shell.proposals.find((x) => x.id === p.id);
        if (!pr) throw new Error(`提案 ${String(p.id)} がない`);
        const log = shell.audit.filter((a) => a.proposal === pr.id).map((a) => ({ app: a.app, target: a.target, action: a.action, result: a.result, reason: a.reason }));
        return { id: pr.id, status: pr.status, log };
      }
      default:
        throw new Error(`未知の呼び出し ${method}`);
    }
  };
  open();
}
