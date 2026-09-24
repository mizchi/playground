**VERDICT: better** — but with one clear metric-gaming smell worth fixing.

**Genuine improvements:**
- `validateOrder` decomposition into `validateUser`/`validateItem(s)`/`validateCoupon` + spread/`flatMap` is a real win — the original was a deeply nested pyramid of `if/else` with implicit early-exit logic; the new version is flat, each piece is independently readable, and the array composition at the end reads like a spec.
- `calculateShipping` dropping the nested `if/else` for country/tier in favor of early returns and a ternary is clearer control flow, not just fewer lines.
- `roundMoney` and `subtotalOf` are textbook extractions: identical logic, identical meaning everywhere they're used, no loss of context at call sites (`roundMoney(x)` is exactly as clear as `Math.round(x*100)/100`, but named).
- `weightFor(item)` turning a `for` loop with `if/else if` into a pure `switch` is a legitimate, self-contained improvement.

**The gaming smell — `tierRate` and `shippingRateFor`:**
These two functions parameterize over bare numeric literals by position:
```ts
function tierRate(years, highRate, midRate, lowRate) { ... }
function shippingRateFor(weight, tiny, small, medium, large) { ... }
```
Yes, this collapses two structurally-identical `if/else if/else` chains into one, which will make a duplication scanner (e.g. jscpd) happy. But it does so by hiding the meaning of each argument behind position:
- `tierRate(user.years, 0.1, 0.05, 0.02)` — a reader at the call site can't tell which number is the 5-year rate vs. the 2-year rate without opening the function and re-deriving it from parameter order. Swap two args by mistake during a future edit (e.g. adding a new discount tier) and it's a silent, un-type-checked bug — nothing stops `tierRate(years, 0.02, 0.1, 0.05)` from compiling.
- `shippingRateFor(weight, 300, 600, 1200, 3000)` has the same problem, now with four positional numbers instead of three.

This is the classic "extract-the-duplicate-shape, parameterize-the-magic-numbers" move that shrinks a duplication/complexity score without actually reducing the domain complexity — the branching logic still exists, it's just been made harder to verify at a glance. A version that kept the same dedup benefit *without* the readability cost would use a small ordered table or a named-arg object, e.g. `tierRate(years, { high: 0.1, mid: 0.05, low: 0.02 })` or an array of `{ min, rate }` bands — either preserves self-documentation at the call site.

Net effect: the validation and shipping-control-flow rework are strong, unambiguous improvements that outweigh the two suspect helper functions, so the diff is better overall — but `tierRate`/`shippingRateFor` should be flagged in review as metric-driven rather than clarity-driven refactoring.
