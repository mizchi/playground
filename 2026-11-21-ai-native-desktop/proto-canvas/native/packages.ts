// ネイティブのホストでパッケージ (app.json + WIT のコンポーネント) を読む。
// ブラウザと同じ loadPackage / workerRunner を使い、Worker の代わりに worker_threads で動かす (応答しなければ止める)。
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { Worker as NodeWorker } from 'node:worker_threads';
import { loadPackage } from '../src/platform/packages.ts';
import type { ComponentRunner, PackageSource } from '../src/platform/packages.ts';
import { hostPackageTools } from '../src/platform/package-tools.ts';
import type { ServiceHost } from '../src/platform/services.ts';
import { workerRunner } from '../src/platform/worker-runner.ts';
import type { RunnerOptions } from '../src/platform/worker-runner.ts';

/** app.json のファイルと、その隣の .wasm (パスは parsePackage がパッケージの中に限る) */
export function filePackageSource(appJson: string): PackageSource {
  const dir = dirname(resolve(appJson));
  return {
    manifest: async () => JSON.parse(await readFile(appJson, 'utf8')) as unknown,
    bytes: async (p) => new Uint8Array(await readFile(join(dir, p))),
  };
}

/** worker_threads の Worker を、ブラウザの Worker と同じ口 (onmessage / onerror / postMessage / terminate) にする */
function nodeWorker(): Worker {
  const w = new NodeWorker(new URL('./component-worker-node.ts', import.meta.url));
  const o = {
    onmessage: null as ((e: { data: unknown }) => void) | null,
    onerror: null as ((e: { message: string }) => void) | null,
    postMessage: (m: unknown) => w.postMessage(m),
    terminate: () => void w.terminate(),
  };
  w.on('message', (data) => o.onmessage?.({ data }));
  w.on('error', (e) => o.onerror?.({ message: e.message }));
  return o as unknown as Worker;
}

export function nativeRunner(services: () => ServiceHost, opts: RunnerOptions = {}): ComponentRunner {
  return workerRunner(nodeWorker, () => ({ grant: (app, cap) => services().grant(app, cap) }), opts);
}

export async function readNativePackage(appJson: string, runner: ComponentRunner) {
  return loadPackage(filePackageSource(appJson), hostPackageTools, runner);
}
