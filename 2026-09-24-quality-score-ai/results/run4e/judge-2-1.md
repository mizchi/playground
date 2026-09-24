VERDICT: worse

Several extractions are genuine improvements — `round2`, `subtotalOf`, `totalWeight`, `hasFreeShipping`, and splitting `validateOrder` into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` all give names to real concepts and are easy to follow in isolation.

But the two most important pieces of business logic — the tiered discount rates and the tiered shipping rates — got worse. `tierDiscountRate(years, atLeast5, atLeast2, otherwise)` and `weightBasedRate(weight, under1, under5, under20, atLeast20)` replace two clear, self-contained if/else chains with a generic helper called via **unlabeled positional numeric arguments**:

```ts
tierDiscountRate(user.years, 0.1, 0.05, 0.02)      // gold vs member? which bucket is which?
weightBasedRate(weight, 300, 600, 1200, 3000)      // JP rates
```

At the call site you can no longer tell what `0.1` or `1200` means without jumping to the helper's parameter names and mentally re-mapping. In the original code, `rate = 0.1;` sitting right under `if (user.years >= 5)` was self-documenting; now that fact is destroyed and reconstructed through positional plumbing. Since all four/five parameters are the same type (`number`), TypeScript gives zero protection against transposing two rates when someone edits this later — a real correctness risk in pricing code, which is about the worst place to introduce it.

This is a textbook example of **gaming a duplication/complexity metric**: the "duplicated" if/else structure across `calculateMemberDiscount`/`calculateGoldDiscount` and the two shipping-rate branches gets collapsed into a single small function to shrink duplicate-line and cyclomatic-complexity counts, but nothing about the domain logic was actually shared (member and gold discounts are conceptually independent policies that happen to share a *shape*, not a *rule*). The result reads worse and is more error-prone than the "duplicate" code it replaced.

Net effect: the validation and subtotal/shipping-eligibility refactors are a win, but the discount/shipping-rate helpers are a readability and safety regression in the most sensitive part of the file, so the diff as a whole is not an improvement.
