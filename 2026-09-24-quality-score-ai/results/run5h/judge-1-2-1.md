**VERDICT: same** — genuine wins are roughly offset by fragmentation that looks aimed at lowering per-function complexity/LOC counts rather than at readability.

Genuine improvements:
- `rateForYears` + `MEMBER_DISCOUNT_TIERS`/`GOLD_DISCOUNT_TIERS`: real duplication removed (the two discount functions were identical except for the numbers), and a table is a more honest representation of "tiered rate by years" than nested if/else.
- `calculateWeight` / `calculateSubtotal` / `roundToCents`: each has ≥2 real call sites, so extracting them removes actual duplication (subtotal was computed independently in both `calculateShipping` and `calculateTotal`).
- `SHIPPING_RATES_BY_COUNTRY` table: reasonable, though see caveat below.

Looks like metric-gaming rather than real refactoring:
- `validateUser`, `validateItem`, `validateItems`, `validateCoupon` are each called from exactly one place. Splitting `validateOrder` into four single-use functions doesn't reduce duplication — it just relocates the same branches behind extra indirection. A reader who wants to understand "what makes an order invalid" now has to jump through 4 functions instead of reading one cohesive block. This is the classic pattern of chopping up one function into many tiny ones purely to shrink each individual function's cyclomatic-complexity/line count.
- `calculateDiscount` and `calculateCouponDiscount`, extracted from `calculateTotal`, are the same story: single call site, no reuse, just branch-relocation to make `calculateTotal` itself look simpler on a per-function metric while total complexity in the file is unchanged (arguably worse, since you now pay a function-call/context-switch tax to follow the logic).

One actual regression, not just style: `baseShippingCost` uses `rates.find(...)!` — a non-null assertion. The original exhaustive if/else structurally guaranteed a fallback (`else { shipping = ... }`); the new table relies on the `Infinity` sentinel entry always being present and correctly last, with no compiler or runtime guarantee. If someone edits `SHIPPING_RATES_BY_COUNTRY` and drops that sentinel, this throws instead of the old code's guaranteed value — a silent maintainability trap introduced by the "cleanup."

Net: the tier/shipping tables are a legitimate, welcome improvement; the validation and total-calculation splits read like the same logic wearing more files' worth of indirection, likely to move the needle on a complexity-per-function metric rather than to help a human understand the code.
