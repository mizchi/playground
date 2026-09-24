**VERDICT: same**

The diff is a mixed bag — genuine improvements in one area, metric-gaming indirection in another, roughly netting out.

**Genuine improvements:**
- `validateOrder` → `validateUser`/`validateItems`/`validateItem`/`validateCoupon` is a real win: each piece is independently readable and testable, and the nested if/else-with-accumulator style is gone.
- `subtotalOf`/`roundMoney` extraction removes three copies of the same reduce/round logic — legitimate DRY.
- `calculateTotal` reads top-to-bottom as a clear pipeline now (`tierDiscountFor`, `couponDiscountFor`, `calculateShipping`) instead of one long imperative block.

**Looks like gaming a complexity/duplication metric:**
- `rateByYears(years, tiers: YearTier[])` and `rateByWeight(weight, tiers: WeightTier[])` turn two simple, self-contained if/else-if chains into a generic "data table + interpreter" pattern. This is the classic move to shave per-function cyclomatic complexity and duplication scores: the original had two structurally-similar-but-independent branching blocks (easy to read in place, e.g. "5+ years → 10%"), now you have to jump to a `MEMBER_TIERS`/`GOLD_TIERS`/`JP_SHIPPING_TIERS` constant, understand an implicit ordering invariant (`minYears` descending, `belowWeight` ascending) that is **not enforced or documented anywhere**, and mentally re-run a `.find()`/loop to get the same answer. That's strictly more indirection for logic that wasn't meaningfully duplicated to begin with (the four rate tables have completely different values and business meaning — only the *shape* of "tiered lookup" is shared).
- `rateByWeight`'s fallback (`tiers[tiers.length - 1].rate`) is dead code, since every tier array already ends with `belowWeight: Infinity`, which the `.find()` will always match first. That's a tell that the abstraction was built to look generic/reusable rather than to fit the actual data.
- Net effect: answering "what does a 3kg JP order cost to ship?" now requires three hops (`calculateShipping` → `baseShippingRate` → `rateByWeight` + reading a const array) instead of reading one linear if/else chain in place.

If the goal were purely readability, I'd have kept the discount/shipping rate logic as explicit if/else (maybe with a short comment) and only applied the extraction pattern used in `validateOrder`. As written, the tier-table sections trade directness for a config-driven abstraction whose main benefit is lower complexity/duplication *counts*, not lower cognitive load.
