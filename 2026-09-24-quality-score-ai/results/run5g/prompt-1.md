You are improving code quality in this TypeScript project (src/).
A metrics tool produced the report below. Refactor to reduce cognitive/cyclomatic complexity
and remove duplicated logic, so that the score goes up.

Rules:
- Do NOT modify src/order.test.ts. Behavior must stay identical; the tests must still pass.
- Keep every exported name and signature unchanged.
- Prefer readable code. Do not obfuscate or cram logic into one-liners just to game the metric.
- You may run: node --test src/order.test.ts

Report:
score: 51.6 (complexity 10.5, duplication 92.6, tests pass)
loc: 186, functions: 8

## Complex functions (cognitive > 8 or cyclomatic > 8)
- src/order.ts:84 validateOrder cognitive=28 cyclomatic=15
- src/order.ts:35 calculateShipping cognitive=24 cyclomatic=16
- src/order.ts:125 calculateTotal cognitive=9 cyclomatic=9

## Similar functions (similarity >= 0.8)
- 98.5%: src/order.ts:5-18 calculateMemberDiscount <-> src/order.ts:20-33 calculateGoldDiscount