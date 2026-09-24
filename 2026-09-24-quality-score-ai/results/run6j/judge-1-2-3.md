**VERDICT: better** — but with one part that looks like metric-gaming rather than genuine simplification.

**Genuine improvements:**
- `roundMoney` and `computeSubtotal` eliminate real duplication (the same `Math.round(x*100)/100` and summing loop appeared 3–4 times) — clear win, no loss of clarity.
- The `validateOrder` split (`validateUser`/`validateItem`/`validateItems`/`validateCoupon`) turns a deeply nested, hard-to-scan function into small, independently readable/testable predicates that mirror the original branching exactly. This is the best part of the diff.
- `calculateTotal` now reads as a clean pipeline (`subtotal → tierDiscount → couponDiscount → shipping → total`) instead of a wall of mutable `let`s — easier to follow and to unit-test each step.

**Looks like gaming a complexity/duplication metric:**
- `tieredRate(years, tiers: [number, number][])` and the `SHIPPING_RATES` tuple table (`shippingCostForWeight`) replace two very explicit, self-contained if/else chains with a generic lookup over positional tuples. The "duplication" here (two similar if/else ladders) was arguably fine as duplication — each one was independently obvious. Now understanding the actual gold/member discount tiers or JP/international shipping bands requires jumping to a data table *and* a generic loop, and the tuples (`[5, 0.1]`, `[1, 300]`) aren't self-documenting without reading the helper.
- `const tier = table.find(...); return tier![1];` introduces a non-null assertion to paper over a case the compiler can't prove — a real code smell that wasn't present before. It only works because someone remembered to put `Infinity`/a catch-all in the table; nothing enforces that.
- Net effect: fewer duplicated lines and lower per-function branching count, but the two places this was applied are the actual pricing business rules — the part of the file most worth keeping literal and greppable. This reads like a refactor optimized for a static complexity/duplication score rather than for the next engineer who has to change a shipping threshold.

**Recommendation:** keep the `roundMoney`/`computeSubtotal`/validation extractions, but revert `tieredRate`/`SHIPPING_RATES` back to explicit if/else (or at minimum use named fields instead of positional tuples and drop the `!` assertion).
