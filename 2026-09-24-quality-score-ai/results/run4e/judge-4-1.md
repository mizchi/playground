**VERDICT: better**

The diff is a clean decomposition, not metric gaming:

- `round2` and `subtotalOf` eliminate real duplication (3 identical rounding call-sites, 2 identical subtotal loops) — these are genuine shared behaviors, not arbitrary line-splitting.
- `validateOrder` was doing four unrelated jobs (user/items/coupon validation) in one function with nested if/else. Splitting into `validateUser`, `validateItem`, `validateItems`, `validateCoupon` gives each piece a name that documents its purpose, and each is independently testable/reusable (e.g. `validateItem` could be unit-tested directly, which the monolith couldn't).
- I traced the control flow of each extracted validator against the original (early-return-on-falsy-user, price/qty/furniture else-if chain, empty-items short-circuit, coupon length-then-regex ordering) — behavior and error-message order are preserved exactly.

Nothing here reads as gaming: there's no trivial "extract single statement into a function to shave a complexity score" or artificial wrapper that adds indirection without removing duplication. Every extracted function corresponds to a coherent domain concept (a rounding op, a subtotal calc, a validation rule for one entity), and the call sites read more clearly for it (`calculateTotal` now reads as "validate → subtotal → discount → shipping → round" instead of being interleaved with a raw loop).

Minor nit, not a real complaint: `validateUser`'s early-return-with-array-literal style vs. building up `errors` is a bit inconsistent internally, but it's consistent with how `validateItems`/`validateCoupon` also short-circuit, so it reads fine as a house style.
