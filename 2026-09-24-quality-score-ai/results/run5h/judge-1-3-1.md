**VERDICT: same**

The diff has some genuine wins mixed with several moves that look tuned to please a static-analysis score rather than a human reader.

**Real improvements:**
- `roundCents` and `calculateSubtotal` deduplicate logic that really was copy-pasted 3× before.
- `calculateTotal` now reads as a clean top-to-bottom pipeline (subtotal → discount → coupon → shipping → total), which is a legitimate readability win.
- Splitting item/coupon/user validation into named functions is reasonable since they're independently testable concerns.

**Signs of metric gaming:**

1. **`rateForYears` / `shippingRateForWeight` generic lookups.** A 3-branch `if/else-if` (trivially readable top-to-bottom in one function) got replaced with: a `TierRate` type, a module-level data array, and a generic `.find()`-based resolver. To answer "what discount does a 3-year member get?" you now hop through 3 separate definitions instead of reading one function. The actual decision complexity didn't shrink — it just moved out of the function body where a cyclomatic-complexity linter can't see it. With only 2 call sites each (member/gold, JP/intl), this is premature abstraction whose main effect is lowering the per-function branch count.

2. **`calculateTierDiscount` / `calculateCouponDiscount`.** These are single-caller, trivial `if` chains pulled out solely to shrink `calculateTotal`'s own complexity number. They add navigation overhead without adding meaning — a classic "extract-to-reduce-my-own-complexity-score" pattern.

3. **Function count roughly tripled** (5 top-level functions → 17) for identical behavior. More files-worth of indirection to trace for the same logic is a real maintainability cost, not just a style preference.

4. `validateUser`/`validateItem`/`validateItems`/`validateCoupon` split is defensible, but combined with the above it contributes to the same pattern: shrinking each function's local complexity by scattering the whole picture across many small functions, which a duplication/complexity scorer rewards but a reader has to reassemble by hand.

Net: the dedup of rounding/subtotal logic and the cleaner `calculateTotal` are real gains, but they're offset by indirection introduced in places (tier lookups, single-use extraction) that mainly serve to lower per-function complexity/duplication metrics rather than to help someone understand the code faster.
