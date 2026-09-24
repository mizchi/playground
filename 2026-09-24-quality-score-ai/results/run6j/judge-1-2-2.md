**VERDICT: same**

The change is a wash: a few extractions are genuine improvements, but several others look like they exist to satisfy a complexity/duplication scanner rather than to help a reader.

Genuine wins:
- `roundMoney` and `computeSubtotal` kill real duplication (subtotal was computed three separate times; rounding logic four times).
- `computeWeight` cleanly isolates a self-contained calculation.

Looks like metric-gaming:
- `tieredRate(years, [[5,0.1],[2,0.05],[0,0.02]])` and `SHIPPING_RATES`/`shippingCostForWeight` replace explicit if/else chains with anonymous tuple arrays. This is the classic move to dodge a duplication detector (jscpd-style tools match on structurally identical if/else blocks, not on data literals) — but it makes the business rule *less* legible: you now have to mentally re-derive "first tuple whose threshold the value clears, in this specific order" instead of reading a plain threshold ladder. The `find(...)!` non-null assertion in `shippingCostForWeight` is a real smell — it silently depends on `Infinity` always being present in the table, with no compiler or runtime guarantee if someone edits `SHIPPING_RATES` later.
- `calculateTierDiscount` and `calculateCouponDiscount` don't reduce anything — they just relocate the same if/else out of `calculateTotal` into their own functions, presumably to lower that one function's cyclomatic-complexity score. Total branching in the program is unchanged; you've just added two more places to jump to understand one linear calculation.
- Splitting `validateOrder` into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` is defensible for testability, but combined with the other splits, `calculateTotal`'s logic is now spread across 6+ tiny functions for what was previously one ~15-line function that read top-to-bottom. Tracing "how is total computed" now requires bouncing through the file rather than reading linearly.

Net effect: line count and per-function complexity went down, but so did local readability — you trade "one function with visible branches" for "many functions with implicit contracts" (ordering of tuples, presence of the `Infinity` sentinel, etc.), which is a maintainability wash rather than a clear win.
