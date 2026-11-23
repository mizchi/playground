// ホストの擬似端末の橋: ブラウザのターミナルアプリ (terminal.pty 能力) を、このマシンの bash につなぐ。
// node-pty (ネイティブ拡張) を使わず、Python の標準ライブラリの pty で擬似端末を作る (pty_helper.py)。
// 端末の大きさは、ブラウザの窓に合わせて TIOCSWINSZ で変える (シェルには SIGWINCH が届く)。
//   node pty/server.ts            # ws://127.0.0.1:8140/?token=... を表示する
//   http://127.0.0.1:8123/?pty=<その URL>
// 本物のシェルを開く口なので: 127.0.0.1 だけで待ち、起動ごとの token と Origin (127.0.0.1 / localhost) を確かめる。
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';

const port = Number(process.env.PTY_PORT ?? 8140);
const token = process.env.PTY_TOKEN ?? randomBytes(16).toString('hex');
const shell = (process.env.PTY_SHELL ?? 'bash --noprofile --norc -i').split(' ');
const helper = fileURLToPath(new URL('./pty_helper.py', import.meta.url));
const python = process.env.PTY_PYTHON ?? 'python3';

const wss = new WebSocketServer({
  host: '127.0.0.1',
  port,
  verifyClient: ({ origin, req }: { origin: string; req: { url?: string } }) => {
    const url = new URL(req.url ?? '/', 'ws://127.0.0.1');
    const okOrigin = !origin || /^https?:\/\/(127\.0\.0\.1|localhost)(:\d+)?$/.test(origin);
    return okOrigin && url.searchParams.get('token') === token;
  },
});
wss.on('listening', () => console.log(`pty: ws://127.0.0.1:${port}/?token=${token}`));

wss.on('connection', (ws) => {
  let child: ReturnType<typeof spawn> | null = null;
  ws.on('message', (raw) => {
    const m = JSON.parse(String(raw)) as { t: 'open'; cols: number; rows: number } | { t: 'in'; data: string } | { t: 'resize'; cols: number; rows: number };
    if (m.t === 'open' && !child) {
      const cols = Math.max(10, Math.min(500, m.cols | 0));
      const rows = Math.max(2, Math.min(200, m.rows | 0));
      // fd 3 は大きさを変える命令の口 ("resize COLS ROWS\n")
      child = spawn(python, ['-I', helper, String(cols), String(rows), '--', ...shell], {
        env: { ...process.env, TERM: 'xterm-256color', COLORTERM: 'truecolor' },
        stdio: ['pipe', 'pipe', 'pipe', 'pipe'],
      });
      const out = (d: Buffer) => ws.readyState === ws.OPEN && ws.send(JSON.stringify({ t: 'out', data: d.toString('utf8') }));
      child.stdout!.on('data', out);
      child.stderr!.on('data', out);
      child.on('exit', (code) => {
        if (ws.readyState === ws.OPEN) ws.send(JSON.stringify({ t: 'exit', code }));
        ws.close();
      });
    } else if (m.t === 'in') child?.stdin!.write(m.data);
    else if (m.t === 'resize' && child) {
      const ctl = child.stdio[3] as NodeJS.WritableStream | null;
      ctl?.write(`resize ${m.cols | 0} ${m.rows | 0}\n`);
    }
  });
  ws.on('close', () => child?.kill('SIGHUP'));
});
