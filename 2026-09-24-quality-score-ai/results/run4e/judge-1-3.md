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

If someone later inserts a bracket out of order, or add a JP-adjacent country with different semantics, this will silently misbehave with no compiler or test signal pointing at the cause — the original inline chain couldn't have that failure mode.

**Recommendation:** keep the validation and shipping-function decomposition, but revert `rateForYears`/`baseShippingRate` back to explicit if/else-if chains (or at least sort-guard/comment the array's required ordering) — the abstraction cost isn't paid for by two call sites each.
