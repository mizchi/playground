// プラットフォームの組み立ての起点。インストール済みのアプリ・システムのサービス・shell をつなぐ。
// DOM に依存しない (ブラウザの描画 backend は main.ts / backends.ts が選ぶ)。
import type { App } from '../apps/app.ts';
import type { Storage } from '../cache.ts';
import type { Actor, AppId, Measure } from '../protocol.ts';
import { Shell } from '../shell.ts';
import { allApps } from '../tiling.ts';
import type { TileNode } from '../tiling.ts';
import { AppRegistry } from './registry.ts';
import { ServiceHost } from './services.ts';
import type { KeyValue } from './services.ts';
import type { AppManifest } from './types.ts';

export interface PlatformConfig {
  apps: AppManifest[];
  /** 起動時に開くアプリ (保存したセッションが無いとき) */
  initial: string[];
  measure: Measure;
  /** テンプレートのキャッシュの保存先 */
  templates: Storage;
  /** 能力の許可・アプリの保存領域・セッションの保存先 */
  kv: KeyValue;
  /** 前回のセッション (開いていた窓と配置) を復元する */
  restoreSession?: boolean;
}

/** 保存するセッション */
export interface Session {
  v: 1;
  windows: { id: AppId; app: string }[];
  tiles: TileNode;
  minimized: AppId[];
  zoomed: AppId | null;
}

export interface Platform {
  shell: Shell;
  registry: AppRegistry;
  services: ServiceHost;
  /** アプリを新しい窓で開く (人の操作として。agent は shell.propose で launch を提案する) */
  launch(appId: string, by?: Actor): string;
  close(windowId: AppId, by?: Actor): string;
  /** アプリを入れる (パッケージなど)。入っている id と同じなら断る。成功なら '' */
  install(m: AppManifest): string;
  /** 今のセッション。変わっていれば保存する */
  saveSession(): Session;
}

const SESSION_KEY = 'platform.session';

export function createPlatform(cfg: PlatformConfig): Platform {
  const services = new ServiceHost(cfg.kv);
  // 起動中に作った窓の id (shell ができる前に、窓の工場が開いている窓を知るため)
  const booting: AppId[] = [];
  let shell: Shell | null = null;
  const registry = new AppRegistry(
    cfg.apps,
    () => (shell ? shell.windows().map((w) => w.id) : booting),
    (app, win) => services.servicesFor(app, win),
  );
  services.registry = registry;

  // 前回のセッションを読む。窓のアプリがすべてインストール済みで、木と窓が一致するときだけ使う
  let session: Session | null = null;
  if (cfg.restoreSession) {
    try {
      const s = JSON.parse(cfg.kv.get(SESSION_KEY) ?? 'null') as Session | null;
      const ids = new Set(s?.windows.map((w) => w.id));
      const placed = s ? [...allApps(s.tiles), ...s.minimized] : [];
      if (s && s.v === 1 && s.windows.every((w) => registry.manifest(w.app)) && placed.length === ids.size && placed.every((id) => ids.has(id))) session = s;
    } catch {
      session = null;
    }
  }

  const apps: App[] = [];
  const open = (app: string, id: AppId) => {
    booting.push(id);
    apps.push(registry.create(app, id));
  };
  if (session) for (const w of session.windows) open(w.app, w.id);
  else for (const app of cfg.initial) open(app, registry.nextId(app));

  shell = new Shell(apps, cfg.measure, cfg.templates);
  shell.factory = registry;
  shell.onWindowClosed = (id) => registry.forget(id);
  if (session) {
    shell.tiles = session.tiles;
    shell.minimized = session.minimized;
    shell.zoomed = session.zoomed;
  }
  const sh = shell;
  services.launch = (appId, by) => sh.applyLayout({ type: 'launch', app: appId }, by);

  let lastSaved = '';
  return {
    shell: sh,
    registry,
    services,
    launch: (appId, by = 'human') => sh.applyLayout({ type: 'launch', app: appId }, by),
    close: (id, by = 'human') => sh.applyLayout({ type: 'close', app: id }, by),
    install(m) {
      try {
        registry.install(m);
        sh.onChange();
        return '';
      } catch (e) {
        return String((e as Error).message ?? e);
      }
    },
    saveSession() {
      const l = sh.getLayout();
      const s: Session = { v: 1, windows: sh.windows().map((w) => ({ id: w.id, app: registry.appOf(w.id) ?? w.id })), tiles: l.tiles, minimized: l.minimized, zoomed: l.zoomed };
      const json = JSON.stringify(s);
      if (json !== lastSaved) {
        cfg.kv.set(SESSION_KEY, json);
        lastSaved = json;
      }
      return s;
    },
  };
}
