VERDICT: better

Reasoning: The extraction is genuine, not metric-gaming. `round2` and `subtotalOf` remove real duplication (the same rounding formula appeared three times and the same subtotal loop appeared twice), and both are used at multiple call sites with clear names. The `validateOrder` split follows a natural seam — user/items/coupon are independent concerns with no shared state — and each helper is independently readable and testable, with `validateItem`/`validateItems` cleanly separating per-item and collection-level logic. The early-return style in `validateUser`/`validateCoupon` also flattens nesting that was previously 3-4 levels deep, which is a real readability win.

Minor concerns, not blockers:
- `validateUser`'s early return for `!user` changes shape slightly from the original (which still ran items/coupon checks when user was missing) — actually checking the diff, original also just skipped straight to items when `!order.user`, so behavior is preserved. Fine.
- `subtotalOf` being defined above the functions that use it but below the type declarations is a minor ordering nit, not a real issue.
- None of the extracted functions are trivial wrappers created only to reduce a line/branch count in a single call site — each has 2+ call sites or a clear semantic boundary, so this doesn't look like gaming cyclomatic complexity or duplication tooling.
