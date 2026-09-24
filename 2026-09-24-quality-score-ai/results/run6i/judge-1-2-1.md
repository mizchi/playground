## Review

**Genuine improvements:**
- `subtotalOf` / `roundCurrency` extraction removes real duplication (subtotal was computed three separate times; rounding logic four times). This is a legitimate, low-risk simplification.
- Splitting `validateOrder` into `validateUser` / `validateItem` / `validateItems` / `validateCoupon` is a reasonable decomposition along natural seams (one concern per function), and it's easy to verify each piece preserves the original logic.

**Questionable extraction — looks like metric gaming:**
- `tieredRate(years, tiers: RateTier[])` and `baseShipping(weight, country)` replace simple, self-contained if/else cascades with a generic loop over a data table. This is the textbook move to dodge a duplication detector (jscpd/SonarQube) that would have flagged the near-identical JP/INTL or member/gold if-else blocks — but it doesn't actually reduce the system's complexity, it just relocates it into implicit invariants:
  - `RateTier[]` and `ShippingTier[]` **must be kept in the right order** (descending by `minYears`, ascending by `belowWeight`) for the loop's first-match-wins logic to be correct. Nothing enforces or documents this. The original nested if/else made the ordering explicit and impossible to get wrong; the table version makes it a silent footgun for the next person who adds or reorders a tier.
  - To answer "what discount does a 3-year gold member get?" you now have to jump from `calculateGoldDiscount` → `tieredRate` → `GOLD_RATE_TIERS` (declared elsewhere in the file) instead of reading four lines top-to-bottom.
  - The change turns 2 functions with clear literal branches into ~9 new symbols (`RateTier`, `tieredRate`, `MEMBER_RATE_TIERS`, `GOLD_RATE_TIERS`, `ShippingTier`, `JP_SHIPPING_TIERS`, `INTL_SHIPPING_TIERS`, `baseShipping`, `totalWeight`/`itemWeight`/`CATEGORY_WEIGHT_PER_UNIT`) for logic that fits comfortably in two small functions. Per-function cyclomatic complexity drops, but that's the metric being gamed — actual cognitive load to trace a single business rule goes up.

- `hasFreeShipping` is fine, but it now silently depends on `subtotalOf` being called a second time (already computed once in `calculateTotal` as `subtotal`), a minor efficiency/readability smell introduced by the split (previously it was one inline computation, now it's a redundant recomputation buried inside `calculateShipping`).

**Net effect:** the validation half of the file got genuinely cleaner; the pricing/shipping half traded straightforward, explicit conditionals for indirect, order-dependent config tables that are harder to audit at a glance and whose main benefit is suppressing a duplication/complexity score rather than making the rules easier to understand or change safely.

VERDICT: worse
Reason: the two riskiest, most business-critical pieces (discount tiers, shipping tiers) went from explicit, self-contained if/else logic to generic lookup tables with an unenforced ordering invariant — a classic duplication/complexity-metric dodge that adds indirection without reducing real complexity, outweighing the genuine gains in the validation split.
