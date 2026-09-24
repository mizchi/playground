VERDICT: better

The extraction genuinely removes duplication rather than gaming a metric:

- `roundCents` collapses three copies of `Math.round(n*100)/100`.
- `discountRate` + `MEMBER_DISCOUNT_TIERS`/`GOLD_DISCOUNT_TIERS` replace two near-identical copy-pasted if/else ladders (same shape, different numbers) with one function and two data tables — a textbook case of real duplication being eliminated, not manufactured.
- `ITEM_WEIGHTS` and `SHIPPING_RATES` turn parallel if/else chains (by category, and by country × weight bracket) into lookup tables. The shipping table is a nice win specifically: the old code had JP and non-JP as two separate 4-branch ladders with matching thresholds spread apart; the new table puts `jp`/`other` on the same row per weight bucket, which is easier to audit for consistency.
- `validateUser`/`validateItem`/`validateItems`/`validateCoupon` decompose along the same seams the original nested if/else already implied (user vs items vs coupon), each independently readable and testable, composed via array spread in `validateOrder`. Control flow (e.g. skipping user field checks when `user` is falsy, short-circuiting on empty items) is preserved correctly.
- `calculateDiscount`/`calculateCouponDiscount`/`calculateSubtotal` pull real branching logic out of the `calculateTotal` monolith, so that function now reads as a linear pipeline instead of an 8-variable procedural block.

No sign of complexity/duplication-metric gaming: I didn't find any single-line pass-through wrapper that exists only to shave a cyclomatic-complexity count off a caller (e.g., a function whose body is just `return other(sameArgs)` with no logic of its own), nor any case where behaviorally distinct logic was force-fit into a "shared" function via an unnecessary parameter just to claim deduplication.

Only tradeoff worth flagging: the module went from 5 functions to ~15, so a reader now has to hop between more, smaller functions to trace one code path (e.g. `calculateShipping` → `hasFreeShipping`/`calculateSubtotal`/`calculateOrderWeight`/`baseShippingRate`). That's a legitimate style cost of the split, but each function is small, well-named, and independently sensible, so it's a net improvement in maintainability, not a metric-driven fragmentation.
