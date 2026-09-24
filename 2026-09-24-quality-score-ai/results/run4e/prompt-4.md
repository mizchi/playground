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

### Rejected attempt 2
Reason: judge: same/same/same

Reviewer 1:
**VERDICT: same**

The diff is a mixed bag: some extractions are genuine DRY wins, others look like mechanical decomposition done to shrink per-function complexity/line-count numbers rather than to help a reader.

**Genuine improvements:**
- `round2` — real duplication removed (was copy-pasted 3x).
- `subtotalOf` — real duplication removed (was copy-pasted 2x, including inside `calculateShipping`).
- `validateUser` / `validateItem` / `validateItems` / `validateCoupon` — these map to real, independent concerns (different entities), and `validateItem` is genuinely reused in a loop. The final `validateOrder` reading as a flat spread of three validators is arguably clearer than the original nested if/else pyramid.

**Looks like metric gaming, not readability:**
- `itemWeight`, `totalWeight`, `baseShippingRate`, and `hasFreeShipping` are each called from exactly one call site. Splitting a single linear, easy-to-trace function (`calculateShipping`) into five tiny functions doesn't add reuse or reduce total branching — it just redistributes the same decision points across more, smaller functions so each one scores low on a per-function cyclomatic/cognitive-complexity check. A reader who wants to understand "how is shipping computed" now has to jump through four definitions instead of reading top-to-bottom. This is the classic "salami-slice a function until the linter stops complaining" pattern — total complexity is unchanged, it's just hidden by fragmentation.
- Similarly, `itemWeight` being pulled out of `totalWeight`'s loop body adds a layer of indirection for a single-use, four-branch if/else that was perfectly readable inline.

Net effect: the validation and rounding/subtotal changes are real improvements; the shipping decomposition adds indirection without payoff. They roughly cancel out, so the change is not a clear net gain in readability/maintainability even though it would likely score better on cyclomatic-complexity-per-function or duplication metrics.

Reviewer 2:
VERDICT: same

**Genuine wins:** `round2` (deduped 3 identical lines) and `subtotalOf` (deduped an item-sum loop used twice) are legitimate DRY extractions — the duplicated code was byte-identical and the extraction removes real risk of the copies drifting. The `validateUser`/`validateItems`/`validateCoupon` split maps cleanly onto the shape of `Order` (user, items, coupon) and composing them via `validateOrder`'s array spread reads better than the original nested if/else — that's a real improvement.

**Looks like metric gaming:** `calculateShipping` was one cohesive, linearly-readable function (compute weight → look up base rate → apply free-shipping override). It's now scattered across four separate functions (`itemWeight`, `totalWeight`, `baseShippingRate`, `hasFreeShipping`) that a reader has to jump between to reconstruct the same three-step story. Each function individually has trivial cyclomatic complexity (which is exactly what a complexity linter rewards), but nothing here was actually duplicated logic — it's a single algorithm sliced into fragments for the sake of fragment count, not comprehension.

More telling: the refactor fixes only *literal, line-for-line* duplication (the `round2` calls, the subtotal loop) while leaving the real semantic duplication untouched — `calculateMemberDiscount`/`calculateGoldDiscount` are still near-identical rate tables, and the JP/non-JP branches in `baseShippingRate` are still a copy-pasted threshold ladder that could trivially be table-driven. If the goal were genuine simplification rather than satisfying a duplication scanner, those would be the first things collapsed. Instead the diff optimizes for whatever a naive duplicate/complexity detector flags at the token level, not for a human tracing the shipping or discount logic end to end.

Net effect: neither clearly better nor worse — a few real improvements offset by unnecessary indirection introduced specifically where it would move the needle on a complexity/duplicati

Reviewer 3:
**VERDICT: same** — genuine improvements are mixed in with several single-use extractions that look aimed at lowering a per-function complexity/LOC score rather than improving clarity.

**Real wins:**
- `round2` and `subtotalOf` are used from ≥2 call sites each and are self-explanatory — legitimate duplication removal.
- Flattening `validateItem`'s nested `if/else { if ... }` into a flat `else if` chain is a genuine readability improvement over the original nested block.

**Signs of metric gaming (complexity/duplication pushed down, not removed):**
- `itemWeight(item)` is called from exactly one place (`totalWeight`). Splitting a single per-item weight rule out of its only caller doesn't reduce real complexity — it just moves the same `if/else` chain into a function with one caller, presumably to shrink `calculateShipping`'s cyclomatic complexity/line count on paper.
- `validateUser` / `validateItem` / `validateItems` / `validateCoupon` are each called exactly once, from `validateOrder`. The original function read top-to-bottom as one cohesive validation pass with clear sections; now understanding "how is an order validated" requires jumping across 4 extra functions with no reuse benefit. This is the classic pattern of decomposing a function until every branch's complexity is "someone else's function," which drives down average cyclomatic complexity / function-length metrics without making the *logic* any simpler.
- `hasFreeShipping` + `baseShippingRate` fragment what was one shipping calculation with two clearly labeled phases (weight → base rate, then tier override) into four indirection layers (`itemWeight → totalWeight`, `hasFreeShipping`, `baseShippingRate`, `calculateShipping`). It's not wrong, but it trades a linear narrative for a scatter of tiny functions you must mentally re-inline to verify the original behavior is preserved (e.g., confirming `subtotalOf` is still only meaningfully used for the `member` tier check).
- Minor behavioral/perf nit introduced 