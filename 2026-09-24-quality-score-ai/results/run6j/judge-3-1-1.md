VERDICT: worse

The extraction replaces two clear, self-contained tables of rates with a generic-looking `weightBasedRate` helper that takes four positional numeric params with no semantic meaning attached to them at the call site. At `weightBasedRate(weight, 300, 600, 1200, 3000)` a reader has no idea which number is which without jumping back to the function signature and counting positions — the original inline `if` chains were actually easier to scan because each threshold sat next to its rate in context (JP vs. default), and the values read top-to-bottom as a table.

This has the hallmarks of duplication-metric gaming rather than a genuine abstraction:
- The "duplication" being removed is just parallel structure (two rate tables with the same shape), not repeated logic that risks drifting out of sync in a meaningful way — the thresholds (`1`, `5`, `20`) are still duplicated as magic numbers baked into the helper, just hidden one level down.
- Four unlabeled numeric parameters is a classic anti-pattern (easy to transpose two arguments, e.g. swap `medium`/`large`, with no type system or naming to catch it).
- No new behavior, validation, or shared invariant is introduced — it's a pure syntactic dedup that trades readability for a lower line/branch count.

A better refactor, if dedup were actually warranted, would use a lookup table/array of `{threshold, rate}` pairs keyed by country, or at least named parameters (an options object) so call sites remain self-documenting.
