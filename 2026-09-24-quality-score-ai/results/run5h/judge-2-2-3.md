**VERDICT: same**

The diff is a mixed bag — some genuine improvements offset by indirection that looks aimed at complexity/duplication scores rather than readers.

**Genuine improvements:**
- `roundMoney`, `calculateSubtotal` are real, low-risk dedup — used at multiple call sites, obvious what they do, no indirection cost.
- Splitting `validateOrder` into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` is a solid decomposition: each function is independently readable and testable, and nothing is hidden behind data tables.
- `calculateTierDiscount`/`calculateCouponDiscount` giving names to the two discount steps in `calculateTotal` makes that function read top-to-bottom more like a spec.

**Looks like metric gaming, not clarity:**
- `shippingRateForWeight(weight, rates)` with `rates[0]`/`rates[1]`/`rates[2]`/`rates[3]` and a comment (`// Rates ordered by ascending weight threshold: [<1, <5, <20, >=20]`) is worse than the original two parallel if/else chains. The original was self-documenting (`shipping = 600`); the new version requires cross-referencing a comment to know what index 2 means, and the "meaning" of a rate now lives purely in position/array order — a classic case of collapsing an if/else duplication smell into a fragile positional array to shrink a duplication/complexity metric, at the cost of a reader having to hold an implicit contract in their head.
- `rateForYears(years, tiers: TierRate[])` + `MEMBER_TIER_RATES`/`GOLD_TIER_RATES` does the same thing to the discount-rate logic: a 3-branch if/else (trivial to read in place) becomes a generic "first match wins, tiers must be sorted descending" loop over a data table defined elsewhere. For two call sites this is over-abstraction — it adds a loop, an implicit ordering invariant, and a level of indirection to replace something that was already maximally clear.

Both of these are the textbook pattern that gets rewarded by cyclomatic-complexity/duplication linters (fewer branches, less repeated `if/else` shape) while making a human trace two files/definitions instead of reading one function. If a linter's complexity or duplication score improved here, that's very likely why these two specific pieces were done this way — the rest of the diff (validation split, subtotal/rounding extraction) doesn't share that pattern and is straightforwardly better.
