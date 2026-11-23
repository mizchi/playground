// agent の作業場: agent のワークスペースに最初からある、システムのアプリ。
// agent が溜めた変更 (重ねた層の差分) を見せ、人のワークスペースへの持ち込みを提案する。答える (書く) のは人だけ (パネル)。
import type { ServiceHost } from '../platform/services.ts';
import type { ActionRequest, NodeId, SemanticNode, TreeUpdate } from '../protocol.ts';
import type { App } from './app.ts';

export function createAgentDeskApp(id: string, host: ServiceHost): App {
  let emit: (u: TreeUpdate) => void = () => {};
  let status = '';
  let sent = new Set<NodeId>();
  // 文字は折り返さないので、窓の最小幅を押し上げないよう短くする (全文はパネルの持ち込みの提案に出る)
  const short = (v: string | undefined) => (v === undefined ? '(なし)' : v.length > 16 ? `${v.slice(0, 16)}…` : v);
  const publish = (root?: string) => {
    const changes = host.pendingChanges();
    const pending = host.merges.filter((m) => m.status === 'pending');
    const nodes: SemanticNode[] = [
      { id: 'root', role: 'x-window', label: 'agent の作業場', children: ['title', 'about', 'about2', 'changes', 'actions', 'status'] },
      { id: 'title', role: 'heading', label: 'agent のワークスペース' },
      { id: 'about', role: 'paragraph', label: 'ここでの保存は、人のワークスペースに届かない' },
      { id: 'about2', role: 'paragraph', label: '持ち込みを提案し、人が承認したら書く' },
      { id: 'changes', role: 'list', label: '溜まった変更', children: changes.length ? changes.map((_, i) => `c${i}`) : ['none'] },
      ...changes.map((c, i): SemanticNode => ({ id: `c${i}`, role: 'paragraph', label: `${c.app} / ${c.key}: ${short(c.before)} → ${short(c.after)}` })),
      { id: 'actions', role: 'group', label: '持ち込み', children: ['merge', 'discard'] },
      { id: 'merge', role: 'button', label: '持ち込みを提案', actions: ['click', 'focus'], disabled: !changes.length || pending.length > 0 },
      // 捨てるのは取り返しがつかないので、agent からは毎回承認が要る (アプリも agent からは断る)
      { id: 'discard', role: 'button', label: '変更を捨てる', sensitive: true, actions: ['click', 'focus'], disabled: !changes.length },
      { id: 'status', role: 'paragraph', label: status || (pending.length ? `${pending.map((m) => m.id).join(', ')} は人の答えを待っている` : `${changes.length} 件の変更`) },
    ];
    if (!changes.length) nodes.push({ id: 'none', role: 'paragraph', label: 'まだ変更は無い' });
    const ids = new Set(nodes.map((n) => n.id));
    const remove = [...sent].filter((x) => !ids.has(x));
    sent = ids;
    emit({ t: 'tree-update', app: id, root, focus: null, nodes, remove });
  };
  return {
    id,
    start(e) {
      emit = e;
      publish('root');
    },
    onAction(req: ActionRequest) {
      if (req.target === 'merge' && req.action === 'click') {
        const r = host.proposeMerge(req.by === 'agent' ? 'agent が作業場から持ち込みを提案した' : '人が作業場から持ち込みを提案した', req.by);
        status = r.ok ? `${r.merge.id} を提案した。人がパネルで答える` : r.reason;
      } else if (req.target === 'discard' && req.action === 'click') {
        const why = host.discardOverlay(req.by);
        status = why || '変更を捨てた';
      }
      publish();
    },
    /** 重ねた層や持ち込みの状態が変わったとき (プラットフォームが呼ぶ) */
    refresh: () => {
      status = '';
      publish();
    },
  } as App & { refresh(): void };
}
