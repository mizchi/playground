// コンポーネントを動かす Worker (1 つの窓に 1 つ)。メインスレッドは、応答しなければこの Worker ごと止められる。
// 能力の import は同期で答える必要があるので、storage は「許可の状態とデータの写し」をここに持つ。
// 読み書きはメインスレッドにも知らせ、そちらでサービスを通す (監査ログ・許可の要求・保存は、今まで通りメインで行う)。
import * as shim from '@bytecodealliance/preview2-shim';
import type { WitTreeUpdate } from '../apps/component-app.ts';
import { componentImports, hasAppExports, instantiateGlue } from './component-imports.ts';
import type { ComponentModule, Glue, StorageBackend } from './component-imports.ts';
import type { Capability, GrantState } from './types.ts';

export type ToWorker =
  | { t: 'init'; mode: 'probe' | 'run'; glue: Glue; caps: Capability[]; storage: StorageSnapshot }
  | { t: 'action'; seq: number; req: { target: string; action: string; data?: string; by: 'human' | 'agent' } }
  | { t: 'storage'; storage: StorageSnapshot }
  | { t: 'storage-update'; key: string; value: string };
export type FromWorker =
  | { t: 'probe'; ok: boolean }
  | { t: 'tree'; u: WitTreeUpdate }
  | { t: 'ready' }
  | { t: 'done'; seq: number }
  | { t: 'storage-get'; key: string }
  | { t: 'storage-set'; key: string; value: string }
  | { t: 'error'; message: string };
/** storage の許可の状態と、アプリの保存領域の写し (許可されているときだけ中身がある) */
export interface StorageSnapshot {
  state: GrantState | undefined;
  data: Record<string, string>;
}

const post = (m: FromWorker) => (self as unknown as Worker).postMessage(m);
let app: { start(): void; onAction(req: unknown): void } | null = null;
let storage: StorageSnapshot = { state: undefined, data: {} };

const backend: StorageBackend = {
  get(key) {
    post({ t: 'storage-get', key });
    if (storage.state === 'granted') return { tag: 'ok', val: storage.data[key] };
    return { tag: 'err', val: storage.state === 'denied' ? 'denied' : 'pending' };
  },
  set(key, value) {
    post({ t: 'storage-set', key, value });
    if (storage.state !== 'granted') return false;
    storage.data[key] = value;
    return true;
  },
};

self.onmessage = async (e: MessageEvent<ToWorker>) => {
  const m = e.data;
  try {
    if (m.t === 'init') {
      storage = m.storage;
      const url = URL.createObjectURL(new Blob([m.glue.js], { type: 'text/javascript' }));
      const mod = (await import(/* @vite-ignore */ url)) as ComponentModule;
      URL.revokeObjectURL(url);
      const exports = instantiateGlue(mod, m.glue, componentImports(m.caps, shim as unknown as Record<string, Record<string, unknown>>, { emit: (u) => post({ t: 'tree', u }) }, backend));
      if (m.mode === 'probe') return post({ t: 'probe', ok: hasAppExports(exports) });
      app = (exports as { app: typeof app }).app;
      app!.start();
      post({ t: 'ready' });
    } else if (m.t === 'action') {
      app!.onAction(m.req);
      post({ t: 'done', seq: m.seq });
    } else if (m.t === 'storage') storage = m.storage;
    else if (m.t === 'storage-update') storage.data[m.key] = m.value;
  } catch (err) {
    post({ t: 'error', message: String((err as Error).message ?? err) });
  }
};
