**VERDICT: same** — genuine improvements are bundled with a couple of refactors that look like they're optimizing for a complexity/duplication scanner rather than for a human reader.

**Real improvements:**
- `validateOrder` split into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` — each is small, single-purpose, independently testable. Net win.
- `calculateSubtotal`, `hasFreeShipping`, `calculateTierDiscount`, `calculateCouponDiscount`, `roundCurrency` — clean extractions that make `calculateTotal`/`calculateShipping` read as a straight-line sequence of named steps. Net win.
- `CATEGORY_WEIGHT_PER_UNIT` lookup for item weight — a plain, order-independent map replacing an if/else chain. Legitimately clearer.

**Looks like metric-gaming, not readability work:**
- `MEMBER_DISCOUNT_TIERS`/`GOLD_DISCOUNT_TIERS` + `discountRateByYears` using `tiers.find(t => years >= t.minYears)`. This only works because the array happens to be sorted **descending** by `minYears`. That invariant is invisible — nothing in the type system or the code enforces it, and a future edit that appends a tier or reorders the array for readability will silently produce wrong discount rates. The original nested if/else was strictly more auditable at a glance than "trust the array order."
- `SHIPPING_RATE_TIERS` does the same trick but with `weight < t.belowWeight` over an **ascending**-sorted array, plus a separate `SHIPPING_RATE_OVER` fallback object instead of just a final `Infinity` row. So the file now has two "tier table" patterns with *opposite* ordering conventions and inconsistent fallback handling — that's more cognitive load than the original explicit `if (weight < 1) ... else if (weight < 5) ...` chain, not less.
- Net effect: cyclomatic-complexity and duplicate-branch counts go down (fewer `if/else` tokens, less obviously repeated code), but the actual logic didn't get simpler — it got moved into implicit, unenforced data invariants (sort order, key naming like `minYears` vs `belowWeight`) that a linter can't check and a reviewer can easily miss.

Net: roughly a wash. The validation decomposition and simple lookups are worth keeping; the two tier-table/`find` conversions should be reverted to explicit conditionals (or at minimum sorted/validated with a comment or an assertion, and made consistent with each other) unless there's a concrete need for runtime-configurable tiers.
