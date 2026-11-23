// ネイティブのホストの保存先: 1 つの JSON ファイル (ブラウザの localStorage の代わり)。
// 書き込みは一時ファイルに書いてから rename する (途中で落ちても、前の中身か新しい中身のどちらかが残る)。
import { mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import type { KeyValue } from '../src/platform/services.ts';

export function fileKeyValue(path: string): KeyValue {
  let data: Record<string, string> = {};
  try {
    const v: unknown = JSON.parse(readFileSync(path, 'utf8'));
    if (v && typeof v === 'object' && !Array.isArray(v)) data = Object.fromEntries(Object.entries(v).filter(([, x]) => typeof x === 'string')) as Record<string, string>;
  } catch {
    data = {};
  }
  mkdirSync(dirname(path), { recursive: true });
  return {
    get: (k) => (Object.hasOwn(data, k) ? data[k] : undefined),
    set: (k, v) => {
      if (data[k] === v) return;
      data[k] = v;
      const tmp = `${path}.${process.pid}.tmp`;
      writeFileSync(tmp, JSON.stringify(data));
      renameSync(tmp, path);
    },
    keys: () => Object.keys(data),
  };
}
