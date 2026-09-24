**Assessment**

This is a genuine refactor, not just metric gaming. The extractions map to real duplication that existed in the original:

- `rateByYears` + `MEMBER_TIERS`/`GOLD_TIERS` replace two near-identical nested if/else cascades that differed only in numbers — a real win. Adding a new tier or a third membership level now means editing a table, not copy-pasting a branch.
- `rateByWeight` + `JP_SHIPPING_TIERS`/`INTL_SHIPPING_TIERS` do the same for the shipping cascade.
- `roundMoney` and `subtotalOf` dedupe logic that was copy-pasted 3-4 times.
- Splitting `validateOrder` into `validateUser`/`validateItems`/`validateItem`/`validateCoupon` is a legitimate decomposition along natural boundaries, and `validateItems` correctly preserves the original's early-return-on-empty behavior via `validateItem`'s `flatMap`.

I checked behavioral equivalence on the trickier parts:
- `validateUser`'s `if (!user) return [...]` correctly preserves the original's short-circuit (avoids touching `user.id`/`user.years` on a missing user).
- `rateByWeight`/`rateByYears` depend on tier arrays being pre-sorted (ascending by weight, descending by years) — this is an implicit invariant not enforced by types, so appending a tier out of order would silently produce wrong results. Minor risk, worth a comment or a sort call.

Two mild concerns, short of "gaming":
1. `calculateShipping` now requires hopping across `subtotalOf → hasFreeShipping`, `baseShippingRate → rateByWeight`, `totalWeight → itemWeight` to reconstruct one code path — more fragmented than the original single function, even though each piece reads cleanly in isolation.
2. `tierDiscountFor` re-checks `order.user.tier`, and then `calculateMemberDiscount`/`calculateGoldDiscount` check it again internally — the tier gets tested redundantly rather than dispatched once.

Neither of these looks like an attempt to game a complexity/duplication score (e.g., there's no pointless single-use wrapper created just to shrink one function's cyclomatic count, no re-exported dead code, no artificial merging of unrelated branches to cut line count). The tables and helper extractions all correspond to actual duplicate logic that existed before.

VERDICT: better
Reason: The refactor replaces truly duplicated branch logic (year-tier rates, weight-tier shipping rates, subtotal/rounding, validation clauses) with shared, data-driven helpers, which is a real maintainability improvement — not a superficial metric-gaming exercise — though it adds some navigation overhead in `calculateShipping` and a couple of redundant tier checks.
