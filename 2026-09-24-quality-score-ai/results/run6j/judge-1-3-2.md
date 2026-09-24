## Assessment

**Genuine improvements** (real deduplication, not just indirection):
- `DiscountBand` tables + `tierDiscount` collapse two copy-pasted if/else chains (member/gold) into one path — a real duplicate was removed, and adding a new band or tier is now a one-line data change instead of an if/else edit in two places.
- `ITEM_WEIGHT_PER_UNIT` / `SHIPPING_RATES` tables replace nested if/else pyramids with lookup data — genuinely easier to scan and modify.
- `orderSubtotal` is reused in two places (`qualifiesForFreeShipping` and `calculateTotal`), removing a real duplicate subtotal loop.
- `validateItem` extracted into a `.flatMap` loop is a legitimate simplification over the manual index loop with nested if/else.

**Looks like metric gaming, not real simplification:**
- `tierDiscountAmount`, `couponDiscountAmount`, `qualifiesForFreeShipping`, and `validateUser` are each called from exactly **one** call site. They don't remove duplication — they just relocate branches out of `calculateTotal`/`calculateShipping`/`validateOrder` into separate functions purely to shrink those functions' line count / cyclomatic complexity. The original 4-line `if (order.user.tier === "member") ... else if ("gold") ...` in `calculateTotal` was already trivial to read in place; turning it into a jump to `tierDiscountAmount` adds a file hop for zero comprehension benefit.
- Net effect: reading "how is the total computed" now requires bouncing through `orderSubtotal` → `tierDiscountAmount` → `calculateMemberDiscount`/`calculateGoldDiscount` → `tierDiscount` → `discountRate`, plus `couponDiscountAmount`, plus `calculateShipping` → `qualifiesForFreeShipping`/`orderWeight`/`shippingCostForWeight`. That's roughly 8 hops to trace one formula that used to be ~15 linear lines in a single function. This is the classic pattern of fragmenting a function into many single-use helpers to lower a per-function complexity/LOC score while the *system's* essential complexity is unchanged (or worse, since indirection itself is cognitive load).
- The band lookups (`discountRate`, `shippingCostForWeight`) also introduce a new implicit invariant — array ordering (descending `minYears`, ascending `maxWeight`) — that isn't enforced by types and wasn't a concern in the explicit if/else version. A future editor who appends a band out of order silently breaks the logic.

**Minor smell:** `couponDiscountAmount(coupon, subtotal, discount)` takes three loosely-related primitives instead of the order/subtotal/discount already being visible in context — primitive-obsession introduced by the extraction.

VERDICT: same

Reason: A few extractions (discount bands, shipping tables, subtotal/item-validation reuse) are genuine, valuable dedup. But several other extractions (`tierDiscountAmount`, `couponDiscountAmount`, `qualifiesForFreeShipping`, `validateUser`) are single-call-site helpers that don't eliminate duplication — they just split simple, linear logic across more files/functions, which reads as chasing a low per-function complexity or short-function-length metric rather than improving actual maintainability. The two effects roughly cancel out.
