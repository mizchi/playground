**VERDICT: same** — genuine wins in one area are offset by metric-gaming smells in another.

**Real improvements:**
- `validateUser`/`validateItem`/`validateItems`/`validateCoupon` split is a clean, genuine decomposition — each piece is independently readable and testable, and the nested if/else pyramid is gone without loss of clarity.
- `weightFor` turning the category if/else chain into a `switch` is a straightforward, non-controversial readability win.
- `roundMoney`/`subtotalOf` are real duplication removal — used identically in ≥2 places, trivial to understand at the call site.

**Smells like metric gaming, not genuine improvement:**
- `tierRate(years, highRate, midRate, lowRate)` and `shippingRateFor(weight, tiny, small, medium, large)` collapse two *structurally similar but semantically distinct* tables (member vs. gold discount rates; JP vs. non-JP shipping costs) into generic functions driven by **positional magic-number arguments**. This looks purpose-built to shrink a duplication-checker's score, not to help a reader:
  - At the call site (`tierRate(user.years, 0.1, 0.05, 0.02)`, `shippingRateFor(weight, 300, 600, 1200, 3000)`), you can no longer see which number means what without jumping to the function definition — the original inline `if (years >= 5) rate = 0.1` was self-documenting by contrast.
  - The two helpers even order their parameters inconsistently — `tierRate` goes high→mid→low, `shippingRateFor` goes tiny→small→medium→large — a landmine for anyone editing one and assuming the other's convention.
  - A lookup table (e.g. `[{min: 5, rate: 0.1}, ...]`) or a small named-tier object would have removed the duplication *and* kept semantics visible at the call site. Bare positional numbers is the worse tradeoff.

Net effect: the validation logic is meaningfully easier to maintain, but the rate/shipping tables traded self-evident inline values for an abstraction that only pays off if you're optimizing a duplication/complexity score — a maintainer reading `calculateGoldDiscount` or `calculateShipping` cold is arguably worse off than before.
