// プラットフォームの型: アプリの宣言 (manifest)、能力 (capability)、システムのサービス。
import type { App } from '../apps/app.ts';

/**
 * アプリが使える能力。manifest で宣言し、人が許可したものだけが使える。
 * - storage: アプリ自身の保存領域 (同じアプリの窓で共有)
 * - clipboard.read / clipboard.write: クリップボード
 * - notifications: 通知
 * - apps.launch: ほかのアプリを起動する
 * - terminal.pty: ホストの本物のシェル (擬似端末) につなぐ。ホストが pty の橋を用意しているときだけ使える
 * - data:<topic>: ほかのアプリが提供するデータを読む (例: data:calendar.events)。提供する側は manifest の provides で宣言する
 */
export type Capability = 'storage' | 'clipboard.read' | 'clipboard.write' | 'notifications' | 'apps.launch' | 'terminal.pty' | `data:${string}`;

/** 能力の状態。ask は「初めて使うときに人に尋ねる」 */
export type GrantState = 'granted' | 'denied' | 'ask';

export type ServiceResult<T> = { ok: true; value: T } | { ok: false; reason: 'denied' | 'pending' | 'undeclared' | 'unavailable' };

/** 擬似端末への接続 (ホストのシェル) */
export interface PtyHandle {
  write(data: string): void;
  /** 端末の大きさを変える (TIOCSWINSZ。シェルには SIGWINCH が届く) */
  resize(cols: number, rows: number): void;
  close(): void;
}
/** ホストが用意する擬似端末の口 (無ければ terminal.pty は unavailable) */
export type PtyConnector = (cols: number, rows: number, onData: (data: string) => void, onExit: (why: string) => void) => PtyHandle;

/** アプリに渡すシステムのサービス。どの呼び出しも能力を確かめ、監査ログに残る */
export interface AppServices {
  clipboard: { read(): ServiceResult<string>; write(text: string): ServiceResult<void> };
  notify(title: string, body?: string): ServiceResult<void>;
  storage: {
    get(key: string): ServiceResult<string | undefined>;
    set(key: string, value: string): ServiceResult<void>;
    /** 保存領域の中身 (Worker で動くコンポーネントに写しを渡すため) */
    entries(): ServiceResult<[string, string][]>;
    /** ほかから保存領域が変わったとき (agent のワークスペースの変更を人が持ち込んだ、など)。アプリは読み直す */
    onChange(cb: (key: string) => void): void;
  };
  apps: { list(): ServiceResult<{ id: string; title: string; description: string }[]>; launch(id: string): ServiceResult<void> };
  /** ほかのアプリが提供するデータを読む (data:<topic> の能力が要る)。提供するアプリが無ければ unavailable */
  data: { query(topic: string, args?: Record<string, unknown>): ServiceResult<unknown> };
  /** ホストのシェルを開く。出力は onData に届く */
  pty: { available(): boolean; open(cols: number, rows: number, onData: (data: string) => void, onExit: (why: string) => void): ServiceResult<PtyHandle> };
  /** 人が許可の要求に答えたとき (ask のあと) */
  onGrant(cb: (cap: Capability, granted: boolean) => void): void;
}

export interface AppManifest {
  /** アプリの id。最初の窓の id にもなる (2 枚目以降は id-2, id-3 ...) */
  id: string;
  title: string;
  description: string;
  /** どう動くか: js (TypeScript のアプリ) / component (WIT の wasm コンポーネント) / legacy (AT-SPI の既存アプリ) */
  kind: 'js' | 'component' | 'legacy';
  /** 使いたい能力。宣言していない能力は、尋ねることもできない */
  capabilities: Capability[];
  /** 同じアプリを複数の窓で開けるか */
  multiInstance: boolean;
  /** システムのアプリ (ランチャーなど)。ランチャーの一覧に出さない */
  system?: boolean;
  /** 窓を作る。services はこの窓に渡す能力付きのサービス */
  create(windowId: string, services: AppServices): App;
  /**
   * ほかのアプリに提供するデータ (読み取りだけ)。窓を開いていなくても答えられるよう、manifest の関数として持つ。
   * services は提供する側のアプリの能力付きのサービス (自分の保存領域を読むなど)。返す値は JSON で表せるもの
   */
  provides?: Record<string, { description: string }>;
  provide?(topic: string, args: Record<string, unknown>, services: AppServices): unknown;
}

/** 窓がどのワークスペースにあるか。agent のワークスペースでは、保存領域への書き込みは重ねた層に溜まり、本物は変わらない */
export type Workspace = 'human' | 'agent';

/** agent のワークスペースで溜まった変更 (1 つのキー) */
export interface StorageChange {
  app: string;
  key: string;
  /** 本物の今の値 (無ければ undefined) */
  before: string | undefined;
  after: string;
}

/** 持ち込みの提案: agent のワークスペースの変更を、人のワークスペース (本物) に書く。人だけが答える */
export interface MergeRequest {
  id: string;
  intent: string;
  by: 'human' | 'agent';
  changes: StorageChange[];
  /** applied: 書いた / rejected: 人が却下 / stale: 提案のあとに本物が変わったので書かなかった */
  status: 'pending' | 'applied' | 'rejected' | 'stale';
}

/** 許可の要求 (人が答える) */
export interface GrantPrompt {
  id: string;
  app: string;
  appTitle: string;
  capability: Capability;
  status: 'pending' | 'granted' | 'denied';
}

export interface Notification {
  id: number;
  app: string;
  window: string;
  title: string;
  body?: string;
  at: number;
}
