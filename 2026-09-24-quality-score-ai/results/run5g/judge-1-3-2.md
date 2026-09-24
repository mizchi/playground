VERDICT: same

**Genuine improvements:**
- `validateUser`/`validateItem`/`validateItems`/`validateCoupon` split cleanly along real domain boundaries (user vs. item vs. coupon) — this mirrors how you'd actually think about the validation problem, and removing the nested if/else pyramid is a legitimate win.
- `SHIPPING_RATES`/`CATEGORY_WEIGHT_FACTORS` as data tables is idiomatic — shipping-by-country and weight-by-category are naturally config-shaped, and this makes adding a new country trivial.
- `orderSubtotal` deduplicating the repeated reduce loop is a fair, unambiguous fix.

**This looks like metric gaming:**
- `rateForYears(years, [[5, 0.1], [2, 0.05], [0, 0.02]])` — the original `if (years >= 5) rate = 0.1; else if (years >= 2) rate = 0.05; else rate = 0.02` was self-documenting: you can read the threshold and its consequence in one line. The tuple-array version trades that for anonymous `[minYears, rate]` pairs where you have to hold the positional convention in your head and mentally re-run the loop to see what `[5, 0.1]` means. The *only* reason to do this is that a duplication detector sees `calculateMemberDiscount` and `calculateGoldDiscount` as two near-identical if/else-if/else blocks and flags them — collapsing the branches into data makes the "duplication" (and cyclomatic complexity) disappear from static analysis while the actual domain logic (tier → threshold → rate) is no more readable, arguably less so, since the tuple shape isn't type-labeled at the call site.
- Function-count explosion: the original file had 5 top-level functions; this version has ~19, many of them one-liners (`itemWeight`, `totalWeight`, `hasFreeShipping`, `tierDiscount`, `couponDiscountFor`, `roundMoney`). Individually each function now scores great on "lines per function" / "complexity per function," but tracing `calculateShipping` or `calculateTotal` end-to-end now means jumping across 5-6 tiny indirections instead of reading one cohesive block. That's a classic complexity-metric-driven fragmentation: local score improves, whole-flow comprehension gets worse.

Net: the validation and config-table changes are real wins, but the discount-rate lookup and the sheer number of trivial helper functions read as changes optimized to satisfy a duplication/complexity scanner rather than a human maintainer, so they roughly cancel out the genuine gains.
