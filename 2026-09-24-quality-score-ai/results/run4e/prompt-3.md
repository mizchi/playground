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

### Rejected attempt 2
Reason: judge: worse/worse/worse

Reviewer 1:
VERDICT: worse

Several extractions are genuine improvements — `round2`, `subtotalOf`, `totalWeight`, `hasFreeShipping`, and splitting `validateOrder` into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` all give names to real concepts and are easy to follow in isolation.

But the two most important pieces of business logic — the tiered discount rates and the tiered shipping rates — got worse. `tierDiscountRate(years, atLeast5, atLeast2, otherwise)` and `weightBasedRate(weight, under1, under5, under20, atLeast20)` replace two clear, self-contained if/else chains with a generic helper called via **unlabeled positional numeric arguments**:

```ts
tierDiscountRate(user.years, 0.1, 0.05, 0.02)      // gold vs member? which bucket is which?
weightBasedRate(weight, 300, 600, 1200, 3000)      // JP rates
```

At the call site you can no longer tell what `0.1` or `1200` means without jumping to the helper's parameter names and mentally re-mapping. In the original code, `rate = 0.1;` sitting right under `if (user.years >= 5)` was self-documenting; now that fact is destroyed and reconstructed through positional plumbing. Since all four/five parameters are the same type (`number`), TypeScript gives zero protection against transposing two rates when someone edits this later — a real correctness risk in pricing code, which is about the worst place to introduce it.

This is a textbook example of **gaming a duplication/complexity metric**: the "duplicated" if/else structure across `calculateMemberDiscount`/`calculateGoldDiscount` and the two shipping-rate branches gets collapsed into a single small function to shrink duplicate-line and cyclomatic-complexity counts, but nothing about the domain logic was actually shared (member and gold discounts are conceptually independent policies that happen to share a *shape*, not a *rule*). The result reads worse and is more error-prone than the "duplicate" code it replaced.

Net effect: the validation and subtotal/shipping-eli

Reviewer 2:
**VERDICT: worse**

The validation split (`validateUser`/`validateItem`/`validateItems`/`validateCoupon`) and the dedup of subtotal/weight computation into `subtotalOf`/`totalWeight` are genuine, sensible extractions — they remove real duplicate loops and read fine standalone. But the two most business-critical pieces of logic — discount tiers and shipping-rate tiers — were refactored in a way that hurts readability more than it helps:

- `tierDiscountRate(years, atLeast5, atLeast2, otherwise)` and `weightBasedRate(weight, under1, under5, under20, atLeast20)` take **unlabeled positional numeric arguments**. Call sites like `tierDiscountRate(user.years, 0.1, 0.05, 0.02)` and `weightBasedRate(weight, 300, 600, 1200, 3000)` no longer show which number applies to which condition — you have to jump to the helper's signature and mentally zip the parameter names against the call-site positions to recover what the original nested `if/else` stated directly and locally (`years >= 5 → rate = 0.1`). This is the classic "collapse near-duplicate branches into one parameterized function" move that shrinks a duplication/complexity score while actually making the domain logic harder to verify at a glance — a bug in argument order (e.g. swapping `atLeast5`/`atLeast2`) would be a silent, easy-to-miss error that the old explicit branches couldn't produce.
- These two helpers only have two call sites each, so they don't even buy meaningful reuse — they exist to merge two structurally-similar `if/else if/else` chains into one generic function, which is a strong sign the refactor was optimizing a duplication/complexity metric rather than clarity.
- The function count nearly triples (5 → 12+), fragmenting logic that was previously readable top-to-bottom in each function; a reader now has to traverse several one-line indirections (`discountFor` → `calculateMemberDiscount`/`calculateGoldDiscount` → `tierDiscountRate`) to reconstruct a single business rule.

Net effect: the pieces that were g

Reviewer 3:
**VERDICT: worse** — the validation/subtotal/coupon decomposition is a genuine improvement, but it's undercut by two helpers that look like they exist to shrink a duplication/complexity score rather than to aid a reader.

Specifics:

**Smells like metric-gaming:**
- `tierDiscountRate(years, atLeast5, atLeast2, otherwise)` and `weightBasedRate(weight, under1, under5, under20, atLeast20)` collapse two *semantically different* if/else cascades (member rates vs. gold rates; JP vs. non-JP shipping tiers) into one shared function purely because they had the same *shape*. The thresholds (`>=5`, `>=2`, `<1`, `<5`, `<20`) are hardcoded inside the helper, but the actual values are passed positionally at each call site:
  ```ts
  tierDiscountRate(user.years, 0.1, 0.05, 0.02)
  weightBasedRate(weight, 300, 600, 1200, 3000)
  ```
  A reader can no longer tell what `0.05` or `1200` means without jumping to the function signature and counting positions. This is strictly worse than the original named `if (years >= 5) rate = 0.1;` — it trades self-documenting branches for anonymous numeric slots, and it's a textbook way to make a duplication checker happy (two blocks now "reuse" one function) while making the code harder to audit or safely modify (swap two args and nothing catches it, since they're all `number`).
- `discountFor` re-implements the same tier check (`"member"` / `"gold"`) that `calculateMemberDiscount`/`calculateGoldDiscount` already do internally — it doesn't remove duplication, it adds a third place encoding "which function handles which tier."

**Genuine improvements:**
- `round2`, `subtotalOf`, `totalWeight`, `hasFreeShipping`, and the `validateUser`/`validateItem`/`validateItems`/`validateCoupon` split are legitimate, well-named extractions that preserve behavior and make `validateOrder`/`calculateShipping` read top-down.

Net effect: the file trades a few real wins for two "reusable" helpers whose reuse is only structural (same control-flow shape), not semantic —