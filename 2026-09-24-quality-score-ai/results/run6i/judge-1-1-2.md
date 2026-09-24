**VERDICT: same**

The diff is a mixed bag — some extractions are genuine wins, one is a clear readability regression, and the sheer number of tiny new functions adds indirection that roughly offsets the gains.

**Genuine improvements:**
- `CATEGORY_WEIGHT_PER_UNIT` and `SHIPPING_RATES_BY_COUNTRY` turning if/else chains into data tables is a real maintainability win — adding a country or category is now a data change, not a code change.
- Splitting `validateOrder` into `validateUser`/`validateItems`/`validateItem`/`validateCoupon` is a legitimate decomposition; each piece is independently readable and testable.

**Gaming smell — `tieredRate`:**
```ts
function tieredRate(years: number, atFive: number, atTwo: number, base: number): number
...
tieredRate(user.years, 0.1, 0.05, 0.02)   // member
tieredRate(user.years, 0.2, 0.15, 0.1)    // gold
```
This is the textbook case of deduplicating code that only *looks* similar. The two original if/else chains represented different domain concepts (member tiers vs. gold tiers) with self-evident inline values (`rate = 0.1` next to `years >= 5`). Collapsing them into a helper with four unlabeled positional numbers means you now have to jump to the definition and count argument positions to know which number is which — and it's trivially easy for a future edit to transpose `atTwo`/`atFive`. This reduces a "duplicate code" metric while making the actual discount logic harder to verify at a glance. It's the one change here that looks metric-driven rather than reader-driven.

**Minor concern — over-fragmentation:** `roundMoney`, `subtotalOf`, `itemWeight`, `totalWeight`, `hasFreeShipping`, `tierDiscount`, `couponDiscountAmount` are each used once or twice. Individually fine, but together they mean `calculateShipping`/`calculateTotal` now read as a sequence of one-line indirections rather than a linear calculation — you have to hop through 3-4 tiny functions to reconstruct what used to be visible in one place. This lowers the "lines of code per function" and "cyclomatic complexity" scores without a proportionate readability benefit.

Net: the data-table extractions are worth keeping, but `tieredRate` should be reverted to inline (or given named parameters/an object arg), and a couple of the single-use helpers (`itemWeight`/`totalWeight`, `tierDiscount`) could be inlined without losing clarity.
