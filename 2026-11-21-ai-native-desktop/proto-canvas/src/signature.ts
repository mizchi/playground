import type { SemanticNode } from './protocol.ts';
import type { SemanticTree } from './tree.ts';

/**
 * 構造シグネチャ。role・action の集合・形の旗だけを使い、値とラベルは含めない。
 * 同じシグネチャの子が続く部分は list-of(...) に畳むので、件数が変わっても同じになる。
 */
export function signature(tree: SemanticTree, node: SemanticNode): string {
  const own: string[] = [node.role, ...[...(node.actions ?? [])].sort()];
  if (node.multiline) own.push('multiline');
  const head = own.join('|');
  const kids = tree.children(node).map((c) => signature(tree, c));
  if (kids.length === 0) return head;
  const folded: string[] = [];
  for (let i = 0; i < kids.length; ) {
    let j = i;
    while (j < kids.length && kids[j] === kids[i]) j++;
    folded.push(j - i > 1 || node.role === 'list' ? `list-of(${kids[i]})` : kids[i]);
    i = j;
  }
  return `${head}[${folded.join(',')}]`;
}

/** 表示・保存用の短いハッシュ (FNV-1a 32bit) */
export function shortHash(s: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h.toString(16).padStart(8, '0');
}
