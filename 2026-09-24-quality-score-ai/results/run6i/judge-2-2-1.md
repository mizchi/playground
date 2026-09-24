VERDICT: better

The three functions were identical except for one key expression (`item.category` vs `order.user.country` vs `order.user.tier`). Extracting `summarizeByKey(orders, keyOf)` removes genuine duplication — if the aggregation logic (the `price * qty` accumulation, or the `??`/undefined handling) ever needs to change, there's now one place to fix instead of three, and a bug fix there can't drift out of sync across copies the way it could before.

The cost is a small indirection: to fully understand `summarizeByCountry` you now have to jump into `summarizeByKey`. But since the helper is short, obviously named, and the callbacks are one-liners, that cost is minor and the trade is worth it.

Nothing here looks like gaming a metric — this isn't a case of merging unrelated branches, collapsing conditionals into unreadable one-liners, or introducing a fake "shared" abstraction between logic that wasn't actually the same. The three loops really were byte-for-byte identical except for the key, so a `keyOf` callback is the natural, idiomatic fix. The one mild wart is that `keyOf`'s signature is `(order, item) => string` even though two of the three call sites ignore `item` — a cosmetic inconsistency, not a red flag.
