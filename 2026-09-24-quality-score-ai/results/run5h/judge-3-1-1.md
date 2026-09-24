VERDICT: better

The extraction is legitimate, not metric-gaming. The three functions were genuinely identical except for which key they grouped by — a textbook case for a `keyOf` extraction function. This isn't a cosmetic dedup (like factoring out something that only *looked* similar); the shared loop, the `??`/default-init logic, and the accumulation (`price * qty`) were byte-for-byte duplicated three times, and a bug fix or behavior change (e.g., switching to `Map`, adding a filter, changing the price calc) previously required three synchronized edits. Now it requires one.

A couple of minor points worth a second look, not blockers:
- The `keyOf` callback takes `(order, item)` but two of the three call sites ignore `order` or `item` — fine, but it signals the abstraction is slightly wider than any current caller needs (fully justified only if a future summarizer needs both, which grouping-by-key summaries often do, so this is a reasonable bet, not overreach).
- Naming: `summarizeBy` is a little generic: `summarizeByKey` or `groupSumBy` would be marginally clearer about what it computes (sum of price*qty per key), but this is a nitpick, not a maintainability issue.

Nothing here reads as gaming a duplication/complexity score — no meaningless helper wrapping trivial code, no artificial line-splitting, no indirection that doesn't pay for itself. The behavior-determining logic (grouping key) is genuinely the only axis of variation, and it's now expressed as data (a function parameter) rather than copy-pasted control flow.
