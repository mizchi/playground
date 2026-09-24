**VERDICT: same**

The diff is a mixed bag: some extractions are genuine DRY wins, others look like mechanical decomposition done to shrink per-function complexity/line-count numbers rather than to help a reader.

**Genuine improvements:**
- `round2` — real duplication removed (was copy-pasted 3x).
- `subtotalOf` — real duplication removed (was copy-pasted 2x, including inside `calculateShipping`).
- `validateUser` / `validateItem` / `validateItems` / `validateCoupon` — these map to real, independent concerns (different entities), and `validateItem` is genuinely reused in a loop. The final `validateOrder` reading as a flat spread of three validators is arguably clearer than the original nested if/else pyramid.

**Looks like metric gaming, not readability:**
- `itemWeight`, `totalWeight`, `baseShippingRate`, and `hasFreeShipping` are each called from exactly one call site. Splitting a single linear, easy-to-trace function (`calculateShipping`) into five tiny functions doesn't add reuse or reduce total branching — it just redistributes the same decision points across more, smaller functions so each one scores low on a per-function cyclomatic/cognitive-complexity check. A reader who wants to understand "how is shipping computed" now has to jump through four definitions instead of reading top-to-bottom. This is the classic "salami-slice a function until the linter stops complaining" pattern — total complexity is unchanged, it's just hidden by fragmentation.
- Similarly, `itemWeight` being pulled out of `totalWeight`'s loop body adds a layer of indirection for a single-use, four-branch if/else that was perfectly readable inline.

Net effect: the validation and rounding/subtotal changes are real improvements; the shipping decomposition adds indirection without payoff. They roughly cancel out, so the change is not a clear net gain in readability/maintainability even though it would likely score better on cyclomatic-complexity-per-function or duplication metrics.
