VERDICT: better

**Why it's better:**
- `roundMoney` and `subtotalOf` are genuine duplicate-code extractions — the exact same three-line rounding formula and the exact same summation loop appeared three times each in the original. Pulling them out removes real duplication, not metric-driven duplication.
- Splitting `validateOrder` into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` turns one large nested function into small single-purpose ones that map cleanly onto the four independent concerns being validated (and each is now independently testable). This is a legitimate structural improvement, not just line-splitting.
- `calculateShipping` decomposing into `orderWeight`/`baseShippingRate`/`qualifiesForFreeShipping` correctly separates "how heavy is this order," "what does that weight cost to ship," and "does this user get it free" — three genuinely distinct questions that were tangled together in the original's linear procedural code.
- Early returns (`if (user.tier !== "member") return 0;`) replace `let rate = 0; if (...) {...}` mutation patterns — less state to track while reading.

**Mild concern (borderline "gaming" smell, but minor):**
- `yearsBasedRate(years, highRate, midRate, lowRate)` takes three unlabeled numeric positional args. At the call sites (`yearsBasedRate(user.years, 0.1, 0.05, 0.02)`), you can no longer tell at a glance which number is the 5-year/2-year/under-2 rate without jumping to the function definition — the original's inline if/else, though duplicated, was self-documenting at the point of use. This isn't metric-gaming per se (it does remove real duplication between member/gold discount logic), but it trades local readability for DRY-ness in a way that's a wash rather than a clear win. A small object argument (`{high, mid, low}`) would have kept the dedup benefit without losing call-site clarity.

Nothing here looks like artificial complexity-metric gaming (no pointless wrapper indirection, no splitting single logical lines apart just to lower a cyclomatic-complexity count, no fake "interfaces" with one implementation). The extractions correspond to real, reusable, nameable concepts in the domain.
