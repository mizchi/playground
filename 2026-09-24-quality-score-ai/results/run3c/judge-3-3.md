Warning: no stdin data received in 3s, proceeding without it. If piping from a slow command, redirect stdin explicitly: < /dev/null to skip, or wait longer.
VERDICT: worse

The original was a straightforward, linearly readable cascade of if/return statements — anyone can scan it top to bottom and immediately see the weight breakpoints and prices. The refactor replaces this with a data table plus a `.find()` lookup, but it doesn't actually reduce complexity — it just relocates it and adds indirection:

- The non-null assertion (`!`) on `.find()` is a latent bug: it's only safe because of the `Infinity` sentinel, which is a hidden invariant someone could easily break by editing the table (e.g., removing that last tier) without any type-level guarantee.
- The `belowWeight` field name is awkward and less clear than the original's plain `weight < N` comparisons — you now have to mentally map "belowWeight: 5" back to "weight < 5" rather than reading it directly.
- Domestic/international as parallel object keys duplicates structure that a simple 2D structure or two arrays would express more directly; it doesn't reduce duplication so much as rename it.
- This has the shape of a change made to satisfy a cyclomatic-complexity or "avoid nested if" linter rule rather than to genuinely help a reader — the branching logic didn't go away, it moved into `.find()`'s predicate and the array literal, with an extra layer of indirection and a new failure mode (the `!`) added in exchange.

Net effect: same behavior, slightly more code, a new non-null assertion risk, and no real gain in clarity — a reviewer should push back on this as metric-gaming rather than genuine simplification.
