**VERDICT: better** — but with a few extractions that look like they exist mainly to shrink per-function complexity numbers rather than for real reuse or clarity.

**Genuine improvements:**
- `roundCents` kills 3x duplicated `Math.round(x * 100) / 100`.
- `discountRate` + `MEMBER_DISCOUNT_TIERS`/`GOLD_DISCOUNT_TIERS` collapse two nearly-identical nested if/else chains into one table-driven function — this is real duplication removed, and adding a new tier is now a one-line data change instead of editing two parallel functions.
- `baseShippingRate` + `SHIPPING_RATES` table does the same for the shipping matrix — the original had two duplicated 4-branch if/else ladders (JP vs other); now it's one lookup. Verified the tier ordering/thresholds are preserved exactly.
- `calculateSubtotal` centralizes a sum that was previously written out twice (once in `calculateShipping`, once in `calculateTotal`).
- Top-level `calculateTotal`/`calculateShipping`/`validateOrder` now read as a flat, named sequence of steps instead of a wall of nested conditionals — that's a real readability win even where the sub-functions aren't reused.

**Borderline / looks like metric gaming:**
- `validateUser`, `validateItem`, `validateItems`, `validateCoupon` are each called from exactly one place. There was no duplicated logic here to remove — this is straight decomposition of a single function into several single-use pieces. That's the classic move for satisfying a cyclomatic-complexity-per-function threshold (or a "function too long" lint rule): total branching in the module is unchanged, but no single function's count looks bad anymore. It's not harmful (each piece is short and well-named), but it's decomposition for its own sake, not for reuse or genuine cohesion.
- Same pattern with `calculateDiscount` and `calculateCouponDiscount` inside `calculateTotal`, and `hasFreeShipping` inside `calculateShipping` — single call sites, extracted mainly to keep the caller's branch count low.

None of this is egregious — the extracted functions are small, correctly named, and don't hurt correctness (I traced the shipping-tier and discount-tier logic and it's behavior-preserving). But if a duplication/complexity score improved by more than the table-driven dedups alone would explain, the single-use validation/total helpers are the likely reason, and that's inflated metric improvement rather than real duplication reduction.
