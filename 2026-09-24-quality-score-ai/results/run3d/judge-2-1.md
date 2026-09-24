Warning: no stdin data received in 3s, proceeding without it. If piping from a slow command, redirect stdin explicitly: < /dev/null to skip, or wait longer.
VERDICT: same

The refactor trades three simple, self-contained loops for one generic `summarizeBy` plus three one-line wrappers. Each original function was trivially readable in isolation — a reader could understand `summarizeByCategory` without knowing anything else existed. Now understanding any single summarizer requires jumping into `summarizeBy` and mentally substituting the closure, plus keeping track of the `(order, item) => string` signature and which param each wrapper ignores (`(_order, item)` vs `(order)`).

This is a classic case of deduplication driven by a duplication/complexity metric rather than genuine complexity reduction:

- The "duplicated" logic (accumulate `price * qty` into a bucket keyed by some string) is about 5 lines — below the threshold where extraction typically pays for itself.
- The extraction doesn't remove a concept, it parameterizes one, which is a different (and costlier) kind of abstraction: now every call site needs a lambda, and the shared function's cyclomatic complexity/responsibility count is unchanged, just relocated and hidden behind indirection.
- `keyOf(order, item)` forces an awkward interface — two of three callers ignore one parameter — a sign the abstraction doesn't cleanly fit all three use cases and was reverse-engineered from "how do I make these three blocks textually identical" rather than from a real shared concept.
- If a future variant needs different aggregation (e.g., count instead of sum, or a different combine step), the generic function has to grow more parameters, cascading complexity into the "shared" core — whereas before, each function could evolve independently with a one-line change.

Net effect: token/line count and a naive duplication scanner would score this as improved, but nothing here reduces genuine cognitive load — it just moves the (trivial) repetition into a layer of indirection. That's a lateral move at best, and arguably worse for a newcomer skimming the file top-to-bottom.
