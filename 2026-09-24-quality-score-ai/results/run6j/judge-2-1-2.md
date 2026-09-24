VERDICT: same

The diff is two very different refactors bundled together, and they pull in opposite directions.

**Genuine improvements:**
- `orderSubtotal` and `roundMoney` extract real, meaningful duplication (subtotal was computed 3 times, rounding done 3 times) and give it a name. Good.
- Splitting `validateOrder` into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` is a legitimate decomposition — each piece is independently testable and the top-level function now reads as a clear checklist.
- `calculateShipping`'s early `return 0` instead of mutating a `shipping` variable is a small, honest clarity win.

**Gaming smell — `tierDiscountRate` and `shippingRateForWeight`:**
These two functions take the *rate values themselves* as positional numeric parameters:
```ts
tierDiscountRate(user.years, 0.1, 0.05, 0.02)
shippingRateForWeight(weight, 300, 600, 1200, 3000)
```
This is the classic move of parameterizing an if/else ladder purely to make a duplication/complexity checker (jscpd, SonarQube, etc.) stop flagging two structurally-similar blocks — even though the blocks weren't really duplicated logic, just the same *shape* applied to unrelated constants (member vs. gold discount tiers; JP vs. non-JP shipping tiers).

The cost is real: the original code was self-documenting at the call site (`if (user.years >= 5) rate = 0.1;` tells you the threshold and the rate together). After the refactor, the call site is four unlabeled magic numbers, and you must open `tierDiscountRate`'s body and map `highRate`/`midRate`/`lowRate` back to `>=5`/`>=2`/else by position. A transposed-argument bug (e.g. swapping mid/low) would now be silent and hard to spot in a diff. This trades a metric-friendly line count for a strictly worse call site.

Net effect: the validation split and subtotal/rounding extraction are solid wins, but they're offset by the two positional-parameter extractions, which look optimized for a duplication/complexity score rather than for a human reading the code.
