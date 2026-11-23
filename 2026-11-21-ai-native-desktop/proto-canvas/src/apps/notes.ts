import type { AppServices } from '../platform/types.ts';
import type { ActionRequest, NodeId, SemanticNode, TreeUpdate } from '../protocol.ts';
import type { App } from './app.ts';

/** services を渡すと、メモを保存し (storage)、コピー / 貼り付け (clipboard) ができる */
export function createNotesApp(opts: { id?: string; services?: AppServices } = {}): App {
  const appId = opts.id ?? 'notes';
  const sv = opts.services;
  let emit: (u: TreeUpdate) => void = () => {};
  let notes = ['タイトルバーを掴んで運ぶ', '端で分割、中央で入れ替え', '外周なら全体の端へ'];
  const initial = [...notes];
  let draft = '';
  let status = '';
  let focus: NodeId | null = null;
  let sent = new Set<NodeId>();
  if (sv) {
    const saved = sv.storage.get('notes');
    if (saved.ok && saved.value) notes = JSON.parse(saved.value) as string[];
    sv.storage.onChange((key) => {
      if (key !== 'notes') return;
      const r = sv.storage.get('notes');
      if (r.ok) notes = r.value ? (JSON.parse(r.value) as string[]) : [...initial];
      publish();
    });
    sv.onGrant((cap, granted) => {
      if (cap === 'clipboard.read') {
        status = granted ? 'クリップボードの読み取りが許可された。もう一度「貼り付け」を押す' : 'クリップボードの読み取りは拒否された';
        publish();
      }
    });
  }
  const save = () => sv?.storage.set('notes', JSON.stringify(notes));

  const publish = (root?: string) => {
    const nodes: SemanticNode[] = [
      { id: 'root', role: 'x-window', label: appId === 'notes' ? 'Notes' : `Notes (${appId})`, children: ['title', ...notes.map((_, i) => `n${i}`), 'entry', ...(sv ? ['clip', 'status'] : [])] },
      { id: 'title', role: 'heading', label: 'メモ' },
      ...notes.map((t, i): SemanticNode => ({ id: `n${i}`, role: 'paragraph', label: t })),
      { id: 'entry', role: 'group', label: 'メモを追加', children: ['input', 'save'] },
      { id: 'input', role: 'textbox', label: 'メモ', value: draft, actions: ['focus', 'setValue', 'replaceSelectedText'] },
      { id: 'save', role: 'button', label: '保存', actions: ['click', 'focus'] },
    ];
    if (sv)
      nodes.push(
        { id: 'clip', role: 'group', label: 'クリップボード', children: ['copy', 'paste'] },
        { id: 'copy', role: 'button', label: '最後のメモをコピー', actions: ['click', 'focus'] },
        { id: 'paste', role: 'button', label: '貼り付け', actions: ['click', 'focus'] },
        { id: 'status', role: 'paragraph', label: status || `${notes.length} 件のメモ` },
      );
    const ids = new Set(nodes.map((n) => n.id));
    const remove = [...sent].filter((id) => !ids.has(id));
    sent = ids;
    emit({ t: 'tree-update', app: appId, root, focus, nodes, remove });
  };

  return {
    id: appId,
    start(e) {
      emit = e;
      publish('root');
    },
    onAction(req: ActionRequest) {
      if (req.action === 'focus') focus = req.target;
      else if (req.target === 'input' && req.action === 'setValue') draft = req.data ?? '';
      else if (req.target === 'input' && req.action === 'replaceSelectedText') draft += req.data ?? '';
      else if (req.target === 'save' && draft.trim()) {
        notes = [...notes, draft.trim()];
        draft = '';
        save();
      } else if (req.target === 'copy' && sv) {
        const r = sv.clipboard.write(notes.at(-1) ?? '');
        status = r.ok ? 'コピーした' : `コピーできない (${r.reason})`;
      } else if (req.target === 'paste' && sv) {
        const r = sv.clipboard.read();
        if (r.ok) {
          draft += r.value;
          status = '貼り付けた';
        } else status = r.reason === 'pending' ? 'クリップボードの読み取りの許可を待っている' : `貼り付けられない (${r.reason})`;
      }
      publish();
    },
  };
}
