VERDICT: worse

The extraction is thin abstraction chasing a duplication metric, not a real readability win:

- The three original functions were self-explanatory: "sum item price×qty grouped by category/country/tier." Now each caller passes an opaque lambda (`(_order, item) => item.category`, `(order) => order.user.tier`) into a generically-named `summarizeByKey`, forcing readers to jump to the helper and mentally substitute the callback to know what each function does.
- `keyOf(order, item)` takes both `order` and `item` but two of the three call sites ignore `item` (note the `_order`/unused-param noise). That's a signature shaped to fit the least common denominator of three cases, not a natural abstraction — a classic sign of DRY-for-DRY's-sake.
- The original triplication was genuinely trivial (3 near-identical 10-line loops) — the kind of duplication that's cheap to read and cheap to keep in sync, versus an indirection that costs a level of misdirection on every read.
- Nothing about correctness, testability, or extensibility improved; grouping key logic didn't need to be pulled out for any future requirement in evidence here — it reads like it was done to reduce a duplication/complexity score.

If a fourth or fifth grouping dimension were actually needed, this pattern would earn its keep; as-is, for three fixed call sites it trades clear, flat code for indirection.
