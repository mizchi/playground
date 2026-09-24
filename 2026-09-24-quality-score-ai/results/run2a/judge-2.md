**VERDICT: better** — mostly. The validation split (`validateUser`/`validateItem`/`validateItems`/`validateCoupon` composed via array spread) and the extraction of `calculateSubtotal`/`calculateWeight`/`roundMoney`/`baseShippingRate` remove genuine duplication (the same subtotal loop appeared twice; item validation had deep nested `else` chains) and each new function has a single, obvious job. `calculateTotal` and `calculateShipping` read top-to-bottom now instead of requiring you to trace mutable `let` state through branches.

**One thing looks like metric-gaming, not real refactoring:** `tierRateByYears(rateAtFive, rateAtTwo, rateBase, years)`.

```ts
const rate = user.tier === "member" ? tierRateByYears(0.1, 0.05, 0.02, user.years) : 0;
...
const rate = user.tier === "gold" ? tierRateByYears(0.2, 0.15, 0.1, user.years) : 0;
```

The member and gold discount schedules only happen to share the same *shape* (an if/else-if on `years >= 5` / `>= 2`), not the same *meaning*. Before, each function was self-documenting: you could read `calculateMemberDiscount` and see "members get 10%/5%/2% by tenure" in one place. Now that's been collapsed into a generic helper called with four positional numeric literals, so at the call site you can't tell which number is which without jumping into `tierRateByYears`'s signature and mentally re-binding `rateAtFive`→0.1, `rateAtTwo`→0.05, `rateBase`→0.02. That's a textbook case of merging *coincidentally similar* control flow into a shared parameterized function purely to shrink a duplicate-code-block count — it adds indirection without adding any real shared logic, and if member/gold rules ever diverge in shape (e.g., gold gets a 4th tier), the abstraction has to be unwound anyway.

Net: the rest of the diff is a solid, well-scoped refactor; that one helper is the piece to push back on in review.
