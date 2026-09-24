Warning: no stdin data received in 3s, proceeding without it. If piping from a slow command, redirect stdin explicitly: < /dev/null to skip, or wait longer.
**VERDICT: same**

The refactor is a mixed bag — some parts are genuine improvements, others look like they're optimizing for a complexity/duplication scanner rather than for a human reader.

**Genuine wins:**
- `calculateSubtotal` / `roundCurrency` extraction — real duplication removed, correctly reused.
- `validateUser` / `validateItem` / `validateItems` / `validateCoupon` — the original `validateOrder` was one long function with three separate concerns; splitting it by concern is a legitimate readability gain.
- `calculateTierDiscount` / `calculateCouponDiscount` pulled out of `calculateTotal` — reasonable, keeps the top-level function as a readable pipeline.

**Looks like metric gaming, not real simplification:**
- `MEMBER_DISCOUNT_RATES` / `GOLD_DISCOUNT_RATES` + `rateForYears`, and `JP_SHIPPING_RATES` / `DEFAULT_SHIPPING_RATES` + `shippingCostForWeight`: turning a simple, explicit if/else ladder into a data table + generic "find first match" lookup is a textbook way to drop McCabe/branch-count metrics without actually making the logic easier to follow. Now a reader has to hold two things in their head that used to be self-evident from the code: that the rate table must stay sorted *descending* by `minYears`, and the weight table must stay sorted *ascending* by `belowWeight` — neither invariant is enforced by the type system or a comment, so a future edit that appends a tier in the wrong position silently breaks pricing. The original explicit chain was more verbose but impossible to get subtly wrong this way.
- `shippingCostForWeight`'s `brackets.find(...) ?? brackets[brackets.length-1]` fallback is dead code — the last bracket always has `belowWeight: Infinity`, so `find` can never fail. That's defensive code for a case that can't happen, added seemingly just to satisfy TypeScript's possibly-undefined return, and it obscures that guarantee instead of stating it.
- `calculateShipping` now calls `calculateSubtotal(order.items)` internally via `hasFreeShipping`, while `calculateTotal` also computes the subtotal separately and doesn't reuse it — so the "de-duplication" refactor actually introduced a second, redundant subtotal computation on every `calculateTotal` call.

Net effect: the validation and rounding/subtotal extractions are worth keeping, but the rate-table conversions trade explicit, verifiable branching for implicit sort-order invariants — a pattern more consistent with gaming a complexity/duplication score than with improving comprehension.
