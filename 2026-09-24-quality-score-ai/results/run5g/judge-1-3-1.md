VERDICT: better

The decomposition tracks real seams in the domain (rounding, tiered-rate lookup, subtotal, shipping weight/fee, free-shipping eligibility, per-field validation, coupon logic), and each extraction removes genuine duplication rather than paraphrasing it:

- `roundMoney` collapses three copies of the same rounding expression.
- `rateForYears` is the correct fix for `calculateMemberDiscount`/`calculateGoldDiscount`, which were the same algorithm with different constants — this is the strongest part of the diff.
- `validateUser/validateItem/validateItems/validateCoupon` turn one long imperative function into independently readable, independently testable pieces, and preserve the original error ordering and early-return semantics exactly (checked: user→items→coupon order, mutually-exclusive coupon errors, furniture-qty check only reached after price/qty pass).
- The shipping table (`SHIPPING_RATES`/`DEFAULT_SHIPPING_RATE`, `CATEGORY_WEIGHT_FACTORS`) replaces nested if/else-if chains with data, and `calculateShipping` itself reads cleanly as "free shipping? else base fee by weight/country."

I traced the logic through by hand and didn't find behavior changes — bracket ordering, thresholds, and coupon math all match the original.

Two things worth flagging, not as metric-gaming but as real trade-offs to watch:
- The bracket arrays (`[[1,300],[5,600],...]`) encode control-flow order as data — correct today, but a future editor who "just sorts the config" could silently break it. A comment or an explicit `sort`/assertion would make that invariant visible.
- `hasFreeShipping` calls `orderSubtotal(order.items)` again even though `calculateTotal` already computed the subtotal — minor duplicated work, not a correctness issue, but slightly inconsistent with how aggressively duplication was hunted elsewhere in this same diff.

Nothing here looks like it was split up purely to shrink a duplication/complexity score — the extracted functions are meaningfully named, individually testable, and the two/three genuinely duplicated code paths (rounding, discount-rate ladder, subtotal loop) are the ones actually merged. The discount-rate tuples (`rateForYears(user.years, [[5, 0.1], ...])`) could use a named type alias like `ShippingBracket` for consistency, but that's a nit, not a red flag.
