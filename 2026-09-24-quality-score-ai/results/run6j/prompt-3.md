You are improving code quality in this TypeScript project (src/).
A metrics tool produced the report below. Refactor to reduce cognitive/cyclomatic complexity
and remove duplicated logic, so that the score goes up.

Rules:
- Do NOT modify src/order.test.ts. Behavior must stay identical; the tests must still pass.
- Keep every exported name and signature unchanged.
- Prefer readable code. Do not obfuscate or cram logic into one-liners just to game the metric.
- You may run: node --test src/order.test.ts

Report:
score: 95.2 (complexity 90.5, duplication 100, tests pass)
loc: 148, functions: 20

## Complex functions (cognitive > 8 or cyclomatic > 8)
- src/order.ts:42 baseShippingRate cognitive=10 cyclomatic=8

## Similar functions (similarity >= 0.8)
(none)

Previous attempts were REJECTED and rolled back. Learn from the reviewers' feedback below.
If a reported function is already readable and further change would only add indirection,
leave it as it is: a lower metric score is acceptable, a less readable change is not.
Making no change at all is an acceptable outcome.

### Rejected attempt 1
#### Candidate 1 — judge: better/same/same

Reviewer 1:
**Assessment**

This is a genuine refactor, not just metric gaming. The extractions map to real duplication that existed in the original:

- `rateByYears` + `MEMBER_TIERS`/`GOLD_TIERS` replace two near-identical nested if/else cascades that differed only in numbers — a real win. Adding a new tier or a third membership level now means editing a table, not copy-pasting a branch.
- `rateByWeight` + `JP_SHIPPING_TIERS`/`INTL_SHIPPING_TIERS` do the same for the shipping cascade.
- `roundMoney` and `subtotalOf` dedupe logic that was copy-pasted 3-4 times.
- Splitting `validateOrder` into `validateUser`/`validateItems`/`validateItem`/`validateCoupon` is a legitimate decomposition along natural boundaries, and `validateItems` correctly preserves the original's early-return-on-empty behavior via `validateItem`'s `flatMap`.

I checked behavioral equivalence on the trickier parts:
- `validateUser`'s `if (!user) return [...]` correctly preserves the original's short-circuit (avoids touching `user.id`/`user.years` on a missing user).
- `rateByWeight`/`rateByYears` depend on tier arrays being pre-sorted (ascending by weight, descending by years) — this is an implicit invariant not enforced by ty

Reviewer 2:
**VERDICT: same**

The diff is a mixed bag — genuine improvements in one area, metric-gaming indirection in another, roughly netting out.

**Genuine improvements:**
- `validateOrder` → `validateUser`/`validateItems`/`validateItem`/`validateCoupon` is a real win: each piece is independently readable and testable, and the nested if/else-with-accumulator style is gone.
- `subtotalOf`/`roundMoney` extraction removes three copies of the same reduce/round logic — legitimate DRY.
- `calculateTotal` reads top-to-bottom as a clear pipeline now (`tierDiscountFor`, `couponDiscountFor`, `calculateShipping`) instead of one long imperative block.

**Looks like gaming a complexity/duplication metric:**
- `rateByYears(years, tiers: YearTier[])` and `rateByWeight(weight, tiers: WeightTier[])` turn two simple, self-contained if/else-if chains into a generic "data table + interpreter" pattern. This is the classic move to shave per-function cyclomatic complexity and duplication scores: the original had two structurally-similar-but-independent branching blocks (easy to read in place, e.g. "5+ years → 10%"), now you have to jump to a `MEMBER_TIERS`/`GOLD_TIERS`/`JP_SHIPPING_TIERS` constant, understand a

Reviewer 3:
## Review

**Genuine improvements:**
- `roundMoney` and `subtotalOf` extraction removes real duplication (subtotal was computed independently in 3 places before, and could drift).
- Splitting `validateOrder` into `validateUser`/`validateItem`/`validateItems`/`validateCoupon` is a legitimate readability win — each rule is independently testable and the top-level function reads like a spec.
- `tierDiscountFor`/`couponDiscountFor` pulled out of `calculateTotal` are reasonable, low-risk extractions.

**The discount/shipping tier tables are metric gaming, not refactoring:**

`rateByYears(years, tiers)` and `rateByWeight(weight, tiers)` turn simple, self-contained if/else chains into "iterate a data table until a predicate matches." This is the classic move to game a cyclomatic-complexity or nesting linter: the branches don't disappear, they move into array literals, so a reader now has to jump between a type definition, a const array, and a generic lookup function to reconstruct one sentence ("gold members with 5+ years get 20% off") that used to be readable in a single `if`.

Worse, the two tables use **inconsistent, unstated ordering conventions**:
- `MEMBER_TIERS`/`GOLD_TIERS` are or

#### Candidate 2 — judge: same/same/better

Reviewer 1:
**VERDICT: same**

The diff is a mixed bag — genuine improvements in one area are offset by a metric-gaming abstraction in another.

**Genuine improvements:**
- `validateUser` / `validateItem` / `validateItems` / `validateCoupon` — this decomposition is a real win. Each function is independently readable, testable, and the early-return-on-missing-user behavior is preserved correctly.
- `computeSubtotal` / `roundMoney` — trivial, honest deduplication of literally-identical code (`subtotal * qty` sum, `Math.round(x*100)/100`). No downside.
- `calculateTierDiscount` / `calculateCouponDiscount` pulled out of `calculateTotal` — reasonable, each reads as a named step in the pipeline.

**Looks like metric-gaming, not real improvement:**
- `tieredRate(years, [[5, 0.1], [2, 0.05], [0, 0.02]])` — the original `if (years >= 5) ... else if (years >= 2) ... else ...` was completely self-documenting. The new version replaces explicit, ordered branches with an opaque array of magic tuples whose correctness depends on an *unstated* invariant (entries must be sorted descending, first match wins). This is a classic case of collapsing branches into a generic loop purely to lower a cyclomatic-complexi

Reviewer 2:
**VERDICT: same**

The change is a wash: a few extractions are genuine improvements, but several others look like they exist to satisfy a complexity/duplication scanner rather than to help a reader.

Genuine wins:
- `roundMoney` and `computeSubtotal` kill real duplication (subtotal was computed three separate times; rounding logic four times).
- `computeWeight` cleanly isolates a self-contained calculation.

Looks like metric-gaming:
- `tieredRate(years, [[5,0.1],[2,0.05],[0,0.02]])` and `SHIPPING_RATES`/`shippingCostForWeight` replace explicit if/else chains with anonymous tuple arrays. This is the classic move to dodge a duplication detector (jscpd-style tools match on structurally identical if/else blocks, not on data literals) — but it makes the business rule *less* legible: you now have to mentally re-derive "first tuple whose threshold the value clears, in this specific order" instead of reading a plain threshold ladder. The `find(...)!` non-null assertion in `shippingCostForWeight` is a real smell — it silently depends on `Infinity` always being present in the table, with no compiler or runtime guarantee if someone edits `SHIPPING_RATES` later.
- `calculateTierDiscount` and 

Reviewer 3:
**VERDICT: better** — but with one part that looks like metric-gaming rather than genuine simplification.

**Genuine improvements:**
- `roundMoney` and `computeSubtotal` eliminate real duplication (the same `Math.round(x*100)/100` and summing loop appeared 3–4 times) — clear win, no loss of clarity.
- The `validateOrder` split (`validateUser`/`validateItem`/`validateItems`/`validateCoupon`) turns a deeply nested, hard-to-scan function into small, independently readable/testable predicates that mirror the original branching exactly. This is the best part of the diff.
- `calculateTotal` now reads as a clean pipeline (`subtotal → tierDiscount → couponDiscount → shipping → total`) instead of a wall of mutable `let`s — easier to follow and to unit-test each step.

**Looks like gaming a complexity/duplication metric:**
- `tieredRate(years, tiers: [number, number][])` and the `SHIPPING_RATES` tuple table (`shippingCostForWeight`) replace two very explicit, self-contained if/else chains with a generic lookup over positional tuples. The "duplication" here (two similar if/else ladders) was arguably fine as duplication — each one was independently obvious. Now understanding the actual gold/me

#### Candidate 3 — judge: better/same/same

Reviewer 1:
**VERDICT: better**

This is a genuine, idiomatic refactor, not metric-gaming. The two biggest wins:

- **Discount tiers** (`MEMBER_DISCOUNT_BANDS`/`GOLD_DISCOUNT_BANDS` + `discountRate`/`tierDiscount`): the old `calculateMemberDiscount`/`calculateGoldDiscount` were near-identical if/else ladders — exactly the kind of duplication a real reviewer would flag. Replacing them with a lookup table + shared band-resolution function removes that duplication at the *design* level, not by renaming variables to dodge a token-similarity checker.
- **Validation split** (`validateUser`/`validateItem`/`validateItems`/`validateCoupon`): each function now maps to one error-producing concern instead of one deeply nested function doing all of them. Semantics are preserved exactly (e.g. the furniture-qty check still only fires when price/qty are already valid, matching the original nesting).
- **Shipping**: weight calc, rate-band lookup, and free-shipping eligibility are each named and testable in isolation instead of buried in one long function with three concerns interleaved. `calculateTotal` and `calculateShipping` read top-to-bottom as a sequence of named steps now, which is a real comprehension w

Reviewer 2:
## Assessment

**Genuine improvements** (real deduplication, not just indirection):
- `DiscountBand` tables + `tierDiscount` collapse two copy-pasted if/else chains (member/gold) into one path — a real duplicate was removed, and adding a new band or tier is now a one-line data change instead of an if/else edit in two places.
- `ITEM_WEIGHT_PER_UNIT` / `SHIPPING_RATES` tables replace nested if/else pyramids with lookup data — genuinely easier to scan and modify.
- `orderSubtotal` is reused in two places (`qualifiesForFreeShipping` and `calculateTotal`), removing a real duplicate subtotal loop.
- `validateItem` extracted into a `.flatMap` loop is a legitimate simplification over the manual index loop with nested if/else.

**Looks like metric gaming, not real simplification:**
- `tierDiscountAmount`, `couponDiscountAmount`, `qualifiesForFreeShipping`, and `validateUser` are each called from exactly **one** call site. They don't remove duplication — they just relocate branches out of `calculateTotal`/`calculateShipping`/`validateOrder` into separate functions purely to shrink those functions' line count / cyclomatic complexity. The original 4-line `if (order.user.tier === "member") ...

Reviewer 3:
**VERDICT: same** — genuine wins are offset by indirection that looks tuned for complexity/duplication metrics rather than real clarity.

**What's genuinely better:**
- `MEMBER_DISCOUNT_BANDS`/`GOLD_DISCOUNT_BANDS` collapsing two near-identical if/else ladders into one `discountRate` lookup is real duplication removal.
- `validateUser`/`validateItem`/`validateItems`/`validateCoupon` is a sensible decomposition — each piece is independently readable and testable, and the nesting in the original (`if user... else if items... else`) is gone.
- `orderSubtotal` is reused twice (`qualifiesForFreeShipping` and `calculateTotal`) — that's legitimate.

**What smells like metric-gaming rather than real improvement:**

1. **Single-use "extractions" with no reuse benefit**: `tierDiscountAmount` and `couponDiscountAmount` are each called exactly once, from `calculateTotal`. Splitting them out doesn't remove duplication — it just relocates the same branches into same-named functions so `calculateTotal`'s own line count / branch count drops. To understand `calculateTotal` you now have to jump to three other functions instead of reading five lines top-to-bottom. This is the classic pattern of gamin