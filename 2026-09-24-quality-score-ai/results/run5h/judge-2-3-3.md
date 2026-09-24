**VERDICT: better**

The bulk of this refactor is genuine, not metric theater: `subtotalOf`, `roundCents`, and the split of `validateOrder` into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` extract real repeated logic and give each validation rule an obvious home — that's a legitimate readability win, and `calculateTotal`/`calculateShipping` read top-to-bottom as a clear pipeline now instead of a wall of mutated locals.

One spot smells like duplication-metric gaming rather than real abstraction: **`tieredRate`**. It's shared between `calculateMemberDiscount`/`calculateGoldDiscount` (genuinely the same concept — tenure-based rate tables) and `baseShippingRate` (a different concept — weight-based shipping cost tiers). To force shipping through the same helper, the author had to:

- Invert the original `weight < X` ascending ladder into a `weight >= X` descending ladder and re-derive the tier boundaries/order to stay equivalent (e.g. `weight<1→300` became `[0,300]` matched last). It's correct, but it's non-obvious — a reviewer has to mentally re-derive the equivalence rather than see it directly, whereas the original if/else was self-evidently correct by inspection.
- Leave the generic function's parameter literally named `years` (`tieredRate(years: number, tiers: ...)`), which is now called with a `weight` argument from `baseShippingRate`. That's a clear tell that the function was lifted from the discount case and reused for shipping without being generalized, not designed as a shared abstraction — it collapses two unrelated domains because they happen to have the same "threshold → value" shape.

This is the classic sign of chasing a duplication metric: same code shape, different concepts, merged via array-of-tuples literals (`[[5, 0.1], [2, 0.05], [0, 0.02]]`) that are less self-documenting than the explicit if/else they replaced, and require a naming leak (`years`) and a comparison-direction inversion to make the merge work.

Net: worth keeping — the validation and total-calculation decomposition clearly improves maintainability — but I'd push back on `tieredRate` being shared across discount and shipping specifically, and would rename its parameter (e.g. `metric`/`threshold`) or just give shipping its own small tier lookup rather than force a shared generic.
