// システムのサービス (クリップボード・通知・保存領域・アプリの起動)。どの呼び出しも能力を確かめ、監査ログに残す。
import type { Storage } from '../cache.ts';
import type { AppServices, Capability, GrantPrompt, GrantState, Notification, PtyConnector, ServiceResult } from './types.ts';
import type { AppRegistry } from './registry.ts';

export interface ServiceAudit {
  at: number;
  app: string;
  window: string;
  capability: Capability;
  op: string;
  result: 'ok' | 'denied' | 'pending' | 'undeclared';
}

/** 宣言しているときの既定の状態 */
const DEFAULT_GRANT: Record<Capability, GrantState> = {
  storage: 'granted', // 自分の保存領域だけ
  'clipboard.write': 'granted',
  'clipboard.read': 'ask', // 他のアプリが書いたものを読めるので、尋ねる
  notifications: 'ask',
  'apps.launch': 'ask', // システムのアプリは常に許可 (下)、それ以外は尋ねる
  'terminal.pty': 'ask', // ホストのシェルはファイルもネットワークも触れる。必ず尋ねる
};

/** 永続化に使う、文字列を読み書きできる場所 */
export interface KeyValue {
  get(key: string): string | undefined;
  set(key: string, value: string): void;
}

export const memoryKeyValue = (): KeyValue => {
  const m = new Map<string, string>();
  return { get: (k) => m.get(k), set: (k, v) => void m.set(k, v) };
};

export class ServiceHost {
  readonly audit: ServiceAudit[] = [];
  readonly prompts: GrantPrompt[] = [];
  readonly notifications: Notification[] = [];
  onChange: () => void = () => {};
  /** アプリの起動 (プラットフォームが差し込む) */
  launch: (appId: string, by: 'human' | 'agent') => string = () => '起動できない';
  registry!: AppRegistry;
  /** ホストの擬似端末の口 (main.ts が ?pty= のときに差し込む) */
  pty: PtyConnector | null = null;
  private clipboard = '';
  private grants = new Map<string, Map<Capability, GrantState>>();
  private grantListeners = new Map<string, ((cap: Capability, granted: boolean) => void)[]>();
  private kv: KeyValue;
  private seq = 1;

  constructor(kv: KeyValue) {
    this.kv = kv;
    try {
      const saved = JSON.parse(kv.get('platform.grants') ?? '{}') as Record<string, Record<Capability, GrantState>>;
      for (const [app, g] of Object.entries(saved)) this.grants.set(app, new Map(Object.entries(g) as [Capability, GrantState][]));
    } catch {
      /* 壊れていたら初期状態から */
    }
  }

  /** アプリの能力の状態。宣言していなければ undefined */
  grant(app: string, cap: Capability): GrantState | undefined {
    const m = this.registry.manifest(app);
    if (!m || !m.capabilities.includes(cap)) return undefined;
    const saved = this.grants.get(app)?.get(cap);
    if (saved) return saved;
    if (cap === 'apps.launch' && m.system) return 'granted';
    return DEFAULT_GRANT[cap];
  }

  setGrant(app: string, cap: Capability, state: GrantState): void {
    if (!this.grants.has(app)) this.grants.set(app, new Map());
    this.grants.get(app)!.set(cap, state);
    const obj = Object.fromEntries([...this.grants].map(([a, g]) => [a, Object.fromEntries(g)]));
    this.kv.set('platform.grants', JSON.stringify(obj));
  }

  /** 許可の要求に答える。答えられるのは人だけ (agent には答えさせない) */
  answer(promptId: string, granted: boolean, by: 'human' | 'agent'): string {
    const p = this.prompts.find((x) => x.id === promptId);
    if (!p || p.status !== 'pending') return '要求がない';
    if (by !== 'human') return '許可の要求に答えられるのは人だけ';
    p.status = granted ? 'granted' : 'denied';
    this.setGrant(p.app, p.capability, granted ? 'granted' : 'denied');
    for (const cb of this.grantListeners.get(p.app) ?? []) cb(p.capability, granted);
    this.onChange();
    return '';
  }

  /** 能力を確かめる。ask なら要求を出して pending */
  private check(app: string, window: string, cap: Capability, op: string): ServiceResult<true> {
    const g = this.grant(app, cap);
    const log = (result: ServiceAudit['result']) => this.audit.push({ at: Date.now(), app, window, capability: cap, op, result });
    if (g === undefined) {
      log('undeclared');
      return { ok: false, reason: 'undeclared' };
    }
    if (g === 'denied') {
      log('denied');
      return { ok: false, reason: 'denied' };
    }
    if (g === 'ask') {
      if (!this.prompts.some((p) => p.app === app && p.capability === cap && p.status === 'pending'))
        this.prompts.push({ id: `g${this.seq++}`, app, appTitle: this.registry.manifest(app)?.title ?? app, capability: cap, status: 'pending' });
      log('pending');
      this.onChange();
      return { ok: false, reason: 'pending' };
    }
    log('ok');
    return { ok: true, value: true };
  }

  /** 窓に渡すサービス */
  servicesFor(app: string, window: string): AppServices {
    const fail = <T>(r: ServiceResult<true>) => r as ServiceResult<T>;
    return {
      clipboard: {
        read: () => {
          const r = this.check(app, window, 'clipboard.read', 'read');
          return r.ok ? { ok: true, value: this.clipboard } : fail(r);
        },
        write: (text) => {
          const r = this.check(app, window, 'clipboard.write', 'write');
          if (!r.ok) return fail(r);
          this.clipboard = text;
          return { ok: true, value: undefined };
        },
      },
      notify: (title, body) => {
        const r = this.check(app, window, 'notifications', 'notify');
        if (!r.ok) return fail(r);
        this.notifications.push({ id: this.seq++, app, window, title, body, at: Date.now() });
        this.onChange();
        return { ok: true, value: undefined };
      },
      storage: {
        get: (key) => {
          const r = this.check(app, window, 'storage', `get ${key}`);
          return r.ok ? { ok: true, value: this.kv.get(`app.${app}.${key}`) } : fail(r);
        },
        set: (key, value) => {
          const r = this.check(app, window, 'storage', `set ${key}`);
          if (!r.ok) return fail(r);
          this.kv.set(`app.${app}.${key}`, value);
          return { ok: true, value: undefined };
        },
      },
      apps: {
        list: () => {
          const r = this.check(app, window, 'apps.launch', 'list');
          return r.ok ? { ok: true, value: this.registry.list().map((m) => ({ id: m.id, title: m.title, description: m.description })) } : fail(r);
        },
        launch: (id) => {
          const r = this.check(app, window, 'apps.launch', `launch ${id}`);
          if (!r.ok) return fail(r);
          const why = this.launch(id, 'human');
          return why ? { ok: false, reason: 'denied' } : { ok: true, value: undefined };
        },
      },
      pty: {
        available: () => !!this.pty,
        open: (cols, rows, onData, onExit) => {
          if (!this.pty) return { ok: false, reason: 'unavailable' };
          const r = this.check(app, window, 'terminal.pty', `open ${cols}x${rows}`);
          if (!r.ok) return fail(r);
          return { ok: true, value: this.pty(cols, rows, onData, onExit) };
        },
      },
      onGrant: (cb) => {
        if (!this.grantListeners.has(app)) this.grantListeners.set(app, []);
        this.grantListeners.get(app)!.push(cb);
      },
    };
  }

  /** 人が読んだ通知を消す */
  dismiss(id: number): void {
    const i = this.notifications.findIndex((n) => n.id === id);
    if (i >= 0) this.notifications.splice(i, 1);
    this.onChange();
  }
}

/** Storage (テンプレートのキャッシュ用) と同じ場所を KeyValue としても使う */
export type { Storage };
