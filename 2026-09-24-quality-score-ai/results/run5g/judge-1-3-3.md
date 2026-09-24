**VERDICT: better**

This is a genuine refactor, not metric theater. The extracted functions correspond to real, previously-duplicated logic:

- `roundMoney` — the same `Math.round(x*100)/100` was inlined three times.
- `orderSubtotal` — subtotal computation was duplicated in `calculateShipping`'s member-tier check and again in `calculateTotal`.
- `rateForYears` — collapses two structurally identical if/else-if ladders (member vs. gold tiers) into one data table, which is the right way to kill that duplication.
- `hasFreeShipping` / `baseShippingFee` / `itemWeight` / `totalWeight` — separates "how much does this order weigh," "what's the base fee for that weight/country," and "is shipping free," which were previously tangled into one long function mixing three concerns.
- `validateUser` / `validateItem` / `validateItems` / `validateCoupon` — splits validation by the thing being validated, which matches how you'd actually want to extend or test this (e.g., unit-test coupon rules in isolation).
- `tierDiscount` / `couponDiscountFor` — give names to branches that were previously anonymous inline logic in `calculateTotal`, making that function read as a straight-line pipeline (subtotal → discount → coupon → shipping → total) instead of a wall of mutation.

I traced the behavior through (rounding, free-shipping thresholds, furniture qty cap, coupon rules, `Math.max(0, total)`) and didn't find a semantic change — it looks behavior-preserving.

**Minor cost, not gaming:** the tier/bracket tables (`rateForYears([[5,0.1],[2,0.05],[0,0.02]])`, `SHIPPING_RATES` brackets) trade explicit, self-documenting if/else for positional tuples. The labeled tuple types (`minYears`, `rate` / `ceiling`, `fee`) mitigate this, but a reader still has to hold "first match wins, table is descending" in their head instead of reading it off an if-chain. That's a reasonable trade for removing real duplication, not an attempt to game a metric — there's no sign of splitting logic into trivial wrappers just to lower per-function branch counts; every new function is independently meaningful and independently testable.
