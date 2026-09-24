**VERDICT: same** — genuine wins are offset by indirection that looks tuned for complexity/duplication metrics rather than real clarity.

**What's genuinely better:**
- `MEMBER_DISCOUNT_BANDS`/`GOLD_DISCOUNT_BANDS` collapsing two near-identical if/else ladders into one `discountRate` lookup is real duplication removal.
- `validateUser`/`validateItem`/`validateItems`/`validateCoupon` is a sensible decomposition — each piece is independently readable and testable, and the nesting in the original (`if user... else if items... else`) is gone.
- `orderSubtotal` is reused twice (`qualifiesForFreeShipping` and `calculateTotal`) — that's legitimate.

**What smells like metric-gaming rather than real improvement:**

1. **Single-use "extractions" with no reuse benefit**: `tierDiscountAmount` and `couponDiscountAmount` are each called exactly once, from `calculateTotal`. Splitting them out doesn't remove duplication — it just relocates the same branches into same-named functions so `calculateTotal`'s own line count / branch count drops. To understand `calculateTotal` you now have to jump to three other functions instead of reading five lines top-to-bottom. This is the classic pattern of gaming a "function complexity" or "function length" linter score: total branching in the module is unchanged, but no single function looks "complex" anymore.

2. **`shippingCostForWeight`'s dead fallback**: `bands.find(b => weight < b.maxWeight)` always matches because the last band's `maxWeight` is `Infinity`, so `return band ? band.cost : bands[bands.length - 1].cost` — the `: bands[...]` branch is unreachable. That's defensive code added to *look* safe while actually just adding a branch nothing exercises (also inflates the function's apparent "robustness" without real justification).

3. **Order-dependent correctness hidden in data**: `discountRate` relies on `bands` being sorted descending by `minYears`, and `shippingCostForWeight` relies on ascending `maxWeight`. Nothing enforces this — it's an implicit invariant that used to be obvious from an explicit `if/else if` chain and is now a silent trap for whoever edits the table later (e.g., appends a new band instead of inserting it in order).

4. **Tier logic gets scattered instead of consolidated**: knowledge of "who is gold/member" now lives in four separate places — `MEMBER_DISCOUNT_BANDS`/`GOLD_DISCOUNT_BANDS`, `qualifiesForFreeShipping`, and `tierDiscountAmount`. Adding a new tier means touching multiple disjoint spots rather than one clear switch, which is a step backward for maintainability even though each individual function looks simpler.

Net: behavior is preserved and a couple of pieces (discount bands, validation split) are honest improvements, but several extractions exist only to shrink individual function metrics, adding indirection you have to chase without reducing actual complexity — and in a couple of spots (unenforced band ordering, dead fallback branch) it trades explicit-but-repetitive code for implicit, more fragile code.
