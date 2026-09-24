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

Previous attempts were REJECTED and rolled back. Learn from the reviewers' feedback below.
If a reported function is already readable and further change would only add indirection,
leave it as it is: a lower metric score is acceptable, a less readable change is not.
Making no change at all is an acceptable outcome.

### Rejected attempt 1
Reason: judge: better/same/same

Reviewer 1:
**VERDICT: better**

The file is genuinely easier to work with than before, mainly because of the validation and total-calculation split:

- `validateUser`/`validateItem`/`validateItems`/`validateCoupon` replace one deeply-nested function with small, independently readable/testable pieces, and `validateOrder` becomes a one-line composition. Clear win.
- `discountFor`, `couponDiscountFor`, `subtotalOf`, `hasFreeShipping` flatten `calculateTotal`/`calculateShipping` from nested if/else soup into a short, linear sequence of named steps. Clear win.
- `ITEM_WEIGHT_PER_UNIT` as a lookup table is a legitimate simplification of the category if/else chain.

**Metric-gaming smell to flag:** the `YearsBracket`/`rateForYears` abstraction. Two simple, self-contained 3-branch if/else chains (member and gold discount tiers) were turned into a generic "iterate brackets sorted descending, first match wins" loop over two still-separately-hardcoded arrays. This doesn't actually remove duplication (there are still two literal tables to keep in sync if a tier count changes) — it just moves the branching into a loop, which is the classic way to lower a cyclomatic-complexity/duplication score without changing real duplication. It also adds a footgun: the fallback `return 0` at the end of `rateForYears` is unreachable dead code, since every bracket list already includes a `{min: 0, ...}` entry, but a reader has to trace that to be sure.

Worse, it's inconsistent with the shipping bracket abstraction introduced right next to it: `WeightBracket.belowWeight` is checked with `<` and sorted **ascending**, while `YearsBracket.min` is checked with `>=` and sorted **descending**. Same "bracket table" shape, opposite ordering/comparison conventions — a future maintainer editing one pattern by analogy with the other will introduce a bug. That inconsistency is a real maintainability regression the original plain if/else chains didn't have.

So: solid net improvement overall, but the discount-bracket 

Reviewer 2:
VERDICT: same

**What's genuinely better:**
- `rateForYears`/`MEMBER_BRACKETS`/`GOLD_BRACKETS` and `baseShippingRate`/`DOMESTIC_...`/`INTERNATIONAL_...` remove *real* duplication — the member/gold and JP/international logic was copy-pasted with only numbers differing, which is a classic "forgot to update both copies" bug risk. Turning that into data tables + one lookup function is a legitimate improvement, not metric theater.
- `validateOrder` composed from `validateUser`/`validateItems`/`validateCoupon` is a reasonable, self-documenting decomposition.

**What's a wash or worse:**
- `discountFor`, `couponDiscountFor`, and `hasFreeShipping` don't remove any duplication or branching — they just relocate the exact same if/else out of `calculateTotal`/`calculateShipping` into single-call, single-use functions. This is the textbook move for satisfying a per-function cyclomatic-complexity or max-statements linter: each function's own score drops, but the total number of decision points is unchanged and you now have to jump across 3–4 extra function definitions to reconstruct what used to be one linear read. That's a strong "gaming the metric" smell.
- The bracket-table pattern introduces a new, *undocumented* invariant that didn't exist before: `rateForYears` relies on brackets being sorted **descending** by `min` (first match wins on `years >= min`), while `baseShippingRate` relies on brackets being sorted **ascending** by `belowWeight` (first match wins on `weight < belowWeight`). Same shape (`{threshold, rate}[]` + linear scan), opposite ordering convention, no comment or type-level enforcement. A future engineer adding a new bracket to either table has no signal about which order is required — previously the if/else chain made the ordering visually self-evident. This is a real regression in safety that the original explicit chain didn't have.
- Net effect: you've traded "long but linear and obviously-correct" procedural code for "short functions, but you must hold 6–8

Reviewer 3:
**VERDICT: same** — some extractions are genuine wins, but the discount/shipping bracket refactor is a textbook complexity-metric dodge that makes that part of the code worse to read.

**Genuine improvements:**
- `validateUser`/`validateItem`/`validateItems`/`validateCoupon` — clean decomposition, each function is independently understandable, and `validateItems` using `flatMap` is a nice simplification of the manual loop.
- `calculateShipping` split into `hasFreeShipping` / `totalWeight` / `baseShippingRate` — matches how a human would already chunk the logic mentally. Clear win.
- `round2`, `subtotalOf` — trivial, uncontroversial dedup.

**The gaming smell — bracket tables:**
`rateForYears`/`YearsBracket` and `baseShippingRate`/`WeightBracket` replace a plain, self-evidently-correct if/else-if chain with a generic loop over a data array. This is the classic move for beating a cyclomatic-complexity or duplication scanner: it collapses N branches into 1 branch-in-a-loop, and turns two "duplicate" if-chains into calls to one shared function. But it doesn't reduce complexity for a human — it *hides* it:
- Correctness now depends on an unenforced invariant: `MEMBER_BRACKETS`/`GOLD_BRACKETS` must stay sorted descending by `min`, and `DOMESTIC_SHIPPING_BRACKETS`/`INTERNATIONAL_SHIPPING_BRACKETS` must stay sorted ascending by `belowWeight`. The original if/else made the ordering (and thus intent) visually obvious; the table hides it behind a loop with an implicit contract nothing checks or documents.
- Each table has only two call sites with different values — not enough real duplication to justify a generic reusable abstraction. It reads as "extract a helper so the two functions look shorter and less repetitive to a scanner," not "extract a helper because this logic is genuinely reused elsewhere."
- A reader now has to jump between the call site, the table, and the generic loop body to reconstruct what a single flat if/else chain said directly.

If someone later inserts 