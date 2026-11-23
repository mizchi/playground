// コンポーネントを Worker で動かす (ブラウザ)。メインスレッドは応答を待つだけで、期限までに応答しなければ Worker ごと止める。
// 止まったコンポーネントの窓は「応答しない」と出して、再起動できる (状態は保存領域から読み直す)。ほかの窓は固まらない。
import type { App } from '../apps/app.ts';
import { fromWitUpdate, toWit } from '../apps/component-app.ts';
import type { NodeId, TreeUpdate } from '../protocol.ts';
import type { FromWorker, StorageSnapshot, ToWorker } from './component-worker-core.ts';
import type { Glue } from './component-imports.ts';
import type { ComponentRunner, PackageManifest } from './packages.ts';
import type { AppServices, Capability, GrantState } from './types.ts';

/** Worker に渡す storage の写しを作るための、ホスト側の口 (許可の状態は人だけが変える) */
export interface RunnerHost {
  grant(app: string, cap: Capability): GrantState | undefined;
}

export interface RunnerOptions {
  /** インスタンスを作って start が返るまでの期限 */
  startMs?: number;
  /** on-action が返るまでの期限 */
  actionMs?: number;
}

export function workerRunner(spawn: () => Worker, host: () => RunnerHost, opts: RunnerOptions = {}): ComponentRunner {
  const startMs = opts.startMs ?? 5000;
  const actionMs = opts.actionMs ?? 2000;
  /** 動いている Worker (アプリ id → 窓 id → Worker)。保存領域の書き込みを、同じアプリのほかの窓に知らせる */
  const live = new Map<string, Map<string, Worker>>();
  /** 写しは窓のサービスから作る (agent のワークスペースの窓なら、重ねた層を含む) */
  const snapshot = (pkg: PackageManifest, services: AppServices): StorageSnapshot => {
    if (!pkg.capabilities.includes('storage')) return { state: undefined, data: [] };
    const state = host().grant(pkg.id, 'storage');
    const r = state === 'granted' ? services.storage.entries() : null;
    return { state, data: r?.ok ? r.value : [] };
  };
  const send = (w: Worker, m: ToWorker) => w.postMessage(m);

  return {
    probe(glue, pkg) {
      return new Promise((resolve) => {
        const w = spawn();
        const timer = setTimeout(() => {
          w.terminate();
          resolve(`インスタンスを作るのが ${startMs / 1000} 秒で終わらない (止めた)`);
        }, startMs);
        w.onmessage = (e: MessageEvent<FromWorker>) => {
          if (e.data.t !== 'probe' && e.data.t !== 'error') return;
          clearTimeout(timer);
          w.terminate();
          resolve(e.data.t === 'error' ? `インスタンスを作れない: ${e.data.message}` : e.data.ok ? null : 'gui:app/app (start と on-action) を export していない');
        };
        w.onerror = (e) => {
          clearTimeout(timer);
          w.terminate();
          resolve(`インスタンスを作れない: ${e.message}`);
        };
        send(w, { t: 'init', mode: 'probe', glue, caps: pkg.capabilities, storage: { state: 'denied', data: [] } });
      });
    },

    create(glue, pkg, windowId, services: AppServices): App {
      let emit: (u: TreeUpdate) => void = () => {};
      let worker: Worker | null = null;
      let seq = 0;
      const timers = new Map<number | 'start', ReturnType<typeof setTimeout>>();
      /** コンポーネントが出したノード (止めたときに消す) */
      const sent = new Set<NodeId>();
      let pendingRemove: NodeId[] = [];
      let stopped = false;

      const deadline = (key: number | 'start', ms: number, what: string) =>
        timers.set(key, setTimeout(() => kill(`${what}が ${ms / 1000} 秒で返らなかったので、コンポーネントを止めた`), ms));
      const clear = (key: number | 'start') => {
        clearTimeout(timers.get(key));
        timers.delete(key);
      };
      const detach = () => {
        for (const t of timers.values()) clearTimeout(t);
        timers.clear();
        worker?.terminate();
        live.get(pkg.id)?.delete(windowId);
        worker = null;
      };

      /** 止めた: 窓には「応答しない」と再起動のボタンだけを出す */
      const kill = (reason: string) => {
        if (stopped) return;
        stopped = true;
        detach();
        const remove = [...sent].filter((id) => id !== 'root');
        sent.clear();
        pendingRemove = ['x-stopped', 'x-restart'];
        emit({
          t: 'tree-update',
          app: windowId,
          root: 'root',
          focus: null,
          remove,
          nodes: [
            { id: 'root', role: 'x-window', label: `${pkg.title} (応答しない)`, children: ['x-stopped', 'x-restart'] },
            { id: 'x-stopped', role: 'paragraph', label: reason },
            { id: 'x-restart', role: 'button', label: '再起動', actions: ['click', 'focus'] },
          ],
        });
      };

      const onMessage = (e: MessageEvent<FromWorker>) => {
        const m = e.data;
        switch (m.t) {
          case 'tree': {
            const u = fromWitUpdate(windowId, m.u);
            for (const n of u.nodes) sent.add(n.id);
            for (const id of u.remove ?? []) sent.delete(id);
            // 再起動の直後は、止めたときの表示を消す
            if (pendingRemove.length) {
              u.remove = [...(u.remove ?? []), ...pendingRemove.filter((id) => !u.nodes.some((n) => n.id === id))];
              pendingRemove = [];
            }
            emit(u);
            break;
          }
          case 'ready':
            clear('start');
            break;
          case 'done':
            clear(m.seq);
            break;
          case 'storage-get':
            services.storage.get(m.key); // 監査ログと、許可の要求 (ask のとき) のため。値は Worker が写しから答えている
            break;
          case 'storage-set': {
            const r = services.storage.set(m.key, m.value);
            if (r.ok) for (const [id, w] of live.get(pkg.id) ?? []) if (id !== windowId) send(w, { t: 'storage-update', key: m.key, value: m.value });
            break;
          }
          case 'error':
            kill(`コンポーネントが失敗した: ${m.message}`);
            break;
        }
      };

      const boot = () => {
        stopped = false;
        const w = spawn();
        worker = w;
        if (!live.has(pkg.id)) live.set(pkg.id, new Map());
        live.get(pkg.id)!.set(windowId, w);
        w.onmessage = onMessage;
        w.onerror = (e) => kill(`コンポーネントが失敗した: ${e.message}`);
        deadline('start', startMs, '起動 (start)');
        send(w, { t: 'init', mode: 'run', glue, caps: pkg.capabilities, storage: snapshot(pkg, services) });
      };

      return {
        id: windowId,
        start(e) {
          emit = e;
          // 人が許可を変えたら、Worker の写しも変える
          services.onGrant((cap) => {
            if (cap === 'storage' && worker) send(worker, { t: 'storage', storage: snapshot(pkg, services) });
          });
          // ほかから保存領域が変わったら (持ち込みなど)、Worker を起こし直して読み直させる (コンポーネントは start で読む)
          services.storage.onChange(() => {
            if (stopped || !worker) return;
            detach();
            pendingRemove = [...sent];
            sent.clear();
            boot();
          });
          // shell は窓を作った直後に描くので、根だけ先に同期で出す (Worker の最初の更新で置き換わる)
          emit({ t: 'tree-update', app: windowId, root: 'root', focus: null, nodes: [{ id: 'root', role: 'x-window', label: `${pkg.title} (起動中)`, children: [] }] });
          // Worker は少し後で起こす (セッションの復元では、プラットフォームを組み立てている途中で start が呼ばれる)
          queueMicrotask(boot);
        },
        onAction(req) {
          if (stopped) {
            if (req.target === 'x-restart' && req.action === 'click') boot();
            return;
          }
          if (!worker) return;
          const n = ++seq;
          deadline(n, actionMs, `操作 (${req.target} ${req.action}) `);
          send(worker, { t: 'action', seq: n, req: { target: req.target, action: toWit(req.action), data: req.data, by: req.by } });
        },
        stop() {
          stopped = true;
          detach();
        },
      };
    },
  };
}
