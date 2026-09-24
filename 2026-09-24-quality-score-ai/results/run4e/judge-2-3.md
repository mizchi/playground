**VERDICT: worse** — the validation/subtotal/coupon decomposition is a genuine improvement, but it's undercut by two helpers that look like they exist to shrink a duplication/complexity score rather than to aid a reader.

Specifics:

**Smells like metric-gaming:**
- `tierDiscountRate(years, atLeast5, atLeast2, otherwise)` and `weightBasedRate(weight, under1, under5, under20, atLeast20)` collapse two *semantically different* if/else cascades (member rates vs. gold rates; JP vs. non-JP shipping tiers) into one shared function purely because they had the same *shape*. The thresholds (`>=5`, `>=2`, `<1`, `<5`, `<20`) are hardcoded inside the helper, but the actual values are passed positionally at each call site:
  ```ts
  tierDiscountRate(user.years, 0.1, 0.05, 0.02)
  weightBasedRate(weight, 300, 600, 1200, 3000)
  ```
  A reader can no longer tell what `0.05` or `1200` means without jumping to the function signature and counting positions. This is strictly worse than the original named `if (years >= 5) rate = 0.1;` — it trades self-documenting branches for anonymous numeric slots, and it's a textbook way to make a duplication checker happy (two blocks now "reuse" one function) while making the code harder to audit or safely modify (swap two args and nothing catches it, since they're all `number`).
- `discountFor` re-implements the same tier check (`"member"` / `"gold"`) that `calculateMemberDiscount`/`calculateGoldDiscount` already do internally — it doesn't remove duplication, it adds a third place encoding "which function handles which tier."

**Genuine improvements:**
- `round2`, `subtotalOf`, `totalWeight`, `hasFreeShipping`, and the `validateUser`/`validateItem`/`validateItems`/`validateCoupon` split are legitimate, well-named extractions that preserve behavior and make `validateOrder`/`calculateShipping` read top-down.

Net effect: the file trades a few real wins for two "reusable" helpers whose reuse is only structural (same control-flow shape), not semantic — that's the part to push back on in review.
