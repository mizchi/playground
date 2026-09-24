**VERDICT: better** — the refactor is net positive, but two of the "table-driven" extractions look like they exist to shrink complexity/duplication metrics rather than to make the logic clearer.

Genuine improvements:
- `roundCurrency`, `calculateSubtotal` — real duplication removed, trivial and safe.
- `validateUser`/`validateItem`/`validateItems`/`validateCoupon` — the original `validateOrder` was one large nested function; splitting it into single-purpose validators that each return `string[]` and get spread together is a clear win, and matches how the errors are actually composed.
- `hasFreeShipping` — turning "gold is always free, member is free over ¥10000" into a named predicate makes `calculateShipping` read top-to-bottom without mentally executing the branches.
- `calculateTierDiscount`/`calculateCouponDiscount` pulled out of `calculateTotal` — good, `calculateTotal` is now a readable summary of the pricing pipeline instead of one big procedural block.

Things that look like metric gaming rather than real simplification:
- `discountRateByYears` + `MEMBER_DISCOUNT_TIERS`/`GOLD_DISCOUNT_TIERS`: turning a 3-branch `if/else if` into a generic `{minYears, rate}[]` + `.find()` doesn't reduce real complexity — it *hides* the branching in data and introduces an **unenforced invariant** (the array must stay sorted descending by `minYears`, or `.find` silently returns the wrong tier). The original if/else was self-documenting and impossible to get wrong by reordering. This reads like an attempt to lower a branch-count/cyclomatic-complexity score.
- `SHIPPING_RATE_TIERS` + separate `SHIPPING_RATE_OVER` fallback: same ascending-order invariant risk, and it's structurally inconsistent — every tier has `belowWeight`, but the "over" case is a bespoke object with no `belowWeight`, so the abstraction doesn't even uniformly apply to itself. It saves lines but adds a shape you have to special-case to understand.

Net effect: a future engineer adding a new discount or shipping band now has to know an implicit sort-order rule instead of just inserting an `else if`. That's a real maintainability regression hiding inside an otherwise good cleanup. I'd keep the validation/subtotal/shipping-predicate extractions and revert the two generic tier-lookup abstractions back to explicit if/else chains (or at least add a comment/assertion documenting the required sort order and a shared shape for the fallback).
