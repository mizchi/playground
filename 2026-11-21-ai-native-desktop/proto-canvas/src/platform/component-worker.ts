// ブラウザの Worker の入口: コンポーネントを動かす (中身は component-worker-core.ts)。
// 生成した glue の JS は Blob URL で読み込む。
import * as shim from '@bytecodealliance/preview2-shim';
import { serveComponent } from './component-worker-core.ts';
import type { ComponentModule } from './component-imports.ts';
import type { FromWorker, ToWorker } from './component-worker-core.ts';

export type { FromWorker, StorageSnapshot, ToWorker } from './component-worker-core.ts';

const handle = serveComponent(
  (m: FromWorker) => (self as unknown as Worker).postMessage(m),
  async (js) => {
    const url = URL.createObjectURL(new Blob([js], { type: 'text/javascript' }));
    try {
      return (await import(/* @vite-ignore */ url)) as ComponentModule;
    } finally {
      URL.revokeObjectURL(url);
    }
  },
  shim as unknown as Record<string, Record<string, unknown>>,
);
self.onmessage = (e: MessageEvent<ToWorker>) => void handle(e.data);
