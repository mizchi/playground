**VERDICT: better** — genuine duplication was removed (rate-lookup logic, subtotal/weight/rounding calcs, validation composition), and the result is easier to test and extend. But a few extractions look like they exist to shrink per-function complexity/line counts rather than to aid a reader, and the table-driven lookups trade explicit safety for an unenforced invariant.

**What's genuinely better:**
- `calculateMemberDiscount`/`calculateGoldDiscount` shared identical branching logic before (same structure, different numbers) — collapsing that into `rateForYears` + two config tables is textbook, non-gamed DRY.
- `roundToCents`, `calculateSubtotal`, `calculateWeight` were literally copy-pasted across functions before; extracting them is a real win, not a metric trick.
- `validateItem`/`validateItems` via `flatMap` is a legitimate simplification of the manual index loop, and it's reused.

**What looks like gaming the metric rather than improving the code:**
- `calculateDiscount(user, subtotal)` and `calculateCouponDiscount(coupon, subtotal, discount)` in `calculateTotal` each have exactly one call site. They don't remove any duplication — they just relocate the same if/else chain into a same-file helper so `calculateTotal`'s own cyclomatic complexity/line count drops. A reader now has to jump to two extra functions to reconstruct the same linear flow that used to read top-to-bottom in one place. This is the clearest case of "smaller function, same total complexity, worse locality."
- `validateUser` is similarly single-call-site; splitting it out is defensible for symmetry with `validateItems`/`validateCoupon`, but on its own it's borderline (readability payoff is small).

**A real regression, not just style:**
- `rateForYears` (and the two tier tables) and `baseShippingCost`'s `SHIPPING_RATES_BY_COUNTRY` both rely on array **order** to be correct — `.find()` returns the *first* match, so `MEMBER_DISCOUNT_TIERS`/`GOLD_DISCOUNT_TIERS` must stay sorted descending by `minYears`, and the shipping tables must stay sorted ascending by `maxWeight`. Nothing enforces this — a future edit (e.g. adding a `{ minYears: 3, rate: 0.07 }` tier in the wrong position) silently produces a wrong discount with no type error and no test failure pointing at the cause. The original nested `if/else if` made the ordering the *only possible reading* of the code; the table version makes it a convention someone has to know. This is a real maintainability cost the refactor introduces in exchange for compactness.

Net: worth keeping, but I'd push back on `calculateDiscount`/`calculateCouponDiscount` (inline them back into `calculateTotal`, or merge them into one `applyDiscounts` helper that returns both) and add a one-line comment or an assertion that the tier arrays must stay sorted.
