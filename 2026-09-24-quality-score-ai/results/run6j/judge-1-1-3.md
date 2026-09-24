## Review

**Genuine improvements:**
- `roundMoney` and `subtotalOf` extraction removes real duplication (subtotal was computed independently in 3 places before, and could drift).
- Splitting `validateOrder` into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` is a legitimate readability win — each rule is independently testable and the top-level function reads like a spec.
- `tierDiscountFor`/`couponDiscountFor` pulled out of `calculateTotal` are reasonable, low-risk extractions.

**The discount/shipping tier tables are metric gaming, not refactoring:**

`rateByYears(years, tiers)` and `rateByWeight(weight, tiers)` turn simple, self-contained if/else chains into "iterate a data table until a predicate matches." This is the classic move to game a cyclomatic-complexity or nesting linter: the branches don't disappear, they move into array literals, so a reader now has to jump between a type definition, a const array, and a generic lookup function to reconstruct one sentence ("gold members with 5+ years get 20% off") that used to be readable in a single `if`.

Worse, the two tables use **inconsistent, unstated ordering conventions**:
- `MEMBER_TIERS`/`GOLD_TIERS` are ordered highest-to-lowest and matched with `>=` (first match wins going down).
- `JP_SHIPPING_TIERS`/`INTL_SHIPPING_TIERS` are ordered lowest-to-highest and matched with `<` via `.find()`.

Nothing in the types or code enforces or documents this — it's an implicit invariant a future editor adding a tier can easily violate silently (e.g. inserting a tier out of order produces wrong results with no error). The original nested if/else, while more "complex" by a metric tool, was actually easier to verify correct at a glance and impossible to get subtly wrong by ordering.

This pattern (generic table + lookup helper replacing a short conditional chain, for a case with only 2-4 call sites and no evidence of reuse elsewhere) is a textbook way to lower a static complexity score while making the code harder to trace.

VERDICT: same
Reason: The validation split and dedup helpers (`roundMoney`, `subtotalOf`) are real, valuable simplifications, but they're offset by the tier-table indirection in `calculateMemberDiscount`/`calculateGoldDiscount`/`calculateShipping`, which trades transparent, verifiable conditionals for generic data+lookup abstractions with an unenforced (and inconsistent) ordering invariant — a pattern more consistent with gaming a complexity/duplication metric than with improving comprehension.
