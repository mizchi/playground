// ネイティブのホストの擬似端末: ブラウザでは WebSocket の橋 (pty/server.ts) を通すが、ここでは同じ helper を直接起こす。
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import type { PtyConnector } from '../src/platform/types.ts';

const helper = fileURLToPath(new URL('../pty/pty_helper.py', import.meta.url));

export function directPtyConnector(shell = (process.env.PTY_SHELL ?? 'bash --noprofile --norc -i').split(' '), python = process.env.PTY_PYTHON ?? 'python3'): PtyConnector {
  return (cols, rows, onData, onExit) => {
    const child = spawn(python, ['-I', helper, String(Math.max(10, Math.min(500, cols | 0))), String(Math.max(2, Math.min(200, rows | 0))), '--', ...shell], {
      env: { ...process.env, TERM: 'xterm-256color', COLORTERM: 'truecolor' },
      stdio: ['pipe', 'pipe', 'pipe', 'pipe'],
    });
    let ended = false;
    const end = (why: string) => {
      if (!ended) {
        ended = true;
        onExit(why);
      }
    };
    child.stdout!.on('data', (d: Buffer) => onData(d.toString('utf8')));
    child.stderr!.on('data', (d: Buffer) => onData(d.toString('utf8')));
    child.on('exit', (code) => end(`終了コード ${code}`));
    child.on('error', (e) => end(String(e.message)));
    // helper が先に pipe を閉じたときの書き込みの失敗で、ホストごと落ちないように
    const ctl = child.stdio[3] as (NodeJS.WritableStream & { writable?: boolean }) | null;
    child.stdin!.on('error', () => end('入力の pipe が閉じた'));
    ctl?.on('error', () => {});
    return {
      write: (d) => {
        if (!ended && child.stdin!.writable) child.stdin!.write(d);
      },
      resize: (c, r) => {
        if (!ended && ctl?.writable !== false) ctl?.write(`resize ${c | 0} ${r | 0}\n`);
      },
      close: () => {
        end('閉じた');
        child.kill('SIGHUP');
      },
    };
  };
}
