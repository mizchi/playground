**VERDICT: better** — most of the extraction is genuine and improves the file (shared `subtotalOf`/`roundMoney`, `validateOrder` split into per-field validators that mirror its own structure, shipping broken into `orderWeight`/`baseShippingRate`/`qualifiesForFreeShipping` named concepts). Those extractions read *more* clearly at the call site than the original nested loops.

One thing to flag as metric-gaming rather than genuine deduplication:

**`yearsBasedRate(years, highRate, midRate, lowRate)`** — this merges two *semantically unrelated* tier tables (member discount: 10/5/2%, gold discount: 20/15/10%) into one function purely to cut duplicate-looking lines. The cost:
- Call sites (`yearsBasedRate(user.years, 0.1, 0.05, 0.02)`) are now positional magic numbers with no label telling you which is "high" vs "mid" vs "low" without opening the function — strictly less self-documenting than the original explicit if/else.
- The thresholds (`>=5`, `>=2`) are hardcoded inside the helper while only the *rates* are parameterized, so it implicitly assumes member and gold tiers will always share the same year breakpoints. That's an invented coupling that didn't exist in the domain — if gold's tier boundary ever changes independently, this abstraction has to be un-done.
- It doesn't reduce real complexity, it just moves two literal 3-line if-chains into two call sites with unlabeled arguments — the classic "extract-a-function-to-lower-the-duplication-score" pattern rather than removing an actual shared concept.

I'd keep the rest of the diff but inline `yearsBasedRate` back into each discount function (or replace it with a small typed lookup like `{high:0.1, mid:0.05, low:0.02}` passed as a named object, not positional args).
