**VERDICT: same** — genuine improvements in some areas are offset by over-abstraction elsewhere that reads like duplication-metric gaming rather than real clarity gains.

**Real improvements:**
- `round2`, `subtotalOf` dedup trivial, uncontroversial repeated logic.
- `validateOrder` → `validateUser`/`validateItems`/`validateItem`/`validateCoupon` is a clean decomposition; each piece is independently readable and the top-level function now reads as a spec.
- `calculateTotal` orchestration (`tierDiscount`, `couponDiscountFor`) makes the main flow scannable as a sequence of named steps instead of one long function.
- `CATEGORY_WEIGHT_PER_UNIT` lookup table for item weight is a legitimate simplification of a 4-branch if/else into a map lookup.

**Looks like metric gaming, not real improvement:**
- `RateTier`/`tieredRate`/`discountForTier` + `MEMBER_RATE_TIERS`/`GOLD_RATE_TIERS`. The original had two near-identical if/else chains — a classic duplicate-block hit for tools like jscpd/SonarQube. Instead of just accepting that two tiny, tier-specific functions are fine, it was generalized into a loop over a data table with a `minYears: 0` fallback sentinel, plus a `discountForTier(user, subtotal, tier, tiers)` wrapper that exists only to do a tier-equality check. To answer "what discount does a 3-year gold member get?" you now traverse three functions and a table instead of reading one if/else. That's indirection added specifically to collapse a duplication count, at the cost of directly readable business rules.
- Same pattern for shipping: `SHIPPING_TIERS_BY_COUNTRY`/`DEFAULT_SHIPPING_TIERS`/`baseShippingCost` replaces two clear if/else chains with a table keyed by country, a non-null-asserted `.find()`, and an `Infinity` sentinel for the last bracket. The `!` assertion and the magic `Infinity` are a real (mild) fragility regression — a data-driven table only pays off when there are many countries or tiers; with one special case (JP) it's ceremony that "hides" duplication rather than removing complexity.

In short: the validation and top-level orchestration refactors are worth keeping, but the tiered-discount and shipping-tier abstractions trade straightforward, locally-readable branching for generic table-driven engines whose main visible benefit is a lower duplication/complexity score, not better human comprehension.
