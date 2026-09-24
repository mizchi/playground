**VERDICT: better**

Most of this refactor is genuine: `subtotalOf`/`roundMoney` eliminate real duplication used in 4+ places, and splitting `calculateShipping` into `orderWeight` → `baseShippingRate` → `qualifiesForFreeShipping` turns a deeply-nested wall of `if/else` into a readable pipeline. Same for `validateUser`/`validateItem`/`validateItems`/`validateCoupon` — each concern is now named and independently testable instead of buried in one 40-line function.

One extraction looks like it exists to shrink a duplication/complexity metric rather than to help a reader, and actually makes the code *worse*:

```ts
function yearsBasedRate(years: number, highRate: number, midRate: number, lowRate: number): number
...
yearsBasedRate(user.years, 0.1, 0.05, 0.02)   // member
yearsBasedRate(user.years, 0.2, 0.15, 0.1)    // gold
```

The two tier-discount blocks were structurally similar (a duplication scanner would flag them), but the original was self-documenting — you read `rate = 0.1` two lines below `user.years >= 5` inside `calculateMemberDiscount`, so meaning was local. Now the meaning lives at the call site as three bare positional floats (`0.1, 0.05, 0.02`) whose mapping to "high/mid/low" you have to trust or go verify in the function body — a transposed pair of args (e.g. swapping `0.2` and `0.15` for gold) would be a silent, easy-to-miss bug that no type system catches. This is a textbook "collapse near-duplicate branches into a parameterized helper" move that reduces a duplication/complexity score without adding real clarity — arguably it's a step backward in reviewability since the domain semantics (member vs. gold rate tables) are stripped out of the names.

Everything else in the diff is legitimate simplification; only `yearsBasedRate` reads as metric-driven rather than reader-driven.
