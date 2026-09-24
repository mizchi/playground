VERDICT: same

**What's genuinely better:**
- `subtotalOf` and `roundMoney` deduplicate logic that was previously copy-pasted in 3+ places — clear win, no downside.
- The `validateOrder` split into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` is a real improvement: each piece is independently readable and testable, and the nested if/else-with-mutation style is gone.
- `discountFor`/`couponDiscountFor` extraction from `calculateTotal` is a mild, honest win — makes the top-level function read as a pipeline.

**What's worse, and looks like metric gaming:**
- `tierDiscountRate(years, tiers: [number, number][])` and `shippingCostForWeight(weight, tiers: [number, number][], overCost)` take the two discount functions and the shipping-tier ladder — each originally a clean, self-documenting if/else chain — and replace them with a generic loop driven by anonymous tuple literals like `[[5, 0.1], [2, 0.05], [0, 0.02]]` or `[[1, 300], [5, 600], [20, 1200]]`.
  - This is the classic "extract a generic helper to kill duplication-metric hits" move: `calculateMemberDiscount` and `calculateGoldDiscount` don't actually share business logic, they just share *shape* (tiered lookup). Unifying them behind a generic function trades clear, greppable domain logic for opaque positional tuples you have to mentally decode (which number is years? which is rate? is the list sorted descending? is that guaranteed anywhere?).
  - The `[number, number]` tuple type carries zero semantic meaning — compare to a named type like `{ minYears: number; rate: number }`, or just leaving the if/else in place. A future engineer editing the JP shipping table now has to trust that tuples stay sorted descending by threshold, with no compiler or runtime check enforcing it — the original if/else made that ordering structurally obvious.
  - Net effect: fewer lines, lower "duplication" score, but strictly harder to scan, debug, and safely modify. This is the part of the diff I'd push back on in review.

Recommendation: keep the `subtotalOf`/`roundMoney`/validation extractions, but revert `tierDiscountRate` and `shippingCostForWeight` back to explicit if/else chains (or at least give the tuples named fields) — the "shared shape" here isn't real duplication worth abstracting.
