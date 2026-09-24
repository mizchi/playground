VERDICT: same

The extraction is honest deduplication, not metric-gaming — the three functions really were copy-pasted logic, and `summarizeBy` is a legitimate shared helper. But readability is roughly a wash: you trade three self-contained, trivially-understandable functions for one generic helper plus three one-liners, and now understanding `summarizeByCountry` requires jumping to `summarizeBy` and mentally substituting the `keyOf` closure. The `(_order, item)` / `(order)` signature asymmetry (unused param in one lambda) is a minor smell suggesting the abstraction's shape was driven by wanting one shared function rather than by a natural domain concept.

Nothing here looks like deliberate complexity/duplication-metric gaming (no artificial splitting into tiny functions just to lower a cyclomatic count, no fake indirection) — it's a genuine, reasonable refactor, just a marginal one.
