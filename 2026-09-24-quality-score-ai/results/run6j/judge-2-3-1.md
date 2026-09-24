**VERDICT: same** — real improvements in one area are offset by metric-gaming in another.

**Genuine improvements:**
- `roundMoney`, `subtotalOf`, `weightFor` are honest, well-named, single-purpose extractions that remove real duplication (subtotal was computed 3 separate ways before) and are independently testable/reusable.
- Splitting `validateOrder` into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` is a legitimate decomposition — each function reads as its own unit test target, and the top-level function now reads as a clear summary (`return [...validateUser(...), ...validateItems(...), ...validateCoupon(...)]`).

**This is where it's gaming a complexity/duplication metric:**

`tierRate(years, highRate, midRate, lowRate)` and `shippingRateFor(weight, tiny, small, medium, large)` are the tell. Both take the *shape* of an if/else-if chain (same branch structure) and factor it into a generic helper called with raw positional numeric literals:

```ts
tierRate(user.years, 0.1, 0.05, 0.02)
shippingRateFor(weight, 300, 600, 1200, 3000)
```

This is coincidental duplication, not real duplication — member rates and gold rates are different domain concepts that happen to have the same branch shape (`>=5`, `>=2`, else). Merging them into a "shared" function:

- Deletes the self-documentation the original had (`if (user.years >= 5) { rate = 0.1; }` was readable in place; `tierRate(user.years, 0.1, 0.05, 0.02)` requires memorizing that params are ordered high→mid→low with no names or types enforcing it).
- Introduces a silent-failure risk that didn't exist before: transpose `midRate`/`lowRate` at a call site and you get a wrong discount with no compiler or lint error — pure primitive obsession / shotgun parameters.
- Buys nothing in exchange: the two call sites aren't kept in sync by anything meaningful; if gold discount logic needs a 4th tier tomorrow, this "shared" abstraction has to be abandoned or awkwardly extended (e.g. optional params), which is the classic sign the abstraction was never load-bearing.

This is a textbook pattern for gaming a cyclomatic-complexity or duplication (jscpd/SonarQube-style) score: each caller function's branch count drops, and the tool sees the two near-identical if-chains merged into "reused" code — but a reader now has to jump to the helper, then jump back to the call site and count positional args to recover information the original code stated inline.

**Net:** the validation split and the small pure helpers (`roundMoney`, `subtotalOf`, `weightFor`) are real wins; `tierRate`/`shippingRateFor` are a regression dressed up as reuse. They roughly cancel out, so overall readability/maintainability is a wash, not a clear improvement.
