**VERDICT: same**

The diff bundles two very different kinds of extraction, and they roughly cancel out.

**Genuine improvements:**
- `roundMoney` and `subtotalOf` are real duplication removal (used 3x and 2x) — clear win, no downside.
- Splitting `validateOrder` into `validateUser`/`validateItems`/`validateItem`/`validateCoupon` is a legitimate decomposition along real conceptual boundaries (user vs. items vs. coupon are independent concerns), each is independently testable, and nothing is lost in following the logic.
- `qualifiesForFreeShipping` and `baseShippingRate` are reasonable, self-explanatory extractions.

**This looks like metric gaming:**
- `yearsBasedRate(years, highRate, midRate, lowRate)` is the clearest smell in the diff. The original inline `if (years >= 5) rate = 0.1; else if (years >= 2) rate = 0.05; else rate = 0.02;` was self-documenting in context. The extracted version is called as `yearsBasedRate(user.years, 0.1, 0.05, 0.02)` — three unlabeled positional numbers whose meaning ("high/mid/low") only exists in the parameter names you can't see at the call site. A future edit that swaps two arguments by accident (e.g. `yearsBasedRate(user.years, 0.05, 0.1, 0.02)`) is a silent bug with no type system or test to catch the intent mismatch. The only reason to do this extraction is that a token-based duplication detector (jscpd/SonarQube CPD) would flag the two near-identical if/else-if blocks in `calculateMemberDiscount`/`calculateGoldDiscount` as clones — this "fixes" that metric while making the call sites strictly harder to read.
- `itemWeight`/`orderWeight`/`baseShippingRate` fragment what was one linear, easy-to-scan function (`calculateShipping`) into four single-use functions that must be mentally re-composed. Each individual function now has lower cyclomatic complexity, but the total control flow and coupling haven't changed — this reads like complexity-metric gaming (per-function complexity thresholds) rather than a comprehension win. Nothing here is reused elsewhere, so it's decomposition for its own sake.

**Net assessment:** the validation split and the money/subtotal helpers are honest DRY/SRP improvements. The `yearsBasedRate` helper is a readability regression disguised as deduplication, and the shipping-function fragmentation trades a slightly long-but-linear function for several harder-to-navigate small ones without a real payoff. Those two roughly offset the genuine gains, landing the overall change at "same" rather than "better."
