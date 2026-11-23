// システムのサービス (クリップボード・通知・保存領域・アプリの起動)。どの呼び出しも能力を確かめ、監査ログに残す。
import type { Storage } from '../cache.ts';
import type { AppServices, Capability, GrantPrompt, GrantState, MergeRequest, Notification, PtyConnector, ServiceResult, StorageChange, Workspace } from './types.ts';
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
const DEFAULT_GRANT: Record<Exclude<Capability, `data:${string}`>, GrantState> = {
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
  /** 保存されているキー (アプリの保存領域の写しを作るのに使う) */
  keys(): string[];
}

export const memoryKeyValue = (): KeyValue => {
  const m = new Map<string, string>();
  return { get: (k) => m.get(k), set: (k, v) => void m.set(k, v), keys: () => [...m.keys()] };
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
  readonly merges: MergeRequest[] = [];
  /** agent のワークスペースの書き込み (kv のキー → 値)。人が持ち込むまで本物には書かない */
  private overlay = new Map<string, string>();
  private storageListeners = new Map<string, ((key: string) => void)[]>();
  /** 重ねた層が変わったとき (作業場のアプリが差分を出し直す) */
  onOverlayChange: () => void = () => {};
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
    // ほかのアプリのデータは、いつも尋ねる
    if (cap.startsWith('data:')) return 'ask';
    return DEFAULT_GRANT[cap as keyof typeof DEFAULT_GRANT];
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

  /** 窓に渡すサービス。agent のワークスペースの窓では、保存領域は重ねた層に書き、外へ出る能力は使えない */
  servicesFor(app: string, window: string, workspace: Workspace = 'human'): AppServices {
    if (workspace === 'agent') return this.agentServicesFor(app, window);
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
        entries: () => {
          const r = this.check(app, window, 'storage', 'entries');
          return r.ok ? { ok: true, value: this.storageEntries(app) } : fail(r);
        },
        onChange: (cb) => this.onStorageChange(app, cb),
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
      data: {
        query: (topic, args = {}) => {
          const provider = this.registry.list({ includeSystem: true }).find((m) => m.provides?.[topic] && m.provide);
          if (!provider) return { ok: false, reason: 'unavailable' };
          const r = this.check(app, window, `data:${topic}`, `query ${topic} from ${provider.id}`);
          if (!r.ok) return fail(r);
          // 提供する側は、自分の能力付きのサービスで答える (相手の保存領域には触れない)
          const value = provider.provide!(topic, structuredClone(args), this.servicesFor(provider.id, `${provider.id}#provider`));
          return { ok: true, value: structuredClone(value) };
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

  private onStorageChange(app: string, cb: (key: string) => void) {
    if (!this.storageListeners.has(app)) this.storageListeners.set(app, []);
    this.storageListeners.get(app)!.push(cb);
  }

  /** agent のワークスペースの窓のサービス */
  private agentServicesFor(app: string, window: string): AppServices {
    const human = this.servicesFor(app, window, 'human');
    const k = (key: string) => `app.${app}.${key}`;
    // 外へ出る能力 (クリップボード・通知・アプリの起動・ホストのシェル) は使わせない。人の目に触れる前に外へ漏れないように
    const outside = (cap: Capability, op: string) => {
      this.audit.push({ at: Date.now(), app, window, capability: cap, op: `${op} (agent のワークスペース)`, result: 'denied' });
      return { ok: false as const, reason: 'denied' as const };
    };
    return {
      ...human,
      clipboard: { read: () => outside('clipboard.read', 'read'), write: () => outside('clipboard.write', 'write') },
      notify: () => outside('notifications', 'notify'),
      apps: { list: human.apps.list, launch: (id) => outside('apps.launch', `launch ${id}`) },
      pty: { available: () => false, open: () => outside('terminal.pty', 'open') },
      storage: {
        get: (key) => {
          const r = this.check(app, window, 'storage', `get ${key} (agent のワークスペース)`);
          if (!r.ok) return r as ServiceResult<string | undefined>;
          return { ok: true, value: this.overlay.has(k(key)) ? this.overlay.get(k(key)) : this.kv.get(k(key)) };
        },
        set: (key, value) => {
          const r = this.check(app, window, 'storage', `set ${key} (agent のワークスペース)`);
          if (!r.ok) return r as ServiceResult<void>;
          this.overlay.set(k(key), value);
          this.onOverlayChange();
          this.onChange();
          return { ok: true, value: undefined };
        },
        entries: () => {
          const r = this.check(app, window, 'storage', 'entries (agent のワークスペース)');
          if (!r.ok) return r as ServiceResult<[string, string][]>;
          const m = new Map(this.storageEntries(app));
          const prefix = `app.${app}.`;
          for (const [key, v] of this.overlay) if (key.startsWith(prefix)) m.set(key.slice(prefix.length), v);
          return { ok: true, value: [...m] };
        },
        // 重ねた層は agent のワークスペースだけのもの。持ち込みで本物が変わっても、こちらは知らせない
        onChange: () => {},
      },
    };
  }

  /** agent のワークスペースで溜まった変更 (本物と違うものだけ) */
  pendingChanges(): StorageChange[] {
    const out: StorageChange[] = [];
    for (const [key, after] of this.overlay) {
      const m = key.match(/^app\.([^.]+)\.(.+)$/);
      if (!m) continue;
      const before = this.kv.get(key);
      if (before !== after) out.push({ app: m[1], key: m[2], before, after });
    }
    return out;
  }

  /** 持ち込みを提案する (agent も人もできる)。変更が無ければ '' 以外を返す */
  proposeMerge(intent: string, by: 'human' | 'agent'): { ok: true; merge: MergeRequest } | { ok: false; reason: string } {
    const changes = this.pendingChanges();
    if (!changes.length) return { ok: false, reason: '持ち込む変更が無い' };
    const merge: MergeRequest = { id: `m${this.seq++}`, intent, by, changes, status: 'pending' };
    this.merges.push(merge);
    this.onOverlayChange();
    this.onChange();
    return { ok: true, merge };
  }

  /** 持ち込みに答える。答えられるのは人だけ。提案のあとに本物が変わっていれば書かない (stale) */
  answerMerge(id: string, approve: boolean, by: 'human' | 'agent'): string {
    const m = this.merges.find((x) => x.id === id);
    if (!m || m.status !== 'pending') return '提案が無い';
    if (by !== 'human') return '持ち込みに答えられるのは人だけ';
    if (!approve) {
      m.status = 'rejected';
      this.onOverlayChange();
      this.onChange();
      return '';
    }
    if (m.changes.some((c) => this.kv.get(`app.${c.app}.${c.key}`) !== c.before)) {
      m.status = 'stale';
      this.onOverlayChange();
      this.onChange();
      return '提案のあとに人のワークスペースの値が変わったので、書かなかった';
    }
    for (const c of m.changes) {
      this.kv.set(`app.${c.app}.${c.key}`, c.after);
      this.overlay.delete(`app.${c.app}.${c.key}`);
      this.audit.push({ at: Date.now(), app: c.app, window: '(持ち込み)', capability: 'storage', op: `merge ${c.key} (${m.id})`, result: 'ok' });
    }
    m.status = 'applied';
    for (const c of m.changes) for (const cb of this.storageListeners.get(c.app) ?? []) cb(c.key);
    this.onOverlayChange();
    this.onChange();
    return '';
  }

  /** agent のワークスペースの変更を捨てる (人だけ) */
  discardOverlay(by: 'human' | 'agent'): string {
    if (by !== 'human') return '変更を捨てられるのは人だけ';
    this.overlay.clear();
    this.onOverlayChange();
    this.onChange();
    return '';
  }

  /** アプリの保存領域の中身 (Worker で動くコンポーネントに写しを渡すため。許可の確認は呼ぶ側で行う) */
  storageEntries(app: string): [string, string][] {
    const prefix = `app.${app}.`;
    return this.kv
      .keys()
      .filter((k) => k.startsWith(prefix))
      .map((k): [string, string] => [k.slice(prefix.length), this.kv.get(k) ?? '']);
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
