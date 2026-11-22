// ④ Expand: Semantic Tree + template -> View Tree。決定的で、LLM を呼ばない。
import { WIDGET_ROLES, defaultTemplate } from './catalog.ts';
import type { BoxStyle, ViewTemplate, WidgetKind } from './catalog.ts';
import type { ColorToken, NodeId, SemanticNode } from './protocol.ts';
import { signature } from './signature.ts';
import type { SemanticTree } from './tree.ts';

export type ViewNode =
  | { kind: 'box'; style: BoxStyle; children: ViewNode[] }
  | { kind: 'text'; str: string; size: number; weight: number; color: ColorToken }
  | { kind: 'widget'; widget: WidgetKind; node: SemanticNode };

export interface TemplateChoice {
  template: ViewTemplate;
  from: 'cache' | 'default';
}
export type TemplateSource = (node: SemanticNode, sig: string) => TemplateChoice;

export interface ExpandIssue {
  node: NodeId;
  sig: string;
  reason: string;
}

class TemplateError extends Error {}

export function expandTree(tree: SemanticTree, source: TemplateSource): { view: ViewNode; issues: ExpandIssue[] } {
  const issues: ExpandIssue[] = [];

  const expandNode = (node: SemanticNode): ViewNode[] => {
    const sig = signature(tree, node);
    const choice = source(node, sig);
    if (choice.from === 'default') return expandChecked(node, choice.template);
    try {
      return expandChecked(node, choice.template);
    } catch (e) {
      if (!(e instanceof TemplateError)) throw e;
      // 不変条件を破る template は使わず、既定レンダラに戻す
      issues.push({ node: node.id, sig, reason: e.message });
      return expandChecked(node, defaultTemplate(node));
    }
  };

  const expandChecked = (node: SemanticNode, t: ViewTemplate): ViewNode[] => {
    const out = expandT(t, node, false);
    checkCoverage(node, out);
    return out;
  };

  const expandT = (t: ViewTemplate, ctx: SemanticNode, inEach: boolean): ViewNode[] => {
    switch (t.type) {
      case 'box': {
        const { type: _type, children, ...style } = t;
        return [{ kind: 'box', style, children: children.flatMap((c) => expandT(c, ctx, inEach)) }];
      }
      case 'text': {
        let str: string;
        // 固定の文字列 (template の文字列と format の地の文) はアプリが言っていない文。誤誘導に使わせない
        const literal = typeof t.text === 'string' ? t.text : (t.text.format ?? '{}').replace('{}', '');
        const untrusted = literal.trim() !== '';
        if (untrusted && hasSensitive(ctx)) throw new TemplateError('固定の文字列は sensitive な操作を含む部分では使えない');
        if (typeof t.text === 'string') str = t.text;
        else {
          const raw =
            t.text.bind === 'label' ? (ctx.label ?? '') : t.text.bind === 'value' ? (ctx.value ?? '') : String(ctx.children?.length ?? 0);
          // 置換文字列の $& などを解釈させない (値をそのまま入れる)
          str = t.text.format ? t.text.format.replace('{}', () => raw) : raw;
        }
        // 固定の文字を含む文は強調できない (muted、太字なし)
        return [{ kind: 'text', str, size: t.size ?? 14, weight: untrusted ? Math.min(t.weight ?? 400, 400) : (t.weight ?? 400), color: untrusted ? 'muted' : (t.color ?? 'text') }];
      }
      case 'widget':
        if (!WIDGET_ROLES[t.kind].includes(ctx.role)) throw new TemplateError(`widget ${t.kind} は role ${ctx.role} に結びつけられない`);
        return [{ kind: 'widget', widget: t.kind, node: ctx }];
      case 'children':
        return tree.children(ctx).flatMap(expandNode);
      case 'each':
        return tree.children(ctx).flatMap((c) => expandT(t.item, c, true));
      case 'self':
        if (!inEach) throw new TemplateError('self は each の中だけで使える');
        return expandNode(ctx);
    }
  };

  const hasSensitive = (n: SemanticNode): boolean => !!n.sensitive || tree.children(n).some(hasSensitive);

  /** 部分木の操作できるノードが、ちょうど 1 回ずつ widget になっていること */
  const checkCoverage = (node: SemanticNode, out: ViewNode[]) => {
    const seen = new Map<NodeId, number>();
    const visit = (v: ViewNode) => {
      if (v.kind === 'widget') seen.set(v.node.id, (seen.get(v.node.id) ?? 0) + 1);
      if (v.kind === 'box') v.children.forEach(visit);
    };
    out.forEach(visit);
    const need: SemanticNode[] = [];
    const walk = (n: SemanticNode) => {
      if ((n.actions?.length ?? 0) > 0 && !n.disabled) need.push(n);
      tree.children(n).forEach(walk);
    };
    walk(node);
    for (const n of need) {
      const c = seen.get(n.id) ?? 0;
      if (c === 0) throw new TemplateError(`操作できるノード ${n.id} (${n.role} "${n.label ?? ''}") が view にない`);
      if (c > 1) throw new TemplateError(`ノード ${n.id} が ${c} 回描かれている`);
    }
  };

  const root = tree.root ? tree.get(tree.root) : undefined;
  if (!root) throw new Error('tree has no root');
  const nodes = expandNode(root);
  const view: ViewNode = nodes.length === 1 ? nodes[0] : { kind: 'box', style: { axis: 'vertical' }, children: nodes };
  const unique = [...new Map(issues.map((i) => [`${i.node}\n${i.reason}`, i])).values()];
  return { view, issues: unique };
}
