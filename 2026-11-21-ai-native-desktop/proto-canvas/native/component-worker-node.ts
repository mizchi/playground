// ネイティブのホストの worker_threads の入口: コンポーネントを動かす (中身はブラウザと同じ component-worker-core.ts)。
// 生成した glue の JS は data: URL で読み込む。WASI はファイルを 1 つも開かない (preopens なし)。
import { parentPort } from 'node:worker_threads';
import * as shim from '@bytecodealliance/preview2-shim';
import type { ComponentModule } from '../src/platform/component-imports.ts';
import { serveComponent } from '../src/platform/component-worker-core.ts';
import type { FromWorker, ToWorker } from '../src/platform/component-worker-core.ts';

const handle = serveComponent(
  (m: FromWorker) => parentPort!.postMessage(m),
  async (js) => (await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`)) as ComponentModule,
  shim as unknown as Record<string, Record<string, unknown>>,
);
parentPort!.on('message', (m: ToWorker) => void handle(m));
