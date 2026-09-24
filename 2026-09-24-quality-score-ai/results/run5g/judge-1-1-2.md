**Review**

Genuine improvements:
- `roundCurrency` / `calculateSubtotal` extraction removes real triplicated code.
- Splitting `validateOrder` into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` is a legitimate decomposition — each piece is independently readable and the original nested-branch monolith was genuinely harder to scan.
- `calculateTierDiscount` / `calculateCouponDiscount` pulled out of `calculateTotal` is a reasonable, low-risk extraction.
- The JP/OTHER shipping-fee table (`SHIPPING_FEE_TIERS`) collapses two structurally identical if/else chains that really were duplicated — turning that into a lookup table plus one `baseShippingFee` function is a fair trade.

Looks like metric gaming:
- `rateForYears(years, tiers)` + `MEMBER_RATE_TIERS` / `GOLD_RATE_TIERS`. The original `if (years >= 5) ... else if (years >= 2) ... else ...` was already about as simple as this logic can be — three lines, obvious thresholds, no cross-function duplication to speak of (member/gold have different values and only a superficially similar shape). Converting it to `[minYears, rate]` tuples iterated in order trades a self-evident conditional for a data table whose correctness depends on an *unenforced invariant* (entries must stay sorted descending by `minYears`, with `0` as a mandatory catch-all). Nothing in the `YearsTier` type prevents someone from reordering the array and silently breaking discount calculation. This is the classic pattern of turning branches into a loop over a table purely to shrink a function's cyclomatic-complexity count — it reduces the branch count the tool sees, not the cognitive load a reader carries. Same critique, milder, applies to `baseShippingFee`'s `tiers.find(([belowWeight]) => weight < belowWeight)` relying on ascending order and an `Infinity` sentinel instead of an explicit final `else`.

Net effect: the parts that removed real duplication (subtotal, rounding, shipping-country table, validation split) are unambiguous wins; the parts that replaced clear if/else‑if chains with generic tuple-driven loops (year-tier discounts) added indirection and a fragile ordering invariant without removing any real duplication, which looks aimed at a complexity/duplication score rather than at readers.

VERDICT: same
Reason: legitimate deduplication (subtotal/rounding/shipping table/validation split) is offset by the year-tier rate lookup, which converts simple, self-contained conditionals into a generically-parameterized loop over unenforced-order tuples — a textbook complexity-metric-gaming pattern that trades clarity for a lower branch count.
