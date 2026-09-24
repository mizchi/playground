**VERDICT: better**

Both extractions are legitimate DRY refactors, not metric-gaming:

- **order.ts**: `calculateTierDiscount` genuinely consolidates two implementations of the same discount formula that previously could drift out of sync (e.g. someone "fixing" rounding in one and forgetting the other). The `tier`/`tiers` parameter split is a little terse but self-explanatory from the two call sites right below it.

- **report.ts**: `summarizeBy` collapses three structurally identical loops (only the group-by key differs) into one, with a `keyOf(order, item)` callback that's a natural fit for the domain (category comes from `item`, country/tier come from `order.user`). This is the textbook "rule of three" case for extraction — real duplication, real shared invariant (accumulate `price * qty` by key), used by three call sites.

Neither change is a shallow wrapper created just to shrink a single function's line count or nesting depth — both new helpers have multiple real callers and encode one actual algorithm. The indirection cost is small (one extra hop to read `summarizeBy`/`calculateTierDiscount`) and is paid back by having a single place to fix bugs or extend behavior (e.g. adding a new discount tier or summary dimension). No red flags like unused parameters, single-caller "abstractions," or renaming-only diffs.

Minor nit, not a blocker: the `tier`/`tiers` parameter names in `calculateTierDiscount` are close enough to each other that a future reader could mistype which is which — `tierName`/`tierRates` would be clearer — but this doesn't outweigh the net readability gain.
