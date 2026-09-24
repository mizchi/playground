Warning: no stdin data received in 3s, proceeding without it. If piping from a slow command, redirect stdin explicitly: < /dev/null to skip, or wait longer.
**VERDICT: same**

The refactor is a mixed bag — genuine wins offset by patterns that look tuned for a complexity/duplication scanner rather than for a human reader.

**Genuine improvements:**
- `report.ts`: `summarizeBy(orders, keyFor)` is a clean, honest dedup of three identical loops. No downside here.
- `calculateSubtotal`/`roundCurrency` extraction removes real repetition (subtotal loop and `Math.round(x*100)/100` appeared 3-4 times).
- Splitting `validateOrder` into `validateUser`/`validateItems`/`validateItem`/`validateCoupon` is reasonable — each piece maps to an obvious concept and the top-level function reads as a clear spec.
- `calculateTierDiscount`/`calculateCouponDiscount` extraction from `calculateTotal` is a fair, low-risk simplification.

**Looks like metric-gaming, not readability:**
- `MEMBER_DISCOUNT_RATES` / `GOLD_DISCOUNT_RATES` + `rateForYears` (`tiers.find(t => years >= t.minYears)`) replaces an explicit, obviously-correct if/else-if chain with a data table that only works because entries are ordered **descending** by `minYears`. That invariant is invisible at the call site and unenforced by the type system — swap two rows and it silently breaks. This is the classic "convert branches to a lookup table" move that lowers a cyclomatic-complexity count while making the actual behavior harder to verify at a glance.
- Same pattern in `JP_SHIPPING_RATES`/`DEFAULT_SHIPPING_RATES` + `shippingCostForWeight` (`brackets.find(b => weight < b.belowWeight)`), which additionally requires **ascending** order to be correct — the opposite ordering convention from the discount tables above, which is an easy source of confusion if someone edits one and mentally copies the other. The trailing fallback `bracket ? bracket.cost : brackets[brackets.length-1].cost` is dead code, since the last bracket already uses `Infinity` and will always match — a tell that the abstraction is being forced to "look" branch-free rather than expressing real logic.
- `calculateShipping` itself was one linear, easy-to-read ~25-line function before. Now understanding "how is shipping computed for this order" requires jumping across `itemWeight`, `totalWeight`, `shippingCostForWeight`, two rate tables, and `hasFreeShipping`. Each piece is individually trivial (and scores well on per-function complexity/LOC metrics), but tracing one behavior now costs far more navigation than before — a classic case of fragmentation that helps metrics while hurting comprehension.

Net effect: the parts that reduce true duplication (report.ts, validate*, subtotal/round helpers) are solid; the parts built around `find()`-over-ordered-tables for discount/shipping tiers trade an explicit if/else chain (complexity a reader can verify by inspection) for implicit ordering invariants (complexity a reader has to trust). That roughly cancels out, so I'd call it a wash rather than a clear win.
