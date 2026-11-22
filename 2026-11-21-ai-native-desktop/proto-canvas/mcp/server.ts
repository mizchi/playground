// AI 向けの MCP サーバ (stdio)。ブラウザで動く shell と localhost の WebSocket でつながり、tool の呼び出しを中継する。
// 承認の tool はあえて作らない: AI は自分の提案を承認できない。承認は人が画面 (パネルの「承認して実行」) で行う。
// 使い方: node mcp/server.ts   (MCP_WS_PORT, 既定 8130)。ブラウザで http://127.0.0.1:8123/?mcp=ws://127.0.0.1:8130 を開く
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { WebSocketServer } from 'ws';
import type { WebSocket } from 'ws';
import { z } from 'zod';

const port = Number(process.env.MCP_WS_PORT ?? 8130);
const wss = new WebSocketServer({ host: '127.0.0.1', port });
let page: WebSocket | null = null;
let seq = 1;
const pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
wss.on('connection', (ws) => {
  page = ws;
  ws.on('message', (data) => {
    const msg = JSON.parse(String(data)) as { id: number; result?: unknown; error?: string };
    const p = pending.get(msg.id);
    if (!p) return;
    pending.delete(msg.id);
    if (msg.error) p.reject(new Error(msg.error));
    else p.resolve(msg.result);
  });
  ws.on('close', () => {
    if (page === ws) page = null;
  });
});

/** 画面の shell を呼ぶ */
function call(method: string, params: unknown = {}): Promise<unknown> {
  if (!page) return Promise.reject(new Error('画面 (shell) が接続されていない。ブラウザで ?mcp=ws://127.0.0.1:' + port + ' を開く'));
  const id = seq++;
  page.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    setTimeout(() => {
      if (pending.delete(id)) reject(new Error('画面が応答しない'));
    }, 10_000);
  });
}

const text = (v: unknown) => ({ content: [{ type: 'text' as const, text: typeof v === 'string' ? v : JSON.stringify(v, null, 1) }] });
const fail = (e: unknown) => ({ content: [{ type: 'text' as const, text: String((e as Error).message ?? e) }], isError: true });
const wrap = (f: () => Promise<unknown>) => f().then(text, fail);

const server = new McpServer({ name: 'ai-native-desktop', version: '0.1.0' });

server.registerTool('list_apps', { description: '表示中・ドックにある窓の一覧' }, () => wrap(() => call('listApps')));

server.registerTool(
  'get_tree',
  {
    description: '窓の意味ツリー (WAI-ARIA の role、label、状態、公開している action)。sensitive なノードの value は伏せてある',
    inputSchema: { app: z.string().optional().describe('窓の id (省略で全部)') },
  },
  ({ app }) => wrap(() => call('getTree', { app })),
);

server.registerTool('get_layout', { description: 'タイルの分割ツリー、ドック、ズーム' }, () => wrap(() => call('getLayout')));

server.registerTool(
  'propose',
  {
    description:
      '操作の提案を出す。人がプレビューを見て承認するまで何も実行されない。承認の前に状況が変わると実行されない (stale)。' +
      ' steps は {app, target, action, data?} (action は get_tree の actions にあるもの。focus は使えない) か、' +
      ' {kind: "layout", op: {type: "move"|"zoom"|"minimize"|"restore"|"equalize", ...}}',
    inputSchema: {
      intent: z.string().describe('人に見せる意図の説明'),
      steps: z.array(z.record(z.string(), z.unknown())).describe('手順'),
    },
  },
  ({ intent, steps }) => wrap(() => call('propose', { intent, steps })),
);

server.registerTool(
  'wait_proposal',
  {
    description: '提案の結果を待つ。done (実行した) / rejected (人が却下) / stale (承認の前に状況が変わった) / pending (時間切れ)',
    inputSchema: { id: z.string(), timeout_s: z.number().min(0).max(600).default(60) },
  },
  ({ id, timeout_s }) =>
    wrap(async () => {
      const until = Date.now() + timeout_s * 1000;
      for (;;) {
        const r = (await call('proposalStatus', { id })) as { status: string };
        if (r.status !== 'pending' && r.status !== 'approved') return r;
        if (Date.now() >= until) return r;
        await new Promise((ok) => setTimeout(ok, 200));
      }
    }),
);

await server.connect(new StdioServerTransport());
