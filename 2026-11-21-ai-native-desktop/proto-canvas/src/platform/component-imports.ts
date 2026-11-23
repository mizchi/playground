// コンポーネントに渡す import を作る (メインスレッドでも Worker でも使う)。DOM にもサービスの実装にも依存しない。
import { hostImports, wasiImports } from '../apps/component-app.ts';
import type { WitTreeUpdate } from '../apps/component-app.ts';
import type { Capability } from './types.ts';

/** 能力 storage の口 (WIT: get は result<option<string>, string>、set は bool) */
export interface StorageBackend {
  get(key: string): { tag: 'ok'; val: string | undefined } | { tag: 'err'; val: string };
  set(key: string, value: string): boolean;
}

/** ホストが生成した glue (jco の --instantiation sync の形) */
export interface ComponentModule {
  instantiate(getCoreModule: (path: string) => WebAssembly.Module, imports: Record<string, unknown>): unknown;
}

/** glue: 生成した JS と、compile した core wasm (WebAssembly.Module は Worker へ postMessage で渡せる) */
export interface Glue {
  js: string;
  cores: [string, WebAssembly.Module][];
}

/** 宣言した能力の import だけを渡す (WASI はファイルを 1 つも開いていない) */
export function componentImports(
  capabilities: Capability[],
  wasiShim: Record<string, Record<string, unknown>>,
  host: { emit(u: WitTreeUpdate): void },
  storage: StorageBackend,
): Record<string, unknown> {
  const imports: Record<string, unknown> = { ...wasiImports(wasiShim, { preopens: 'none', version: '0.2.9' }), ...hostImports(host) };
  if (capabilities.includes('storage')) {
    imports['gui:app/storage'] = storage;
    imports['gui:app/storage@0.1.0'] = storage;
  }
  return imports;
}

/** glue を読み込んで、インスタンスを作る */
export function instantiateGlue(mod: ComponentModule, glue: Glue, imports: Record<string, unknown>): unknown {
  const cores = new Map(glue.cores);
  const getCore = (p: string) => {
    // 同じ名前のファイルが別のディレクトリにあっても取り違えないよう、パスが一致するものを先に探す
    const m = cores.get(p) ?? cores.get(p.split('/').pop()!);
    if (!m) throw new Error(`core wasm ${p} が生成されていない`);
    return m;
  };
  return mod.instantiate(getCore, imports);
}

/** world gui-app の app (start / on-action) を export しているか */
export function hasAppExports(exports: unknown): boolean {
  const app = (exports as { app?: { start?: unknown; onAction?: unknown } } | null)?.app;
  return typeof app?.start === 'function' && typeof app?.onAction === 'function';
}
