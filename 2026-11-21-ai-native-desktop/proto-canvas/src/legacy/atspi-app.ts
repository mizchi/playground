// 既存アプリを試作の App として扱う (Node 専用)。AT-SPI のツリーを意味ツリーとして出し、P2 の操作を AT-SPI の操作に変える。
// 権限 (人 / agent、提案と承認、stale の検出) は shell がそのまま適用する。
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import type { App } from '../apps/app.ts';
import type { ActionRequest, NodeId, TreeUpdate } from '../protocol.ts';
import { PRESS_ACTIONS, mapAtspi } from './atspi.ts';
import type { AtspiNode, MappedTree } from './atspi.ts';

const BRIDGE = path.join(import.meta.dirname, '..', '..', 'legacy', 'atspi_bridge.py');
const PY = process.env.ATSPI_PYTHON ?? '/usr/bin/python3.12';
const sleep = (ms: number) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

export function createAtspiApp(id: string, atspiName: string): App & { refresh(): void; raw(): AtspiNode } {
  let emit: (u: TreeUpdate) => void = () => {};
  let mapped: MappedTree;
  let raw: AtspiNode;
  let focus: NodeId | null = null;
  let sent = new Set<string>();
  const bridge = (...args: string[]) => JSON.parse(execFileSync(PY, [BRIDGE, ...args], { encoding: 'utf8' }));
  const publish = (root?: boolean) => {
    raw = bridge('dump', atspiName) as AtspiNode;
    mapped = mapAtspi(raw);
    const ids = new Set(mapped.nodes.map((n) => n.id));
    const remove = [...sent].filter((x) => !ids.has(x));
    sent = ids;
    if (focus && !ids.has(focus)) focus = null;
    emit({ t: 'tree-update', app: id, root: root ? mapped.root : mapped.root, focus, nodes: mapped.nodes, remove });
  };
  const rawAt = (p: string) => {
    let n = raw;
    for (const i of p.split('.').filter(Boolean)) n = n.children.find((c) => c.path === (n.path ? `${n.path}.${i}` : i))!;
    return n;
  };
  return {
    id,
    start(e) {
      emit = e;
      publish(true);
    },
    onAction(req: ActionRequest) {
      const p = mapped.paths[req.target];
      if (p === undefined) return;
      if (req.action === 'focus') focus = req.target;
      else if (req.action === 'click') {
        const act = PRESS_ACTIONS.find((a) => rawAt(p).actions.includes(a));
        if (act) bridge('action', atspiName, p, act);
      } else if (req.action === 'increment' || req.action === 'decrement') {
        const v = rawAt(p).value;
        if (v) {
          const step = v.step > 0 ? v.step : (v.max - v.min) / 20 || 1;
          const next = Math.min(v.max, Math.max(v.min, v.current + (req.action === 'increment' ? step : -step)));
          bridge('set-value', atspiName, p, String(next));
        }
      } else if (req.action === 'setValue') bridge('set-text', atspiName, p, req.data ?? '');
      else if (req.action === 'replaceSelectedText') bridge('set-text', atspiName, p, (rawAt(p).text ?? '') + (req.data ?? ''));
      sleep(150); // 既存アプリが自分のイベントループで処理するのを待つ
      publish();
    },
    refresh: () => publish(),
    raw: () => raw,
  };
}
