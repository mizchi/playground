import type { NodeId, SemanticNode, TreeUpdate } from './protocol.ts';

/** ① Semantic Tree: app ごとの a11y ツリー (正)。 */
export class SemanticTree {
  readonly nodes = new Map<NodeId, SemanticNode>();
  root: NodeId | null = null;
  focus: NodeId | null = null;

  apply(update: TreeUpdate): void {
    // app から来た値は JSON として複製し、共有参照を持たない
    const u = structuredClone(update);
    if (u.root !== undefined) this.root = u.root;
    for (const id of u.remove ?? []) this.nodes.delete(id);
    for (const n of u.nodes) this.nodes.set(n.id, n);
    this.focus = u.focus;
    if (this.root === null) throw new Error('first tree-update must carry root');
  }

  get(id: NodeId): SemanticNode | undefined {
    return this.nodes.get(id);
  }

  children(node: SemanticNode): SemanticNode[] {
    return (node.children ?? []).map((id) => {
      const c = this.nodes.get(id);
      if (!c) throw new Error(`dangling child ${id} of ${node.id}`);
      return c;
    });
  }

  /** 木の順 (= 既定のフォーカス順・読み上げ順) */
  walk(): SemanticNode[] {
    const out: SemanticNode[] = [];
    const visit = (id: NodeId) => {
      const n = this.nodes.get(id);
      if (!n) return;
      out.push(n);
      for (const c of n.children ?? []) visit(c);
    };
    if (this.root) visit(this.root);
    return out;
  }

  /** 操作できるノード。view はこれをすべて出さなければならない。 */
  actionable(): SemanticNode[] {
    return this.walk().filter((n) => (n.actions?.length ?? 0) > 0 && !n.disabled);
  }

  toJSON(): { root: NodeId | null; focus: NodeId | null; nodes: SemanticNode[] } {
    return { root: this.root, focus: this.focus, nodes: this.walk() };
  }
}
