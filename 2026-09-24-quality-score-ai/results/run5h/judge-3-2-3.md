VERDICT: better

The extraction is genuine, not metric gaming: the three functions were truly identical except for which field they grouped by, and `summarizeBy` factors out that real duplication with a small, clear `keyOf` callback. The callback signature `(order, item)` is honestly justified since `summarizeByCategory` needs `item` while the other two need `order`. The `??` accumulator simplification is a legitimate, more idiomatic replacement for the manual `undefined` check. Call sites (`summarizeByCategory`, etc.) stay as clean one-liners with meaningful names, so callers don't need to understand the shared helper's internals — the abstraction pays for itself in this case.

Minor nitpick: `summarizeBy` isn't exported, so it's private plumbing, which is appropriate given it's only used internally — no over-generalization for hypothetical future callers.
