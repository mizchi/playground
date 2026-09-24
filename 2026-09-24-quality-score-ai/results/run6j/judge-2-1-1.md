VERDICT: worse

The validate* split (validateUser/validateItems/validateItem/validateCoupon) and the roundMoney/orderSubtotal/itemWeight extractions are genuine, legible improvements — each pulls out logic that's conceptually distinct and reused, and the call sites read fine.

But `tierDiscountRate(years, highRate, midRate, lowRate)` and `shippingRateForWeight(weight, tiny, small, medium, large)` are the tell. These two functions don't actually share behavior between their call sites — they share *shape* (a threshold ladder), and the "dedup" was achieved by turning meaningful named branches into a generic helper called with **bare positional numeric literals**:

```ts
tierDiscountRate(user.years, 0.1, 0.05, 0.02)
shippingRateForWeight(weight, 300, 600, 1200, 3000)
```

This is classic duplication/complexity-metric gaming: a static analyzer sees two near-identical if/else-if chains collapsed into one function and scores it as reduced duplication and lower cyclomatic complexity, but a human reading `calculateMemberDiscount` now has to jump to another function and count parameter positions to confirm `0.1` means "5+ years" rather than misreading which literal maps to which tier. There's no type or label protecting against a transposed argument (e.g. swapping `midRate`/`lowRate` at a call site would silently produce wrong discounts with no compiler complaint). The original inline if/else, while textually repeated across two functions, was self-contained and trivially auditable at a glance — you didn't need to hold two files in your head to verify correctness.

If the goal was genuine dedup, a lookup table (`const MEMBER_RATES = [{minYears: 5, rate: 0.1}, ...]`) or at least an object-shaped argument (`{high, mid, low}`) would preserve readability while still removing duplication. As written, this trades a small, harmless duplication for a real increase in bug risk in the pricing/shipping-critical path — that's a net loss for maintainability even though it looks good on a duplication/complexity dashboard.
