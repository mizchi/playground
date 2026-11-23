// MCP サーバの通し確認: MCP クライアント (公式 SDK) → mcp/server.ts → ブラウザの shell。承認は人 (ここでは Playwright) が画面で行う。
// 使い方: pnpm build && node e2e/mcp.ts
import assert from 'node:assert/strict';
import fs from 'node:fs';
import http from 'node:http';
import { createRequire } from 'node:module';
import path from 'node:path';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

const { chromium } = createRequire(import.meta.url)('playwright') as typeof import('playwright');
const root = path.join(import.meta.dirname, '..', 'www');
const server = http.createServer((req, res) => {
  const file = path.join(root, !req.url || req.url === '/' || req.url.startsWith('/?') ? 'index.html' : req.url.split('?')[0]);
  if (!fs.existsSync(file)) return res.writeHead(404).end();
  res.writeHead(200, { 'content-type': file.endsWith('.js') ? 'text/javascript' : 'text/html' }).end(fs.readFileSync(file));
});
await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
const wsPort = 8130 + Math.floor(Math.random() * 500);

let failures = 0;
async function step(name: string, fn: () => Promise<void>) {
  try {
    await fn();
    console.log(`ok   ${name}`);
  } catch (e) {
    failures++;
    console.log(`FAIL ${name}\n     ${String((e as Error).message).split('\n').join('\n     ')}`);
  }
}

const client = new Client({ name: 'e2e', version: '0.0.1' });
await client.connect(new StdioClientTransport({ command: process.execPath, args: [path.join(import.meta.dirname, '..', 'mcp', 'server.ts')], env: { ...process.env, MCP_WS_PORT: String(wsPort) } as Record<string, string> }));
const call = async (name: string, args: Record<string, unknown> = {}) => {
  const r = (await client.callTool({ name, arguments: args })) as { content: { text: string }[]; isError?: boolean };
  const t = r.content[0].text;
  let v: unknown = t;
  try {
    v = JSON.parse(t);
  } catch {
    /* 文字列のまま */
  }
  return { isError: !!r.isError, value: v as Record<string, unknown> & unknown[], text: t };
};

await step('画面がつながる前は、理由付きのエラーを返す', async () => {
  const r = await call('list_apps');
  assert.equal(r.isError, true);
  assert.match(r.text, /接続されていない/);
});

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1680, height: 900 } });
await page.goto(`http://127.0.0.1:${(server.address() as { port: number }).port}/?mcp=ws://127.0.0.1:${wsPort}`);
await page.evaluate(() => localStorage.clear());
await page.waitForFunction(() => (window as unknown as { mcpStatus?: string }).mcpStatus === 'MCP: 接続中');

await step('tool の一覧: 読む・提案する・待つだけ。承認の tool は無い', async () => {
  const names = (await client.listTools()).tools.map((t) => t.name).sort();
  assert.deepEqual(names, ['get_layout', 'get_tree', 'list_apps', 'list_installed', 'propose', 'wait_proposal']);
});

await step('get_tree / get_layout / list_apps で画面の状態が読める', async () => {
  const tree = await call('get_tree', { app: 'todo' });
  assert.ok(tree.text.includes('牛乳を買う'));
  const layout = await call('get_layout');
  assert.equal((layout.value as { tiles: { kind: string } }).tiles.kind, 'split');
  const apps = await call('list_apps');
  assert.equal((apps.value as { shown: unknown[] }).shown.length, 3);
});

await step('propose → 人が画面で承認 → wait_proposal が done を返し、状態が変わる', async () => {
  const p = await call('propose', { intent: '牛乳を完了にする', steps: [{ app: 'todo', target: 'i1', action: 'click' }] });
  assert.equal(p.isError, false, p.text);
  const id = (p.value as { id: string }).id;
  const early = await call('wait_proposal', { id, timeout_s: 0.3 });
  assert.equal((early.value as { status: string }).status, 'pending', '承認までは pending');
  await page.click('#proposals button[data-op="approve"]'); // 人の承認
  const done = await call('wait_proposal', { id, timeout_s: 5 });
  assert.equal((done.value as { status: string }).status, 'done');
  const tree = await call('get_tree', { app: 'todo' });
  const i1 = ((tree.value as unknown as { nodes: { id: string; checked?: boolean }[] }[])[0].nodes).find((n) => n.id === 'i1')!;
  assert.equal(i1.checked, true);
});

await step('人が却下すると rejected', async () => {
  const p = await call('propose', { intent: 'コンパクト表示', steps: [{ app: 'settings', target: 'compact', action: 'click' }] });
  const id = (p.value as { id: string }).id;
  await page.click('#proposals button[data-op="reject"]');
  const r = await call('wait_proposal', { id, timeout_s: 5 });
  assert.equal((r.value as { status: string }).status, 'rejected');
});

await step('公開されていない action や focus の提案は、理由付きで拒否される', async () => {
  const r = await call('propose', { intent: 'x', steps: [{ app: 'todo', target: 'status', action: 'click' }, { app: 'todo', target: 'input', action: 'focus' }] });
  assert.equal(r.isError, true);
  assert.match(r.text, /公開していない/);
  assert.match(r.text, /focus を使わない/);
});

await step('レイアウトの提案も同じ流れ (承認で実行)', async () => {
  const p = await call('propose', { intent: 'Settings を最小化', steps: [{ kind: 'layout', op: { type: 'minimize', app: 'settings' } }] });
  assert.equal(p.isError, false, p.text);
  await page.click('#proposals button[data-op="approve"]');
  const r = await call('wait_proposal', { id: (p.value as { id: string }).id, timeout_s: 5 });
  assert.equal((r.value as { status: string }).status, 'done');
  const apps = await call('list_apps');
  assert.deepEqual((apps.value as { minimized: { app: string }[] }).minimized.map((m) => m.app), ['settings']);
});

await step('list_installed で起動できるアプリを知り、launch の提案で Terminal を開く (承認で実行)', async () => {
  const installed = (await call('list_installed')).value as { id: string }[];
  assert.ok(installed.some((a) => a.id === 'terminal'));
  assert.ok(!installed.some((a) => a.id === 'launcher'), 'システムのアプリは出さない');
  const p = await call('propose', { intent: 'Terminal を開く', steps: [{ kind: 'layout', op: { type: 'launch', app: 'terminal' } }] });
  assert.equal(p.isError, false, p.text);
  await page.click(`#proposals button[data-op="approve"][data-id="${(p.value as { id: string }).id}"]`);
  const r = await call('wait_proposal', { id: (p.value as { id: string }).id, timeout_s: 5 });
  assert.equal((r.value as { status: string }).status, 'done');
  await page.waitForFunction(() => /libghostty-vt/.test((window as any).shell.tree('terminal').get('screen')?.value ?? ''));
  const tree = await call('get_tree', { app: 'terminal' });
  assert.ok(tree.text.includes('libghostty-vt'), 'agent は画面の文字を読める');
  assert.ok(!tree.text.includes('"lines"'), '描画用の格子は渡さない');
});

await client.close();
await browser.close();
server.close();
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
