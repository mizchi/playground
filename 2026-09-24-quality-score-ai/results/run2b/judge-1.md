**VERDICT: same**

The refactor is a mixed bag: real improvements in one area are offset by indirection introduced elsewhere that looks aimed at scoring well on duplication/complexity metrics rather than at clarity.

**Genuine improvements**
- `validateOrder` → `validateUser`/`validateItem`/`validateItems`/`validateCoupon` is a clean decomposition; each function is easy to read in isolation and the `flatMap` for item errors is idiomatic.
- `subtotalOf` correctly removes a subtotal calculation that was duplicated three times in the original.
- `calculateTotal`/`calculateShipping` reading as a short sequence of named steps (`hasFreeShipping`, `tierDiscount`, `couponDiscountFor`) is arguably nicer than the original mutable-variable soup.

**Metric-gaming smells**
- `tieredRate`/`loyaltyDiscount`: the member and gold discount ladders weren't really duplicated *logic* — they just happened to both be 3-branch if/else chains with different numbers. Collapsing them into a generic `[minYears, rate][]` walker with a separately-passed `baseRate` (instead of just modeling the base case as a `[0, rate]` tier) trades a directly-readable if/else for an indirect lookup a reader now has to trace through, plus an inconsistent data shape (fallback isn't in the table). This reads like duplication was eliminated on paper while readability went down — a classic case of gaming a duplication score.
- `baseShippingCost`: `rates.find(([limit]) => weight < limit)![1]` replaces an explicit, obvious if/else chain with a clever one-liner that relies on an `Infinity` sentinel and a non-null assertion (`!`). It's shorter and "less duplicated," but it's more cryptic and the `!` is a latent footgun if the sentinel is ever dropped.
- Coupling member and gold discount through one shared helper implies a business relationship between the two tiers' rate ladders that doesn't really exist — changing one now requires understanding the shared abstraction, not just editing a local chain.

Net effect: the validation and top-level composition changes are a real win, but the discount/shipping helpers substitute genuinely-readable branching for generic, table-driven indirection whose main benefit is fewer lines/branches for a complexity scanner — not clarity for a human maintainer. Those roughly cancel out.
