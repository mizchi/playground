// コンポーネントを動かす Worker の中身 (1 つの窓に 1 つ)。メインスレッドは、応答しなければこの Worker ごと止められる。
// 能力の import は同期で答える必要があるので、storage は「許可の状態とデータの写し」をここに持つ。
// 読み書きはメインスレッドにも知らせ、そちらでサービスを通す (監査ログ・許可の要求・保存は、今まで通りメインで行う)。
// ブラウザの Worker (component-worker.ts) と、ネイティブのホストの worker_threads (component-worker-node.ts) が使う。
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
/** storage の許可の状態と、アプリの保存領域の写し (許可されているときだけ中身がある)。キーは任意の文字列なので組で渡す */
export interface StorageSnapshot {
  state: GrantState | undefined;
  data: [string, string][];
}

/**
 * post: メインへ送る。loadModule: 生成した glue の JS を読み込む (ブラウザは Blob URL、Node は data: URL)。
 * wasiShim: preview2-shim (ファイルは 1 つも開かない)。返すのは、メインからの 1 通を処理する関数
 */
export function serveComponent(post: (m: FromWorker) => void, loadModule: (js: string) => Promise<ComponentModule>, wasiShim: Record<string, Record<string, unknown>>): (m: ToWorker) => Promise<void> {
  let app: { start(): void; onAction(req: unknown): void } | null = null;
  let state: GrantState | undefined;
  /** 写し (Map: "constructor" や "__proto__" のようなキーも、ただの文字列として扱う) */
  let data = new Map<string, string>();

  const backend: StorageBackend = {
    get(key) {
      post({ t: 'storage-get', key });
      if (state === 'granted') return { tag: 'ok', val: data.get(key) };
      return { tag: 'err', val: state === 'denied' ? 'denied' : 'pending' };
    },
    set(key, value) {
      post({ t: 'storage-set', key, value });
      if (state !== 'granted') return false;
      data.set(key, value);
      return true;
    },
  };

  return async (m: ToWorker) => {
    try {
      if (m.t === 'init') {
        state = m.storage.state;
        data = new Map(m.storage.data);
        const mod = await loadModule(m.glue.js);
        const exports = instantiateGlue(mod, m.glue, componentImports(m.caps, wasiShim, { emit: (u) => post({ t: 'tree', u }) }, backend));
        if (m.mode === 'probe') return post({ t: 'probe', ok: hasAppExports(exports) });
        app = (exports as { app: typeof app }).app;
        app!.start();
        post({ t: 'ready' });
      } else if (m.t === 'action') {
        app!.onAction(m.req);
        post({ t: 'done', seq: m.seq });
      } else if (m.t === 'storage') {
        state = m.storage.state;
        data = new Map(m.storage.data);
      } else if (m.t === 'storage-update') data.set(m.key, m.value);
    } catch (err) {
      post({ t: 'error', message: String((err as Error).message ?? err) });
    }
  };
}
