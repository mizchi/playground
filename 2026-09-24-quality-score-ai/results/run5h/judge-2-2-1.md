VERDICT: better

The refactor is a net improvement, but it's not uniformly clean — one part looks like it optimized for a duplication-detector rather than for a human reader.

**Genuine improvements:**
- `rateForYears(years, TierRate[])` collapsing `calculateMemberDiscount`/`calculateGoldDiscount`'s identical if-elseif ladders into one function driven by a self-descriptive `{minYears, rate}` table is real deduplication — a future tier change is now a one-line data edit instead of a two-place edit.
- `calculateSubtotal` and `calculateWeight` extraction removes genuinely duplicated loops (subtotal was computed inline in both `calculateShipping` and `calculateTotal` before).
- Splitting `validateOrder` into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` turns one function doing three unrelated jobs into a clear composition (`validateOrder` now reads as a checklist). This is a legitimate readability win, and behavior is preserved (early-return-with-single-error semantics for a missing user match the original).
- `calculateTierDiscount`/`calculateCouponDiscount` extraction makes `calculateTotal` read as a clean pipeline (subtotal → discount → coupon → shipping → clamp).

**Smell / apparent metric-gaming:**
- `shippingRateForWeight(weight, rates: number[])` with `SHIPPING_RATES_JP`/`SHIPPING_RATES_INTL` as bare positional number arrays, disambiguated only by the comment `// Rates ordered by ascending weight threshold: [<1, <5, <20, >=20]`. The author clearly knew the better pattern — they used it one screen above for `TierRate` (`{minYears, rate}` objects) — but here reached for magic indices plus a comment instead. That's a classic sign of collapsing two duplicate if-chains to satisfy a duplication-count metric without doing the extra step to make the shared representation self-describing. It should have been `{ maxWeight: number; rate: number }[]` like the discount tiers, needing no comment.
- The overall function count balloons from 4 to ~14 for the same logic. Some of that buys real clarity (validation), but chains like `calculateTotal → calculateTierDiscount → calculateMemberDiscount/calculateGoldDiscount → rateForYears` add three hops of indirection for what was previously a flat, linear read. This pattern (many tiny single-call-site functions) is also consistent with chasing a per-function cyclomatic-complexity ceiling rather than pure readability — it's defensible here but worth watching if it recurs.
