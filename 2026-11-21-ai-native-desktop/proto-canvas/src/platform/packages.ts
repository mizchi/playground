// アプリのパッケージ: app.json (manifest) + WIT のコンポーネント (.wasm)。パッケージは JS を持たない。
// glue (コンポーネントを JS から呼ぶためのコード) は、ホストが .wasm から生成する (jco の generate をホストで動かす)。
// 能力 = import。ホストは、宣言して許可された能力の import だけを渡す。宣言していない import を持つコンポーネントは入れない。
// ブラウザでは URL から、テストではファイルから読む (読み方は PackageSource、glue の生成と読み込みは PackageTools で差し替える)。
import { createComponentApp, hostImports, wasiImports } from '../apps/component-app.ts';
import type { ComponentExports, WitTreeUpdate } from '../apps/component-app.ts';
import type { AppManifest, AppServices, Capability } from './types.ts';

/** app.json の形 */
export interface PackageManifest {
  id: string;
  title: string;
  description: string;
  kind: 'component';
  capabilities: Capability[];
  multiInstance: boolean;
  /** WIT のコンポーネント (.wasm)。app.json からの相対パス */
  component: string;
}

/** ホストが生成した glue (jco の --instantiation sync の形) */
export interface ComponentModule {
  instantiate(getCoreModule: (path: string) => WebAssembly.Module, imports: Record<string, unknown>): unknown;
}

export interface PackageSource {
  /** app.json の中身 */
  manifest(): Promise<unknown>;
  /** パッケージの中のファイル (.wasm) のバイト列 */
  bytes(path: string): Promise<Uint8Array>;
}

/** glue を作って読み込む道具 (ホストのもの。パッケージからは来ない) */
export interface PackageTools {
  /** jco の generate: コンポーネントから glue の JS と core wasm を作り、import の一覧を返す */
  generate(component: Uint8Array): Promise<{ files: [string, Uint8Array][]; imports: string[] }>;
  /** 生成した JS を ES module として読み込む */
  loadModule(js: string): Promise<ComponentModule>;
}

/** 能力 → それが渡す import の名前 (WIT の interface)。ここに無い能力は、コンポーネントには渡せない */
const CAPABILITY_IMPORTS: Partial<Record<Capability, string[]>> = {
  storage: ['gui:app/storage'],
};
/** どのコンポーネントにも渡す import: host (意味ツリーを出す) と、何もできない WASI (ファイルは開いていない) */
const BASE_IMPORTS = /^(gui:app\/(host|types)|wasi:(cli|clocks|filesystem|io)\/[a-z-]+)(@[0-9.]+)?$/;

const ID = /^[a-z][a-z0-9-]*$/;
const KNOWN_CAPS: Capability[] = ['storage', 'clipboard.read', 'clipboard.write', 'notifications', 'apps.launch', 'terminal.pty'];

/** app.json の検査 (形・値の範囲。パスはパッケージの中だけ) */
export function parsePackage(v: unknown): { ok: true; value: PackageManifest } | { ok: false; errors: string[] } {
  const errors: string[] = [];
  if (typeof v !== 'object' || v === null || Array.isArray(v)) return { ok: false, errors: ['app.json は object'] };
  const o = v as Record<string, unknown>;
  const str = (k: string) => typeof o[k] === 'string' && (o[k] as string).length > 0 && (o[k] as string).length <= 200;
  if (!str('id') || !ID.test(o.id as string)) errors.push('id は英小文字・数字・- (先頭は英字)');
  for (const k of ['title', 'description', 'component']) if (!str(k)) errors.push(`${k} は 1..200 文字の文字列`);
  if (o.kind !== 'component') errors.push('kind は component');
  if (typeof o.multiInstance !== 'boolean') errors.push('multiInstance は boolean');
  const caps = o.capabilities;
  if (!Array.isArray(caps) || !caps.every((c) => typeof c === 'string' && (KNOWN_CAPS.includes(c as Capability) || /^data:[a-z0-9.-]+$/.test(c))))
    errors.push(`capabilities は ${KNOWN_CAPS.join(' | ')} | data:<topic> の配列`);
  // パッケージの外を読ませない
  if (typeof o.component === 'string' && (!/^[\w.-]+(\/[\w.-]+)*\.wasm$/.test(o.component) || o.component.split('/').includes('..')))
    errors.push(`${o.component}: パッケージの中の .wasm の相対パスだけ`);
  if (errors.length) return { ok: false, errors };
  return { ok: true, value: o as unknown as PackageManifest };
}

/** WASI の実装 (preview2-shim) を受け取り、能力に応じた import を作る */
export function importsFor(pkg: PackageManifest, wasiShim: Record<string, Record<string, unknown>>, host: { emit(u: WitTreeUpdate): void }, services: AppServices): Record<string, unknown> {
  const imports: Record<string, unknown> = { ...wasiImports(wasiShim, { preopens: 'none', version: '0.2.9' }), ...hostImports(host) };
  if (pkg.capabilities.includes('storage')) {
    const storage = {
      // WIT: result<option<string>, string>。読めないとき (拒否・許可待ち) は err にして、キーが無いだけ (ok(none)) と分ける
      get: (key: string) => {
        const r = services.storage.get(key);
        return r.ok ? { tag: 'ok', val: r.value } : { tag: 'err', val: r.reason };
      },
      set: (key: string, value: string) => services.storage.set(key, value).ok,
    };
    imports['gui:app/storage'] = storage;
    imports['gui:app/storage@0.1.0'] = storage;
  }
  return imports;
}

function missingReason(name: string): string {
  const base = name.replace(/@[0-9.]+$/, '');
  const cap = (Object.entries(CAPABILITY_IMPORTS) as [Capability, string[]][]).find(([, names]) => names.includes(base))?.[0];
  return cap ? `${base} を import している (能力 ${cap} を宣言していない)` : `${base} を import している (ホストが渡さない import)`;
}

/**
 * パッケージを読み、AppManifest にする。
 * 1. ホストが .wasm から glue を作る (パッケージの JS は動かさない)
 * 2. コンポーネントの import の一覧 (glue の生成で分かる) を、宣言した能力と照らし合わせる
 * 3. 空のサービスで一度インスタンスを作り、world gui-app の app を export しているかを確かめる
 */
export async function loadPackage(
  src: PackageSource,
  wasiShim: Record<string, Record<string, unknown>>,
  tools: PackageTools,
): Promise<{ ok: true; manifest: AppManifest; pkg: PackageManifest } | { ok: false; errors: string[] }> {
  let raw: unknown;
  try {
    raw = await src.manifest();
  } catch (e) {
    return { ok: false, errors: [`app.json を読めない: ${String((e as Error).message ?? e)}`] };
  }
  const parsed = parsePackage(raw);
  if (!parsed.ok) return parsed;
  const pkg = parsed.value;

  let gen: { files: [string, Uint8Array][]; imports: string[] };
  try {
    gen = await tools.generate(await src.bytes(pkg.component));
  } catch (e) {
    return { ok: false, errors: [`コンポーネントを読めない: ${String((e as Error).message ?? (e as { payload?: string }).payload ?? e)}`] };
  }

  // import の検査: どこにも当たらない import (宣言していない能力・ホストが渡さないもの) があれば入れない
  const capImports = new Set(pkg.capabilities.flatMap((c) => CAPABILITY_IMPORTS[c] ?? []));
  const missing = gen.imports.filter((i) => !BASE_IMPORTS.test(i) && !capImports.has(i.replace(/@[0-9.]+$/, '')));
  if (missing.length) return { ok: false, errors: missing.map(missingReason) };

  let mod: ComponentModule;
  const cores = new Map<string, WebAssembly.Module>();
  try {
    const js = gen.files.find(([n]) => n.endsWith('.js'));
    if (!js) throw new Error('glue の JS が生成されなかった');
    for (const [n, bytes] of gen.files) if (n.endsWith('.wasm')) cores.set(n, await WebAssembly.compile(bytes as BufferSource));
    mod = await tools.loadModule(new TextDecoder().decode(js[1]));
  } catch (e) {
    return { ok: false, errors: [`glue を読み込めない: ${String((e as Error).message ?? e)}`] };
  }
  const getCore = (p: string) => {
    // 同じ名前のファイルが別のディレクトリにあっても取り違えないよう、パスが一致するものを先に探す
    const m = cores.get(p) ?? cores.get(p.split('/').pop()!);
    if (!m) throw new Error(`core wasm ${p} が生成されていない`);
    return m;
  };

  // export の検査 (import の検査とは別): world gui-app の app (start / on-action) を出していなければ、開けないので入れない
  const denied: AppServices['storage'] = { get: () => ({ ok: false, reason: 'denied' }), set: () => ({ ok: false, reason: 'denied' }) };
  let dryExports: unknown;
  try {
    dryExports = mod.instantiate(getCore, importsFor(pkg, wasiShim, { emit: () => {} }, { storage: denied } as AppServices));
  } catch (e) {
    return { ok: false, errors: [`インスタンスを作れない: ${String((e as Error).message ?? e)}`] };
  }
  const app = (dryExports as { app?: { start?: unknown; onAction?: unknown } } | null)?.app;
  if (typeof app?.start !== 'function' || typeof app?.onAction !== 'function') return { ok: false, errors: ['gui:app/app (start と on-action) を export していない'] };

  const manifest: AppManifest = {
    id: pkg.id,
    title: pkg.title,
    description: pkg.description,
    kind: 'component',
    capabilities: pkg.capabilities,
    multiInstance: pkg.multiInstance,
    create: (id, services) =>
      createComponentApp(id, (host) => mod.instantiate(getCore, importsFor(pkg, wasiShim, host, services)) as ComponentExports),
  };
  return { ok: true, manifest, pkg };
}
