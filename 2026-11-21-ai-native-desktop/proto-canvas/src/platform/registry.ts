// インストール済みのアプリ。shell に差し込む「窓の工場」も兼ねる。
import type { App } from '../apps/app.ts';
import type { WindowFactory } from '../shell.ts';
import type { AppManifest, AppServices } from './types.ts';

export class AppRegistry implements WindowFactory {
  private apps = new Map<string, AppManifest>();
  /** 窓 id → アプリ id */
  private windowApp = new Map<string, string>();
  private openIds: () => string[];
  private servicesFor: (appId: string, windowId: string) => AppServices;

  constructor(manifests: AppManifest[], openIds: () => string[], servicesFor: (appId: string, windowId: string) => AppServices) {
    for (const m of manifests) this.install(m);
    this.openIds = openIds;
    this.servicesFor = servicesFor;
  }

  install(m: AppManifest): void {
    if (!/^[a-z][a-z0-9-]*$/.test(m.id)) throw new Error(`アプリの id が不正: ${m.id}`);
    // 入っているアプリ (同梱のアプリを含む) を、あとから入れたもので置き換えさせない (なりすまし)
    if (this.apps.has(m.id)) throw new Error(`アプリ ${m.id} はすでに入っている`);
    this.apps.set(m.id, m);
  }

  manifest(id: string): AppManifest | undefined {
    return this.apps.get(id);
  }

  list(opts: { includeSystem?: boolean } = {}): AppManifest[] {
    return [...this.apps.values()].filter((m) => opts.includeSystem || !m.system);
  }

  /** 窓が属するアプリ */
  appOf(windowId: string): string | undefined {
    return this.windowApp.get(windowId);
  }

  // ---- WindowFactory ----
  has(appId: string): boolean {
    const m = this.apps.get(appId);
    if (!m) return false;
    // 1 つの窓だけのアプリは、開いていれば新しく開けない
    return m.multiInstance || !this.openIds().some((w) => this.windowApp.get(w) === appId);
  }

  title(appId: string): string {
    return this.apps.get(appId)?.title ?? appId;
  }

  nextId(appId: string): string {
    const open = new Set(this.openIds());
    if (!open.has(appId)) return appId;
    for (let n = 2; ; n++) if (!open.has(`${appId}-${n}`)) return `${appId}-${n}`;
  }

  create(appId: string, windowId: string): App {
    const m = this.apps.get(appId);
    if (!m) throw new Error(`アプリ ${appId} はインストールされていない`);
    this.windowApp.set(windowId, appId);
    const app = m.create(windowId, this.servicesFor(appId, windowId));
    if (app.id !== windowId) throw new Error(`アプリ ${appId} が窓 id ${windowId} を使わなかった (${app.id})`);
    return app;
  }

  /** 窓を閉じたとき */
  forget(windowId: string): void {
    this.windowApp.delete(windowId);
  }
}
