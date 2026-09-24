**VERDICT: better** — with one specific piece that looks like it exists to satisfy a duplication scanner rather than to help a reader.

**What's genuinely better:**
- `roundMoney` / `subtotalOf` deduplicate real repeated logic (rounding, item-sum) used identically in 3+ places — clean, low-risk extractions.
- Splitting `validateOrder` into `validateUser` / `validateItem` / `validateItems` / `validateCoupon` is a real win: each piece is independently testable, and the `flatMap` in `validateItems` reads better than the manual index loop.
- `hasFreeShipping` / `totalWeight` / `shippingRateForWeight` turn a single tangled function into three named steps that map directly onto the business rules ("is it free? what's the weight? what does that weight cost?"). This is a legitimate replace-conditional-with-composition refactor.
- `CATEGORY_WEIGHT_PER_UNIT` as a lookup table is a reasonable trade for the category if/else chain — adding a category is now a one-line data change instead of an `else if`.

**The metric-gaming smell — `tieredRate`:**
```ts
function tieredRate(years: number, atFive: number, atTwo: number, base: number): number { ... }
...
tieredRate(user.years, 0.1, 0.05, 0.02)   // member
tieredRate(user.years, 0.2, 0.15, 0.1)    // gold
```
Member and gold discount tiers aren't the same concept that happens to be duplicated — they're two independent business rules that coincidentally share a 3-branch shape. Collapsing them into one function with positional numeric parameters removes the domain names (`0.1`, `0.05`, `0.02` no longer say "member 5yr/2yr/base rate") and forces the reader to jump to the helper's signature to decode every call site. This is the classic pattern of merging incidental structural duplication to shrink a duplicate-lines/complexity score, at a net cost to readability — a change a jscpd/SonarQube duplication check would reward but a reviewer should push back on. A better version would keep the domain meaning, e.g. a small `{minYears, rate}[]` table per tier, or just leave the two original if/else-if chains as-is (they were already short and clear).

The `SHIPPING_RATES_BY_COUNTRY` tuple (`[under1, under5, under20, atLeast20]`) has a milder version of the same issue — the array itself carries no labels, so correctness depends on positional order — but with only two entries and clear destructuring names at the call site, it's a minor, acceptable trade rather than a real problem.

**Recommendation:** keep everything except `tieredRate`; replace it with a small named-tier table or just inline the two original tier chains so the discount numbers stay attached to their meaning.
