Warning: no stdin data received in 3s, proceeding without it. If piping from a slow command, redirect stdin explicitly: < /dev/null to skip, or wait longer.
VERDICT: better

The extraction is legitimate, not metric-gaming. The three functions were genuinely identical except for one line (which key to bucket by), so collapsing them into a single `summarizeBy` helper plus three tiny one-line wrappers removes real duplication rather than just reshuffling it to dodge a linter. A few notes:

- The abstraction is honest: `keyFor(order, item)` reflects the actual variation point (category needs `item`, country/tier need `order`), so it's not an overfit abstraction forcing unrelated cases together.
- Each call site (`summarizeByCategory`, `summarizeByCountry`, `summarizeByTier`) stays a named, exported function with a clear signature — callers don't need to know about `summarizeBy` or pass lambdas themselves. That preserves the readable public API while removing the copy-pasted loop bodies.
- `result[key] = (result[key] ?? 0) + item.price * item.qty` is a reasonable, idiomatic simplification of the `if (undefined) { = 0 }` pattern — not obfuscation.
- No red flags like splitting one meaningful function into arbitrary fragments just to lower a cyclomatic-complexity or LOC-per-function score, no dead indirection, no renaming to dodge a duplicate-string detector.

Minor nit (not disqualifying): `summarizeByCategory`'s callback ignores `order` via `_order`, which is a tiny smell of forcing a uniform signature across two different shapes, but it's cheap and still clear.
