Warning: no stdin data received in 3s, proceeding without it. If piping from a slow command, redirect stdin explicitly: < /dev/null to skip, or wait longer.
VERDICT: same

The change trades one readable form for another equally readable one, without a real improvement. The original if-chain is already clear and the domestic/intl mapping is obvious at a glance. The table version adds a data structure, an `Infinity` sentinel, and an `.find()` with a fallback — more moving parts to hold in your head, and the `?? SHIPPING_TIERS[...]` fallback is only needed because `.find()` returns `undefined`, which is accidental complexity introduced by the refactor itself.

This looks like a textbook case of gaming a cyclomatic-complexity or "magic numbers" metric: converting a 4-branch `if` into a lookup table doesn't make the logic easier to follow, it just moves the branching into `Array.find`, which static analyzers often don't count as complexity. The tiers array is now separated from the function that uses it (module-level, above the function), so the domestic/intl relationship per tier is *slightly* easier to eyeball as a table — that's the only marginal readability win — but it's offset by:
- Losing the direct `weight < N` readability (tier boundaries are now encoded as awkward "limit" upper bounds mixed with a sentinel `Infinity`).
- Introducing a possible-`undefined` case that has to be defensively handled, for data that structurally can never be undefined (the last tier's `Infinity` always matches).
- No new behavior, no new test surface, no reuse elsewhere — the table isn't consumed by anything else, so extracting it buys no real flexibility.

If shipping tiers were reused elsewhere or configured externally, the table would pay for itself. As a same-file, single-use refactor, it's a lateral move dressed up as a cleanup.
