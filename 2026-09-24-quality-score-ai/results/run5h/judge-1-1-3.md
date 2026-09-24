**VERDICT: worse**

The validation split (`validateUser`/`validateItem`/`validateItems`/`validateCoupon`) and the `calculateTotal`/`discountForUser`/`couponDiscountFor` extraction are genuine, low-risk improvements — smaller functions, same logic, no lost meaning.

But the discount/shipping helpers are a textbook case of restructuring code to *look* less duplicated while actually hiding a real semantic difference — this is the part to flag:

- `tieredRate(years, tiers)` walks `tiers` **descending** and matches with `years >= minYears`.
- `tieredFee(weight, tiers, fallback)` walks `tiers` **ascending** and matches with `weight < maxWeight`, plus a separate `fallback` param for the "else" case that `tieredRate` handles by just appending `[0, rate]` to its own list.

These are two different algorithms (different sort order, different comparison operator, different miss-handling) wearing matching "tiered" names. A tool counting duplicate lines or branch depth would see this as DRY-ing up near-identical if/else-if chains into one reusable pattern — but it isn't one pattern, it's two, and calling them by the same name invites a future engineer to reuse `tieredRate`'s mental model (`>=`, descending, no fallback arg) when touching `tieredFee`, or vice versa. That's a real bug-injection risk in the money-calculation path, not a style nit.

On top of that, the original if/else-if chains were self-contained and readable in place (`weight < 1 → 300`). The refactor replaces that with unlabeled numeric tuples (`[1, 300], [5, 600], [20, 1200]`) that require jumping to the helper definition to learn what the two numbers even mean and which comparison applies. You've traded "slightly repetitive but obvious" for "compact but you must hold two different lookup semantics in your head."

Net effect: cyclomatic complexity and duplicate-line counts almost certainly drop, but the most bug-sensitive logic (pricing/shipping tiers) is now harder to verify at a glance and carries a latent trap from the mismatched `tieredRate`/`tieredFee` contracts. That's a metric win bought with a real maintainability cost, which is why I'd call the overall diff worse despite the solid validation cleanup.
