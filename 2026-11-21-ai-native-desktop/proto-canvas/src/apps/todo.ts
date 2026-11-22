import type { ActionRequest, NodeId, SemanticNode, TreeUpdate } from '../protocol.ts';
import type { App } from './app.ts';

interface Item {
  id: NodeId;
  text: string;
  done: boolean;
}

/** id と件数は性能の計測用 (既定は 'todo' と 3 件) */
export function createTodoApp(opts: { id?: string; items?: number } = {}): App {
  const appId = opts.id ?? 'todo';
  let emit: (u: TreeUpdate) => void = () => {};
  let items: Item[] = [
    { id: 'i1', text: '牛乳を買う', done: false },
    { id: 'i2', text: '請求書を送る', done: true },
    { id: 'i3', text: 'PR をレビューする', done: false },
  ];
  for (let i = items.length + 1; i <= (opts.items ?? 3); i++) items.push({ id: `i${i}`, text: `タスク ${i}`, done: i % 3 === 0 });
  let seq = items.length + 1;
  let draft = '';
  let focus: NodeId | null = null;
  let sent = new Set<NodeId>();

  const publish = (root?: string) => {
    const left = items.filter((i) => !i.done).length;
    const nodes: SemanticNode[] = [
      { id: 'root', role: 'x-window', label: appId === 'todo' ? 'Todo' : appId, children: ['title', 'list', 'entry', 'status'] },
      { id: 'title', role: 'heading', label: 'やること' },
      { id: 'list', role: 'list', label: 'タスク', children: items.map((i) => i.id) },
      ...items.map((i): SemanticNode => ({ id: i.id, role: 'checkbox', label: i.text, checked: i.done, actions: ['click', 'focus'] })),
      { id: 'entry', role: 'group', label: '追加', children: ['input', 'add'] },
      { id: 'input', role: 'textbox', label: '新しいタスク', value: draft, actions: ['focus', 'setValue', 'replaceSelectedText'] },
      { id: 'add', role: 'button', label: '追加', actions: ['click', 'focus'] },
      { id: 'status', role: 'paragraph', label: `残り ${left} 件` },
    ];
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
      else if (req.target === 'input') {
        if (req.action === 'setValue') draft = req.data ?? '';
        if (req.action === 'replaceSelectedText') draft += req.data ?? ''; // キャレットは常に末尾 (試作の簡略化)
      } else if (req.target === 'add' && req.action === 'click') {
        const text = draft.trim();
        if (text) items = [...items, { id: `i${seq++}`, text, done: false }];
        draft = '';
      } else if (req.action === 'click') {
        items = items.map((i) => (i.id === req.target ? { ...i, done: !i.done } : i));
      }
      publish();
    },
  };
}
