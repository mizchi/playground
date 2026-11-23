// ネイティブのホストの保存先: 1 つの JSON ファイル (ブラウザの localStorage の代わり)。
// - 書き込みは一時ファイルに書いてから rename する (途中で落ちても、前の中身か新しい中身のどちらかが残る)。
//   rename が済んでから手元の値を変える (失敗したら、同じ値でもう一度書ける)
// - 1 つのファイルを使うホストは 1 つだけ (<file>.lock に pid を書く)。2 つのホストがそれぞれ古い写しで上書きしないように
import { mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import type { KeyValue } from '../src/platform/services.ts';

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return (e as NodeJS.ErrnoException).code === 'EPERM';
  }
}

/** ファイルの持ち主になる。生きている別のプロセスが持っていれば断る */
function lock(path: string): void {
  const file = `${path}.lock`;
  for (let i = 0; i < 2; i++) {
    try {
      writeFileSync(file, String(process.pid), { flag: 'wx' });
      process.on('exit', () => rmSync(file, { force: true }));
      return;
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code !== 'EEXIST') throw e;
      const pid = Number(readFileSync(file, 'utf8'));
      if (pid && pid !== process.pid && alive(pid)) throw new Error(`${path} は別のホスト (pid ${pid}) が使っている。--kv で別のファイルを使う`);
      rmSync(file, { force: true }); // 前のホストが落ちて残ったもの
    }
  }
  throw new Error(`${file} を作れない`);
}

export function fileKeyValue(path: string, opts: { lock?: boolean } = {}): KeyValue {
  mkdirSync(dirname(path), { recursive: true });
  if (opts.lock !== false) lock(path);
  let data: Record<string, string> = {};
  try {
    const v: unknown = JSON.parse(readFileSync(path, 'utf8'));
    if (v && typeof v === 'object' && !Array.isArray(v)) data = Object.fromEntries(Object.entries(v).filter(([, x]) => typeof x === 'string')) as Record<string, string>;
  } catch {
    data = {};
  }
  return {
    get: (k) => (Object.hasOwn(data, k) ? data[k] : undefined),
    set: (k, v) => {
      if (Object.hasOwn(data, k) && data[k] === v) return;
      const next = { ...data, [k]: v };
      const tmp = `${path}.${process.pid}.tmp`;
      writeFileSync(tmp, JSON.stringify(next));
      renameSync(tmp, path);
      data = next;
    },
    keys: () => Object.keys(data),
  };
}
