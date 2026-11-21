// ③ Template Cache: 構造シグネチャ -> template
import { validateTemplate } from './catalog.ts';
import type { ViewTemplate } from './catalog.ts';

export type EntryState = 'ephemeral' | 'pinned' | 'rejected';

export interface CacheEntry {
  sig: string;
  template: ViewTemplate;
  state: EntryState;
  /** 誰が作ったか (llm / human / sample) */
  origin: string;
  /** veriss で検証した結果を入れる場所 (未統合なので今は常に unchecked) */
  verification: 'unchecked' | 'verified' | 'violated' | 'unknown';
}

export interface Storage {
  load(): CacheEntry[];
  save(entries: CacheEntry[]): void;
}

export const memoryStorage = (): Storage => {
  let data: CacheEntry[] = [];
  return { load: () => structuredClone(data), save: (e) => (data = structuredClone(e)) };
};

export class TemplateCache {
  private entries = new Map<string, CacheEntry>();
  private storage: Storage;
  constructor(storage: Storage) {
    this.storage = storage;
    for (const e of storage.load()) if (validateTemplate(e.template).length === 0) this.entries.set(e.sig, e);
  }

  /** 静的検査を通ったものだけを ephemeral として受け入れる */
  propose(sig: string, template: unknown, origin: string): { ok: true; entry: CacheEntry } | { ok: false; errors: string[] } {
    const errors = validateTemplate(template);
    if (errors.length) return { ok: false, errors };
    const entry: CacheEntry = { sig, template: structuredClone(template as ViewTemplate), state: 'ephemeral', origin, verification: 'unchecked' };
    this.entries.set(sig, entry);
    this.persist();
    return { ok: true, entry };
  }

  setState(sig: string, state: EntryState): void {
    const e = this.entries.get(sig);
    if (!e) return;
    e.state = state;
    this.persist();
  }

  /** 使ってよい template (rejected は使わない) */
  lookup(sig: string): CacheEntry | undefined {
    const e = this.entries.get(sig);
    return e && e.state !== 'rejected' ? e : undefined;
  }

  remove(sig: string): void {
    this.entries.delete(sig);
    this.persist();
  }

  list(): CacheEntry[] {
    return [...this.entries.values()];
  }

  private persist(): void {
    // pinned と rejected だけ残す。ephemeral は再読み込みで消える
    this.storage.save(this.list().filter((e) => e.state !== 'ephemeral'));
  }
}
