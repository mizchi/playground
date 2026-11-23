// 同梱のアプリの manifest (インストール済みのアプリ)
import type { AppManifest } from '../platform/types.ts';
import { calendarManifest } from './calendar.ts';
import { launcherManifest } from './launcher.ts';
import { createNotesApp } from './notes.ts';
import { createSettingsApp } from './settings.ts';
import { createTodoApp } from './todo.ts';

export const todoManifest: AppManifest = {
  id: 'todo',
  title: 'Todo',
  description: 'やることの一覧。項目を保存し、すべて終わったら通知する',
  kind: 'js',
  capabilities: ['storage', 'notifications'],
  multiInstance: true,
  create: (id, services) => createTodoApp({ id, services }),
};

export const settingsManifest: AppManifest = {
  id: 'settings',
  title: 'Settings',
  description: '設定 (sensitive な操作を含む)',
  kind: 'js',
  capabilities: [],
  multiInstance: false,
  create: (id) => createSettingsApp({ id }),
};

export const notesManifest: AppManifest = {
  id: 'notes',
  title: 'Notes',
  description: 'メモ。保存とクリップボード',
  kind: 'js',
  capabilities: ['storage', 'clipboard.read', 'clipboard.write'],
  multiInstance: true,
  create: (id, services) => createNotesApp({ id, services }),
};

export const builtinManifests: AppManifest[] = [launcherManifest, todoManifest, settingsManifest, notesManifest, calendarManifest];
