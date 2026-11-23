// アプリのパッケージ: app.json (manifest) + WIT のコンポーネント (jco で ES module にしたもの)。
// 能力 = import。ホストは、宣言して許可された能力の import だけを渡す。宣言していない import を求めるコンポーネントは入れない。
// ブラウザでは URL から、テストではファイルから読む (読み方は PackageSource で差し替える)。
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
  /** jco の生成物 (instantiate を export する ES module)。app.json からの相対パス */
  entry: string;
  /** コンポーネントの中の core wasm (jco の生成物)。読み込みの前にすべて compile しておく */
  core: string[];
}

/** jco の --instantiation sync の生成物 */
export interface ComponentModule {
  instantiate(getCoreModule: (path: string) => WebAssembly.Module, imports: Record<string, unknown>): unknown;
}

export interface PackageSource {
  /** app.json の中身 */
  manifest(): Promise<unknown>;
  /** entry の ES module */
  module(path: string): Promise<ComponentModule>;
  /** core wasm のバイト列 */
  bytes(path: string): Promise<BufferSource>;
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
  for (const k of ['title', 'description', 'entry']) if (!str(k)) errors.push(`${k} は 1..200 文字の文字列`);
  if (o.kind !== 'component') errors.push('kind は component');
  if (typeof o.multiInstance !== 'boolean') errors.push('multiInstance は boolean');
  const caps = o.capabilities;
  if (!Array.isArray(caps) || !caps.every((c) => typeof c === 'string' && (KNOWN_CAPS.includes(c as Capability) || /^data:[a-z0-9.-]+$/.test(c))))
    errors.push(`capabilities は ${KNOWN_CAPS.join(' | ')} | data:<topic> の配列`);
  if (!Array.isArray(o.core) || o.core.length === 0 || !o.core.every((c) => typeof c === 'string')) errors.push('core は core wasm のパスの配列');
  // パッケージの外を読ませない
  const paths = [o.entry, ...(Array.isArray(o.core) ? o.core : [])].filter((p): p is string => typeof p === 'string');
  for (const p of paths) if (!/^[\w.-]+(\/[\w.-]+)*$/.test(p) || p.split('/').includes('..')) errors.push(`${p}: パッケージの中の相対パスだけ`);
  if (errors.length) return { ok: false, errors };
  return { ok: true, value: o as unknown as PackageManifest };
}

/** WASI の実装 (preview2-shim) を受け取り、能力に応じた import を作る */
export function importsFor(pkg: PackageManifest, wasiShim: Record<string, Record<string, unknown>>, host: { emit(u: WitTreeUpdate): void }, services: AppServices): Record<string, unknown> {
  const imports: Record<string, unknown> = { ...wasiImports(wasiShim, { preopens: 'none', version: '0.2.9' }), ...hostImports(host) };
  if (pkg.capabilities.includes('storage')) {
    const storage = {
      get: (key: string) => {
        const r = services.storage.get(key);
        return r.ok ? r.value : undefined;
      },
      set: (key: string, value: string) => services.storage.set(key, value).ok,
    };
    imports['gui:app/storage'] = storage;
    imports['gui:app/storage@0.1.0'] = storage;
  }
  return imports;
}

/** import を求められた名前を記録する (渡していない import を、どの能力が足りないかとして説明するため) */
function recording(imports: Record<string, unknown>, asked: Set<string>): Record<string, unknown> {
  return new Proxy(imports, {
    get(t, k) {
      if (typeof k === 'string') asked.add(k);
      return Reflect.get(t, k);
    },
  });
}

function missingReason(name: string): string {
  const base = name.replace(/@[0-9.]+$/, '');
  const cap = (Object.entries(CAPABILITY_IMPORTS) as [Capability, string[]][]).find(([, names]) => names.includes(base))?.[0];
  return cap ? `${base} を import している (能力 ${cap} を宣言していない)` : `${base} を import している (ホストが渡さない import)`;
}

/**
 * パッケージを読み、AppManifest にする。
 * 入れる前に、空のサービスで一度インスタンスを作り、宣言していない import を求めないかを確かめる
 */
export async function loadPackage(
  src: PackageSource,
  wasiShim: Record<string, Record<string, unknown>>,
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
  const cores = new Map<string, WebAssembly.Module>();
  let mod: ComponentModule;
  try {
    for (const p of pkg.core) cores.set(p, await WebAssembly.compile(await src.bytes(p)));
    mod = await src.module(pkg.entry);
  } catch (e) {
    return { ok: false, errors: [`コンポーネントを読めない: ${String((e as Error).message ?? e)}`] };
  }
  const getCore = (p: string) => {
    const m = cores.get(p.split('/').pop()!) ?? cores.get(p);
    if (!m) throw new Error(`core wasm ${p} が app.json の core に無い`);
    return m;
  };

  // 試しにインスタンスを作る: 渡していない import を求めれば、ここで失敗する
  const denied: AppServices['storage'] = { get: () => ({ ok: false, reason: 'denied' }), set: () => ({ ok: false, reason: 'denied' }) };
  const asked = new Set<string>();
  try {
    const dry = importsFor(pkg, wasiShim, { emit: () => {} }, { storage: denied } as AppServices);
    mod.instantiate(getCore, recording(dry, asked));
  } catch (e) {
    const provided = new Set(Object.keys(importsFor(pkg, wasiShim, { emit: () => {} }, { storage: denied } as AppServices)));
    const missing = [...asked].filter((k) => !provided.has(k) && !BASE_IMPORTS.test(k));
    return { ok: false, errors: missing.length ? missing.map(missingReason) : [`インスタンスを作れない: ${String((e as Error).message ?? e)}`] };
  }

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
