## Review

**Genuine improvements:**
- `discountRateForYears` + `MEMBER_DISCOUNT_TIERS`/`GOLD_DISCOUNT_TIERS` — this is real deduplication. The two nearly-identical if/else rate ladders become one function driven by data tables. Adding a new tier or a third membership level is now a one-line table edit instead of touching branching logic in two places. Same for `baseShippingRate` + `SHIPPING_RATES_JP`/`SHIPPING_RATES_INTL` — a real win.
- `sumSubtotal`/`roundMoney` — subtotal was computed twice in the original (`calculateShipping` and `calculateTotal`); consolidating it is a legitimate fix, not just cosmetic.
- `ITEM_WEIGHT_PER_UNIT` lookup table is cleaner than the if/else chain and correctly preserves the `?? 1` default.

**Looks like metric gaming:**
- `tierDiscount`, `couponDiscountAmount`, `hasFreeShipping`, `validateUser`, `validateCoupon` are each called exactly once. They don't eliminate duplication — they just relocate a branch that was previously inline into its own top-level function. This is the classic pattern of chopping a function into pieces solely to lower per-function cyclomatic complexity / line count, not to create reusable or independently meaningful units.
- `tierDiscount` is especially redundant: it re-checks `user.tier === "member" | "gold"` to decide which function to call, even though `calculateMemberDiscount`/`calculateGoldDiscount` already internally check the same tier and no-op otherwise. The tier check now effectively exists in two places for one decision.
- Net effect: tracing "how is the total computed" now requires jumping through `calculateTotal → tierDiscount → calculateMemberDiscount → discountRateForYears → MEMBER_DISCOUNT_TIERS` and `calculateTotal → couponDiscountAmount`, versus the original's single linear function. That's a lot of indirection for logic that was a few lines of inline branching.
- `validateOrder` → `validateUser`/`validateItems`/`validateCoupon` is a smaller version of the same pattern; defensible as "separate concerns," but each piece is trivial and used once, so it mainly shrinks `validateOrder`'s own line/branch count rather than improving comprehension.

**Net assessment:** the tier/rate tables are a legitimate, valuable refactor that will make future changes easier. But roughly half the extractions (the single-use wrappers around `calculateTotal`/`calculateShipping`/`validateOrder`) add navigation overhead without corresponding benefit — they read as complexity-metric gaming rather than genuine simplification. The two effects roughly cancel out.

VERDICT: same — the shipping/discount tier tables are a real improvement, but it's offset by several single-use wrapper functions that fragment previously-linear flows (`calculateTotal`, `calculateShipping`, `validateOrder`) purely to shrink per-function complexity/line counts, without adding real reuse or clarity.
