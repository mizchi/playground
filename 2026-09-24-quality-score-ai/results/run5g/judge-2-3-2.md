VERDICT: worse

The diff trades three straightforward, self-contained functions for one generic `summarizeBy` helper plus three one-line wrappers passing lambdas. This is a classic dedup-for-dedup's-sake move: the "duplication" being removed was a 5-line accumulation loop, not meaningful logic, and the loop body itself (`result[key] === undefined` init, `+= item.price * item.qty`) is still copy-pasted in spirit into the shared function — it didn't actually eliminate complexity, it just relocated it and added a layer of indirection.

Concerns:
- **Loss of local reasoning**: previously each function's key was inline and obvious (`item.category`, `order.user.country`, `order.user.tier`). Now you must jump to `summarizeBy`'s signature and mentally substitute the callback to understand what any one summarizer does.
- **Inconsistent callback signature**: `keyFor(order, item)` takes both params, but two of three usages ignore `item` (`(_order, item) =>` and `(order) =>`) and one ignores `order`. The awkward `_order` placeholder is a signal the abstraction doesn't cleanly fit its use cases.
- **False generality**: the shared function invites a fourth "summarizeByX" to reuse it, but the abstraction only saves ~4 lines per case while costing an extra indirection and a slightly leaky signature. This reads like it was done to lower a duplication-detector's score rather than because the team hit real pain maintaining three copies.
- **No behavior change, no bug fix** — pure structural churn for a metric, on code that was already clear.

A more defensible version would keep the three functions but factor out just the truly identical part (e.g., a small `accumulate(map, key, amount)` helper) without forcing a single "keyFor" callback abstraction over two different data shapes.
