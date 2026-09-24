## Review

**Genuine wins:**
- `subtotalOf`, `round2`, and the `report.ts` `summarizeBy` extraction eliminate real, exact duplication (three near-identical loops collapsed into one). Low risk, clear win.
- `validateOrder` splitting into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` follows natural sub-schema boundaries and preserves behavior (including the early-return-on-missing-user semantics). Each piece is now independently testable.
- Shipping/discount duplication between JP/Other and member/gold was real duplication — worth deduplicating.

**Concerns — this is where it starts to look metric-driven rather than reader-driven:**

1. **`rateForYears`/`baseShippingFee` trade explicit, locally-verifiable conditionals for order-dependent table lookups.** The original `if (years >= 5) ... else if (years >= 2) ...` is self-evidently correct just by reading it top to bottom. The new version requires knowing that `MEMBER_RATES`/`GOLD_RATES` must stay sorted **descending** for `.find()`'s first-match-wins semantics to work, while `SHIPPING_RATES_*` must stay sorted **ascending** for the same `.find()` pattern to work correctly. Nothing in the types enforces this ordering — a future contributor inserting a new tier in the "obvious" position (e.g., appending) can silently break the logic with no compiler or test error. That's a real fragility the original code didn't have, and it's a classic pattern for scoring well on cyclomatic-complexity/duplication tools while making a human's job harder (execute algorithm-against-data instead of reading straight-line logic).

2. **`calculateTotal`'s discount computation is cute rather than clear:**
   ```ts
   const discount = calculateMemberDiscount(order.user, subtotal) + calculateGoldDiscount(order.user, subtotal);
   ```
   This only works because `tierDiscount` internally guards on `user.tier === tier`, so exactly one term is nonzero. The original `if (tier === "member") ... else if (tier === "gold") ...` made the mutual exclusivity visible at the call site. The new version hides that invariant inside two separate function bodies the reader must open to confirm summing is safe. Fewer branches, same behavior, less self-evident.

3. **Indirection count is high for the value delivered.** `tierDiscount`, `rateForYears`, `itemWeight`, `totalWeight`, `baseShippingFee`, `hasFreeShipping` — six extra hops to trace "what does a 6-year gold member in Germany pay for shipping" versus reading two adjacent if-chains before. Each function is individually trivial, which is exactly the profile of a refactor aimed at lowering per-function complexity scores rather than genuinely aiding comprehension.

VERDICT: same

Real duplication (report.ts, subtotal/round2, validation split) was legitimately removed, but the rate/fee table-plus-`.find()` pattern replaces self-evident conditionals with order-dependent lookups that aren't guarded by types or comments, and the discount-summation trick in `calculateTotal` hides a mutual-exclusivity invariant. Those two things roughly offset the genuine gains — net readability is a wash, and the table-lookup pattern in particular has the signature of being shaped for a complexity/duplication scanner rather than for the next engineer who has to add a shipping tier.
