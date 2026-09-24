// Same behavior written in ways that may hide complexity from the metric.
// Baseline: explicit branches.
export function baseline(t: string, y: number): number {
  if (t === "member") {
    if (y >= 5) return 0.1;
    else if (y >= 2) return 0.05;
    else return 0.02;
  } else if (t === "gold") {
    if (y >= 5) return 0.2;
    else if (y >= 2) return 0.15;
    else return 0.1;
  }
  return 0;
}
// Nested ternaries.
export function ternaries(t: string, y: number): number {
  return t === "member" ? (y >= 5 ? 0.1 : y >= 2 ? 0.05 : 0.02) : t === "gold" ? (y >= 5 ? 0.2 : y >= 2 ? 0.15 : 0.1) : 0;
}
// Short-circuit chains instead of if.
export function shortCircuit(t: string, y: number): number {
  return (t === "member" && ((y >= 5 && 0.1) || (y >= 2 && 0.05) || 0.02)) ||
    (t === "gold" && ((y >= 5 && 0.2) || (y >= 2 && 0.15) || 0.1)) || 0;
}
// Lookup table + find.
const TABLE: Record<string, [number, number][]> = {
  member: [[5, 0.1], [2, 0.05], [0, 0.02]],
  gold: [[5, 0.2], [2, 0.15], [0, 0.1]],
};
export function table(t: string, y: number): number {
  return TABLE[t]?.find(([min]) => y >= min)?.[1] ?? 0;
}
// Split into helper functions (complexity moved, not removed).
function memberRate(y: number) { if (y >= 5) return 0.1; if (y >= 2) return 0.05; return 0.02; }
function goldRate(y: number) { if (y >= 5) return 0.2; if (y >= 2) return 0.15; return 0.1; }
export function split(t: string, y: number): number {
  if (t === "member") return memberRate(y);
  if (t === "gold") return goldRate(y);
  return 0;
}
// Object-literal dispatch with inline closures.
export function dispatch(t: string, y: number): number {
  const h: Record<string, (y: number) => number> = {
    member: (y) => { if (y >= 5) return 0.1; if (y >= 2) return 0.05; return 0.02; },
    gold: (y) => { if (y >= 5) return 0.2; if (y >= 2) return 0.15; return 0.1; },
  };
  return h[t]?.(y) ?? 0;
}
// Math.max/min trickery (branch-free arithmetic).
export function arithmetic(t: string, y: number): number {
  const m = Number(t === "member"), g = Number(t === "gold");
  const step = Number(y >= 2) + Number(y >= 5);
  return m * [0.02, 0.05, 0.1][step] + g * [0.1, 0.15, 0.2][step];
}
