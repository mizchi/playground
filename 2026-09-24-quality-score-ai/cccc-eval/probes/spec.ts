// Expected values follow the SonarSource Cognitive Complexity whitepaper (v1.7).
// @expect cognitive=1
export function ifOnly(a: boolean) {
  if (a) return 1; // +1
  return 0;
}
// @expect cognitive=3
export function ifElseIfElse(a: number) {
  if (a > 0) return 1; // +1
  else if (a < 0) return -1; // +1
  else return 0; // +1
}
// @expect cognitive=6
export function nested(xs: number[][]) {
  for (const row of xs) { // +1
    for (const x of row) { // +2 (nesting 1)
      if (x > 0) { // +3 (nesting 2)
        return x;
      }
    }
  }
  return 0;
}
// @expect cognitive=1
export function switchCase(a: number) {
  switch (a) { // +1 for the whole switch
    case 1: return "a";
    case 2: return "b";
    case 3: return "c";
    default: return "z";
  }
}
// @expect cognitive=2
export function logicalSeq(a: boolean, b: boolean, c: boolean, d: boolean) {
  if (a && b && c && d) return 1; // +1 if, +1 for one && sequence
  return 0;
}
// @expect cognitive=4
export function logicalMixed(a: boolean, b: boolean, c: boolean, d: boolean) {
  if (a && b || c && d) return 1; // +1 if, +3 (&&, ||, && sequences)
  return 0;
}
// @expect cognitive=2
export function recursion(n: number): number {
  if (n <= 1) return 1; // +1
  return n * recursion(n - 1); // +1 recursion
}
// @expect cognitive=3
export function tryCatch(f: () => void) {
  try {
    f();
  } catch (e) { // +1
    if (e instanceof Error) { // +2 (nesting 1)
      return e.message;
    }
  }
  return "";
}
// @expect cognitive=4 (whitepaper: the lambda adds nesting and is folded into the parent)
// cccc scores the arrow as its own unit: parent=0, arrow=2 (documented deviation)
export function nestedLambda(xs: number[]) {
  return xs.map((x) => {
    if (x > 0) { // +2 (nesting 1)
      return x;
    }
    return x > -10 ? 0 : -1; // +2 (nesting 1)
  });
}
// @expect cognitive=1
export function ternary(a: boolean) {
  return a ? 1 : 0; // +1
}
// @expect cognitive=7
export function labeledBreak(xs: number[][]) {
  outer: for (const row of xs) { // +1
    for (const x of row) { // +2 (nesting 1)
      if (x) break outer; // +3 (nesting 2), +1 labeled break
    }
  }
}
