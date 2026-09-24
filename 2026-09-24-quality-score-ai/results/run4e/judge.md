**VERDICT: better**

The diff is a legitimate decomposition along genuine seams, not just churn to make a metric tool happy:

- `round2` and `subtotalOf` eliminate real duplicated logic (rounding was copy-pasted 3x, subtotal-summing 2x with a redundant loop inside `calculateShipping`). These are textbook DRY extractions with obvious names and single call sites worth collapsing.
- The `member` shipping branch (`order.ts:82-84`) goes from a 6-line nested loop+if to a one-line boolean condition — a real complexity reduction, not cosmetic.
- `validateOrder` splitting into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` follows the actual domain shape of the validation (user, items, coupon are independent concerns). Each function is independently readable, testable, and exportable in isolation, which the monolithic version wasn't. Behavior is preserved exactly, including the subtle "only one coupon error at a time" and "furniture check only when price/qty are valid" semantics (verified: the original's nested `else { if (...) }` for furniture became a flattened `else if`, which is equivalent, not a behavior change).

On the "gaming a metric" question: splitting `validateOrder` into four functions will certainly lower per-function cyclomatic-complexity and LOC numbers, which is the kind of thing that can be abused (e.g., extracting single branches into pointlessly named one-off functions just to shrink a parent function's score). But here the split boundaries are semantic (user/items/coupon), each helper does non-trivial, coherent work, and there's no leftover dead code, no trivial wrapper functions, and no renaming-only changes. This reads as an engineer improving structure who also happened to improve the score, not the reverse.

Minor nit, not a concern: `subtotalOf` could be a one-line `.reduce()`, but the manual loop is fine and consistent with the surrounding style.
