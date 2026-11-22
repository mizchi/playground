import type { ActionRequest, NodeId, TreeUpdate } from '../protocol.ts';
import type { App } from './app.ts';

export function createSettingsApp(opts: { id?: string } = {}): App {
  const appId = opts.id ?? 'settings';
  let emit: (u: TreeUpdate) => void = () => {};
  let compact = false;
  let telemetry = false;
  let status = '';
  let focus: NodeId | null = null;

  const publish = (root?: string) =>
    emit({
      t: 'tree-update',
      app: appId,
      root,
      focus,
      nodes: [
        { id: 'root', role: 'x-window', label: 'Settings', children: ['title', 'compact', 'telemetry', 'reset', 'status'] },
        { id: 'title', role: 'heading', label: '設定' },
        { id: 'compact', role: 'switch', label: 'コンパクト表示', checked: compact, actions: ['click', 'focus'] },
        { id: 'telemetry', role: 'switch', label: '利用状況を送信', checked: telemetry, sensitive: true, actions: ['click', 'focus'] },
        { id: 'reset', role: 'button', label: '全データを削除', sensitive: true, actions: ['click', 'focus'] },
        { id: 'status', role: 'paragraph', label: status || '変更はまだありません' },
      ],
    });

  return {
    id: appId,
    start(e) {
      emit = e;
      publish('root');
    },
    onAction(req: ActionRequest) {
      if (req.action === 'focus') focus = req.target;
      else if (req.target === 'compact') compact = !compact;
      else if (req.target === 'telemetry') {
        telemetry = !telemetry;
        status = `利用状況の送信を${telemetry ? 'オン' : 'オフ'}にしました (${req.by})`;
      } else if (req.target === 'reset') {
        // アプリ自身も actor を見て判断できる: 破壊的操作は承認済みでも agent からは受けない
        status = req.by === 'agent' ? 'エージェントからの全削除は拒否しました' : '全データを削除しました (デモ)';
      }
      publish();
    },
  };
}
