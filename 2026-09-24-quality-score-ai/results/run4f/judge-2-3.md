VERDICT: better

Both extractions collapse genuinely duplicated logic (not superficially similar code that happens to share line count) into one implementation, so a future tier or grouping dimension needs one change instead of three. `calculateTierDiscount` and `summarizeBy` are small, and the wrapper functions keep the public API and its call sites unchanged, so callers don't feel the refactor at all.

Two minor readability costs worth noting, neither is a metric-gaming red flag:
- `calculateTierDiscount(user, subtotal, tier, tiers)` re-derives `user.tier === tier` from a value the caller already knows is true at each call site (`"member"`/`"gold"`), which is a little redundant/indirect versus just gating in the caller and passing only `rate`'s inputs.
- `summarizeBy`'s `keyOf: (order, item) => string` callback is a touch more indirect than a straight loop, and `summarizeByCategory`'s unused `_order` param is a small wart (harmless, TS-idiomatic, but a reader still has to check it's unused).

Nothing here looks like gaming a duplication/complexity score — there's no artificial function-splitting, no trivial pass-through wrappers hiding real logic, and no case where the "shared" code secretly diverges between call sites. It's a legitimate extract-function refactor driven by real repeated structure.
