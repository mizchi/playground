Warning: no stdin data received in 3s, proceeding without it. If piping from a slow command, redirect stdin explicitly: < /dev/null to skip, or wait longer.
VERDICT: worse

The original was a clear, linear cascade of `if (weight < X) return ...` — readable top to bottom, easy to trace exactly which branch fires. The new version replaces it with a data table plus `.find()` and a non-null assertion (`!`), which:

- Adds indirection: you now have to mentally map `belowWeight` semantics ("this row applies if weight is below this value") back to the same threshold logic, but via an extra field name that doesn't self-document as clearly as an inline comparison.
- Introduces a runtime risk that didn't exist before: the `!` assumes `.find()` always succeeds. It happens to be safe only because the last tier is `Infinity`, but that safety is now implicit and fragile — if someone edits the array and drops/reorders the `Infinity` sentinel, this becomes a silent `undefined.domestic` crash instead of a compile-time-obvious `return domestic ? ... : ...`.
- Splits one cohesive piece of logic into a module-level constant plus a lookup function, for no behavioral benefit — the branching logic was already trivial and didn't need to be data-driven (no evidence it's reused elsewhere or configured externally).

This has the shape of a metric-gaming refactor: converting a short if/else chain (which trips cyclomatic-complexity or "duplicate branch pattern" linters) into a table lookup that scores better on those metrics while making the code marginally harder to read and less type-safe (non-null assertion masking a real edge case). If the tiers were genuinely going to grow, become config-driven, or be reused elsewhere, a table would be justified — but nothing in this diff suggests that need.
