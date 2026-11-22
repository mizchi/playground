// ブラウザ側: pty/server.ts (ホストの擬似端末の橋) につなぐ PtyConnector。?pty=ws://127.0.0.1:8140/?token=... のときだけ使う。
import type { PtyConnector } from '../../platform/types.ts';

export function wsPtyConnector(url: string): PtyConnector {
  return (cols, rows, onData, onExit) => {
    const ws = new WebSocket(url);
    const queue: string[] = [];
    let open = false;
    let ended = false;
    const end = (why: string) => {
      if (ended) return;
      ended = true;
      onExit(why);
    };
    ws.onopen = () => {
      open = true;
      ws.send(JSON.stringify({ t: 'open', cols, rows }));
      for (const d of queue.splice(0)) ws.send(JSON.stringify({ t: 'in', data: d }));
    };
    ws.onmessage = (e) => {
      const m = JSON.parse(String(e.data)) as { t: 'out'; data: string } | { t: 'exit'; code: number };
      if (m.t === 'out') onData(m.data);
      else end(`終了コード ${m.code}`);
    };
    ws.onclose = () => end(open ? '接続が切れた' : 'つなげなかった');
    return {
      write: (d) => (open ? ws.send(JSON.stringify({ t: 'in', data: d })) : queue.push(d)),
      close: () => ws.close(),
    };
  };
}
