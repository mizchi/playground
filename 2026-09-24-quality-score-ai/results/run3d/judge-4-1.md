Warning: no stdin data received in 3s, proceeding without it. If piping from a slow command, redirect stdin explicitly: < /dev/null to skip, or wait longer.
VERDICT: better

The extraction is genuine, not metric-gaming: the three functions were byte-for-byte identical except for one key expression, so factoring out `summarizeBy(orders, keyFor)` removes real duplication rather than just reshuffling it to dodge a linter. The `result[key] = (result[key] ?? 0) + ...` simplification is also a legitimate readability win over the explicit `undefined` check.

Minor nitpick: `keyFor(order, item)` takes both params even though `summarizeByCategory` ignores `order` and the other two ignore `item` — slightly loose signature, but it's the natural shape needed to cover all three keys and is trivial to read at 4 lines. No signs of gaming (no pointless indirection, no dummy classes/interfaces, no splitting for line-count reasons) — this is a normal, sensible DRY refactor.
