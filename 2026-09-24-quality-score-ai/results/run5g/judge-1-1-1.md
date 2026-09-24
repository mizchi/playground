**VERDICT: worse**

The module was small (four exported functions, each easy to read top-to-bottom) and it's now fragmented into 16+ named entities, most of them one-line, single-call-site helpers. Understanding `calculateTotal` now requires jumping through `calculateTierDiscount → calculateMemberDiscount/calculateGoldDiscount → rateForYears + MEMBER_RATE_TIERS/GOLD_RATE_TIERS`, and `calculateShipping → calculateOrderWeight/ITEM_UNIT_WEIGHTS + baseShippingFee/SHIPPING_FEE_TIERS`, instead of reading one linear function. That's more indirection for the same amount of actual logic.

**Genuine improvements** (keep these):
- `roundCurrency` and `calculateSubtotal` dedup real repeated code — clean win.
- `validateUser`/`validateItem`/`validateItems`/`validateCoupon` split is a sensible decomposition by concern and the `flatMap` in `validateItems` is idiomatic.
- `ITEM_UNIT_WEIGHTS` as a `Record<string, number>` is a fair simplification of a flat 4-branch if/else into a lookup.

**Looks like metric gaming:**
- `YearsTier`/`WeightTier` as positional tuples (`[minYears, rate]`, `[belowWeight, fee]`) turn a directly-readable if/else-if threshold chain into generic data + a loop (`rateForYears`, `baseShippingFee`). This is the textbook move for shrinking a cyclomatic-complexity count per function: branches get relocated into "data" and a shared generic iterator, but the reader now has to reverse-engineer what a positional tuple means and mentally re-simulate the loop instead of just reading `if (years >= 5) …`. Two near-identical rate tables for a trivial 3-tier lookup used by exactly one caller each is over-abstraction for no reuse payoff beyond the tiny `rateForYears` body.
- `SHIPPING_FEE_TIERS` uses an `Infinity` sentinel plus `tiers.find(...) ?? tiers[tiers.length - 1]` — the `??` fallback is dead code (the `Infinity` tier always matches), kept only to satisfy TypeScript, which is a sign the abstraction is fighting the type system rather than clarifying intent.
- `calculateTierDiscount` and `calculateCouponDiscount` are thin, single-call-site wrappers that don't reduce duplication — they just move `calculateTotal`'s existing branches into extra hops, again reducing that function's local complexity number without reducing real complexity.

Net effect: real duplication (rounding, subtotal, validation) was fixed, but simple, obvious conditionals were replaced with tuple-encoded "generic" lookups that add cognitive overhead without adding actual reuse or flexibility — a pattern consistent with optimizing for a complexity/duplication scanner rather than for the next engineer reading this file.
