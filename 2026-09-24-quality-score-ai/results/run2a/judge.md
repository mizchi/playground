**VERDICT: same**

The validation split (`validateUser`/`validateItem`/`validateItems`/`validateCoupon`) and the `calculateTotal`/`calculateShipping` decomposition into named steps are genuine improvements — each function has one job, the top-level functions now read as a clear pipeline, and `roundMoney`/`calculateSubtotal` correctly dedupe real repeated logic. That part is a solid win.

But two extractions look like they exist to shrink a duplication/complexity scanner's count rather than to express the domain better, and they cancel out the gains:

- **`tierRateByYears(rateAtFive, rateAtTwo, rateBase, years)`** — member and gold discounts aren't actually the same logic; they just happen to both be 3-tier-by-years lookups. Collapsing them into one helper called with four positional numbers (`0.2, 0.15, 0.1, user.years`) trades an easy-to-scan if/else (where the threshold sits next to its rate) for a call site you have to mentally re-map against the parameter list. It's coincidental structural duplication, not essential duplication — a classic "DRY it because it looks similar" trap that a duplication linter would reward and a reviewer should reject.
- **`SHIPPING_RATES_BY_WEIGHT` + `baseShippingRate`'s index-based lookup (`rates[0]`, `rates[1]`, …)** — this is worse than the original. The old if/else put the weight threshold and its price on the same line (`weight < 1 → shipping = 300`). Now the correspondence between array index and threshold lives only in `baseShippingRate`'s branching, so verifying "index 2 means weight < 20" requires cross-referencing two places instead of one. This is a pure complexity-metric play (fewer branches counted) at the cost of an extra indirection layer.

Contrast with `WEIGHT_PER_UNIT`, which is a fine use of a lookup table — it's a direct category→value map, no implicit index/position coupling.

Net: real gains in the validation and total-calculation flow are offset by the two rate-table extractions, which reduce branch/duplication counts but make those two functions harder to verify at a glance. I'd ask for `tierRateByYears` and the shipping-rate table to be reverted to explicit if/else (or at least switched to named-threshold objects instead of positional arrays) before approving.
