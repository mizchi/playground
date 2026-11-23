/-
# Miller's deterministic primality test, conditional on `BachBound`

Structure of the classical proof (Miller 1976, Bach 1990), split so that GRH enters exactly once:

* `strongLiars_le_proper` (elementary, unconditional; Monier–Rabin): for odd composite `n` the
  strong liars lie in a proper subgroup of `(ZMod n)ˣ`.            -- TODO (`sorry`)
* `exists_witness_le_genBound` (unconditional): so some witness is `≤ G(n)`.
* `exists_small_witness` (BachBound): so some witness is `≤ 2 ln² n`.

Experiment `explore/gn.c` checks `W(n) ≤ G(n) ≤ 2 ln² n` numerically for `n ≤ 4·10⁵`.

STATUS: not compiled yet (see `GenBound.lean`).
-/
import Leanmath.GenBound

namespace Leanmath

/-- `a` is a strong liar for `n`: writing `n - 1 = 2^s · d` with `d` odd,
`a^d = 1` or `a^(2^r d) = -1` for some `r < s`. -/
def IsStrongLiar (n : ℕ) (a : ZMod n) : Prop :=
  let s := (n - 1).factorization 2
  let d := (n - 1) / 2 ^ s
  a ^ d = 1 ∨ ∃ r < s, a ^ (2 ^ r * d) = -1

/-- Monier–Rabin: for odd composite `n` the strong liars are contained in a proper subgroup.
(If `n` is not Carmichael, take the Fermat liars `{a | a^(n-1) = 1}`; if it is, take
`{a | a^(2^k d) = ±1}` for the largest `k` admitting a liar with `a^(2^k d) = -1`, and use CRT
to show it is proper.)  TODO: formalize. -/
theorem strongLiars_le_proper {n : ℕ} (hodd : Odd n) (h1 : 1 < n) (hc : ¬ n.Prime) :
    ∃ H : Subgroup (ZMod n)ˣ, H ≠ ⊤ ∧ ∀ u : (ZMod n)ˣ, IsStrongLiar n u → u ∈ H := by
  sorry

/-- Unconditional: some base `a ≤ G(n)` is a Miller witness for odd composite `n`. -/
theorem exists_witness_le_genBound {n : ℕ} (hodd : Odd n) (h1 : 1 < n) (hc : ¬ n.Prime) :
    ∃ a : ℕ, 0 < a ∧ a ≤ genBound n ∧ ¬ IsStrongLiar n a := by
  haveI : NeZero n := ⟨by omega⟩
  haveI : Fact (1 < n) := ⟨h1⟩
  obtain ⟨H, hH, hliar⟩ := strongLiars_le_proper hodd h1 hc
  obtain ⟨u, hle, hu⟩ := exists_not_mem_le_genBound H hH
  refine ⟨(u : ZMod n).val, ?_, hle, ?_⟩
  · exact Nat.pos_of_ne_zero fun h0 => u.ne_zero ((ZMod.val_eq_zero _).mp h0)
  · rw [ZMod.natCast_zmod_val]
    exact fun h => hu (hliar u h)

/-- **GRH corollary** (via Bach): Miller's test with bases `a ≤ 2 ln² n` is a correct
deterministic primality test. -/
theorem exists_small_witness (hB : BachBound) {n : ℕ} (hodd : Odd n) (h1 : 1 < n)
    (hc : ¬ n.Prime) :
    ∃ a : ℕ, 0 < a ∧ (a : ℝ) ≤ 2 * Real.log n ^ 2 ∧ ¬ IsStrongLiar n a := by
  obtain ⟨a, ha0, hle, hw⟩ := exists_witness_le_genBound hodd h1 hc
  refine ⟨a, ha0, ?_, hw⟩
  calc (a : ℝ) ≤ genBound n := by exact_mod_cast hle
    _ ≤ 2 * Real.log n ^ 2 := hB n h1

/-! ## An executable version -/

/-- `m = 2^s · d` with `d` odd (for `m > 0`). -/
def splitTwo (m : ℕ) : ℕ × ℕ :=
  if h : m ≠ 0 ∧ m % 2 = 0 then
    let r := splitTwo (m / 2)
    (r.1 + 1, r.2)
  else (0, m)
termination_by m
decreasing_by omega

def powMod (a e n : ℕ) : ℕ :=
  if h : e = 0 then 1 % n
  else
    let r := powMod (a * a % n) (e / 2) n
    if e % 2 = 1 then r * a % n else r
termination_by e
decreasing_by omega

/-- Strong probable prime test to base `a` (for odd `n > 2`). -/
def sprp (n a : ℕ) : Bool :=
  let (s, d) := splitTwo (n - 1)
  let x := powMod a d n
  x == 1 || x == n - 1 ||
    (List.range s).tail.any fun r => powMod a (2 ^ r * d) n == n - 1

/-- `2 (ln n)² ≤ 2 (⌊log₂ n⌋ + 1)²`, a computable over-approximation of Bach's bound. -/
def millerBound (n : ℕ) : ℕ := 2 * (Nat.log 2 n + 1) ^ 2

/-- Miller's deterministic test.  Correct for all `n` under `BachBound` (TODO: state and prove
`millerDet n = true ↔ n.Prime` from `exists_small_witness` and the converse for primes). -/
def millerDet (n : ℕ) : Bool :=
  if n < 2 then false
  else if n % 2 = 0 then n == 2
  else (List.range (min (millerBound n) (n - 1) + 1)).all fun a =>
    a < 2 || (Nat.gcd a n == 1 && sprp n a)

-- sanity checks
#eval (List.range 60).filter millerDet           -- primes < 60
#eval millerDet 3215031751                      -- composite, 2/3/5/7-strong pseudoprime
#eval millerDet 2147483647                      -- 2^31 - 1, prime

end Leanmath
