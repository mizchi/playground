VERDICT: better

The extraction is legitimate, not metric gaming. The three functions were genuinely identical except for a one-line key selection (`item.category`, `order.user.country`, `order.user.tier`), and the loop/accumulation logic (the part actually prone to bugs, like the `?? 0` initialization) was real duplicated logic, not incidental similarity. Consolidating it into `summarizeBy` with a `keyOf` callback:

- Removes a genuine risk: previously, fixing an accumulation bug (e.g., the `undefined` check logic) required updating three copies in sync; now there's one place.
- Keeps each public function trivially readable — one line each, intent obvious from the callback.
- The `keyOf: (order, item) => string` signature is a sensible, minimal abstraction — it doesn't over-generalize (no options bags, no strategy enums, no speculative extensibility beyond what's needed).

Minor nit: `summarizeByCategory`'s callback takes `_order` it doesn't use, which is a little awkward, but that's inherent to unifying the three signatures and is a reasonable, honest tradeoff rather than an artificial one.

Nothing here looks like duplication-counting gaming (e.g., no pointless indirection, no splitting one clear function into many to lower a per-function complexity score, no trivial wrapper functions added just to reduce a duplication percentage). The shared logic was real and worth sharing.
