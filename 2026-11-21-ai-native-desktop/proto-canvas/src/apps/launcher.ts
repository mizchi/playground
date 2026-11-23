// ランチャー: インストール済みのアプリを開くシステムのアプリ。これ自体も、意味ツリーを出すだけの普通のアプリ。
import type { AppManifest } from '../platform/types.ts';
import type { ActionRequest, NodeId, SemanticNode, TreeUpdate } from '../protocol.ts';
import type { App } from './app.ts';
import type { AppServices } from '../platform/types.ts';

export function createLauncherApp(id: string, services: AppServices): App {
  let emit: (u: TreeUpdate) => void = () => {};
  let focus: NodeId | null = null;
  let status = '';
  let sent = new Set<string>();
  const publish = (root?: string) => {
    const r = services.apps.list();
    const apps = r.ok ? r.value : [];
    const nodes: SemanticNode[] = [
      { id: 'root', role: 'x-window', label: 'アプリ', children: ['title', 'list', 'status'] },
      { id: 'title', role: 'heading', label: 'アプリを開く' },
      { id: 'list', role: 'list', label: 'インストール済み', children: apps.map((a) => `open-${a.id}`) },
      ...apps.map((a): SemanticNode => ({ id: `open-${a.id}`, role: 'button', label: `${a.title} を開く`, actions: ['click', 'focus'] })),
      { id: 'status', role: 'paragraph', label: status || (r.ok ? `${apps.length} 個のアプリ` : 'アプリの一覧を読めない') },
    ];
    const ids = new Set(nodes.map((n) => n.id));
    const remove = [...sent].filter((x) => !ids.has(x));
    sent = ids;
    emit({ t: 'tree-update', app: id, root, focus, nodes, remove });
  };
  return {
    id,
    start(e) {
      emit = e;
      publish('root');
    },
    onAction(req: ActionRequest) {
      if (req.action === 'focus') focus = req.target;
      else if (req.action === 'click' && req.target.startsWith('open-')) {
        const app = req.target.slice('open-'.length);
        const r = services.apps.launch(app);
        status = r.ok ? `${app} を開いた` : `${app} を開けない (${r.reason})`;
      }
      publish();
    },
  };
}

export const launcherManifest: AppManifest = {
  id: 'launcher',
  title: 'アプリ',
  description: 'インストール済みのアプリを開く',
  kind: 'js',
  capabilities: ['apps.launch'],
  multiInstance: false,
  system: true,
  create: (id, services) => createLauncherApp(id, services),
};
