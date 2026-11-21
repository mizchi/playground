import type { ActionRequest, NodeId, SemanticNode, TreeUpdate } from '../protocol.ts';
import type { App } from './app.ts';

export function createNotesApp(): App {
  let emit: (u: TreeUpdate) => void = () => {};
  let notes = ['タイトルバーを掴んで運ぶ', '端で分割、中央で入れ替え', '外周なら全体の端へ'];
  let draft = '';
  let focus: NodeId | null = null;
  let sent = new Set<NodeId>();

  const publish = (root?: string) => {
    const nodes: SemanticNode[] = [
      { id: 'root', role: 'x-window', label: 'Notes', children: ['title', ...notes.map((_, i) => `n${i}`), 'entry'] },
      { id: 'title', role: 'heading', label: 'メモ' },
      ...notes.map((t, i): SemanticNode => ({ id: `n${i}`, role: 'paragraph', label: t })),
      { id: 'entry', role: 'group', label: 'メモを追加', children: ['input', 'save'] },
      { id: 'input', role: 'textbox', label: 'メモ', value: draft, actions: ['focus', 'setValue', 'replaceSelectedText'] },
      { id: 'save', role: 'button', label: '保存', actions: ['click', 'focus'] },
    ];
    const ids = new Set(nodes.map((n) => n.id));
    const remove = [...sent].filter((id) => !ids.has(id));
    sent = ids;
    emit({ t: 'tree-update', app: 'notes', root, focus, nodes, remove });
  };

  return {
    id: 'notes',
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
      }
      publish();
    },
  };
}
