/-
# Deterministic square roots mod `p`, conditional on `BachBound`

Tonelli–Shanks is a deterministic algorithm *once a quadratic non-residue `z` is known*; the only
randomness in practice is guessing `z`.  Under GRH (via Bach) a non-residue exists below
`2 ln² p`, so a bounded search finds one and the whole procedure becomes deterministic.

* `tsLoop_sq`: the Tonelli–Shanks loop is correct, for any field, from its invariant.
* `tonelliShanks_sq` (unconditional): given any non-residue `z`, the output squares to `a`.
* `detSqrt_sq` (BachBound): the search over `z ≤ millerBound p` plus Tonelli–Shanks returns a
  square root of every square mod an odd prime `p`.
-/
import Leanmath.Miller

namespace Leanmath

section Loop

variable {F : Type*} [Field F] [DecidableEq F]

/-- Tonelli–Shanks main loop with fuel.  State: `M`, `c` (of order `2^M`), `t`, `R`. -/
def tsLoop : ℕ → ℕ → F → F → F → F
  | 0, _, _, _, R => R
  | fuel + 1, M, c, t, R =>
    if t = 1 then R
    else
      match (List.range M).find? (fun i => t ^ 2 ^ i == 1) with
      | none => R
      | some i =>
        let b := c ^ 2 ^ (M - i - 1)
        tsLoop fuel i (b ^ 2) (t * b ^ 2) (R * b)

/-- Loop invariant: `R² = a t`, `c^(2^(M-1)) = -1`, `t^(2^(M-1)) = 1`. -/
theorem tsLoop_sq (a : F) :
    ∀ (fuel M : ℕ) (c t R : F), 0 < M → M ≤ fuel →
      R ^ 2 = a * t → c ^ 2 ^ (M - 1) = -1 → t ^ 2 ^ (M - 1) = 1 →
      tsLoop fuel M c t R ^ 2 = a
  | 0, M, c, t, R, hM, hfuel, _, _, _ => by omega
  | fuel + 1, M, c, t, R, hM, hfuel, hR, hc, ht => by
    unfold tsLoop
    by_cases ht1 : t = 1
    · simp only [ht1, ↓reduceIte]; rw [hR, ht1, mul_one]
    simp only [ht1, ↓reduceIte]
    -- the least `i < M` with `t^(2^i) = 1` exists (`i = M - 1` qualifies)
    obtain ⟨i, hi⟩ : ∃ i, (List.range M).find? (fun i => t ^ 2 ^ i == 1) = some i := by
      cases h : (List.range M).find? (fun i => t ^ 2 ^ i == 1) with
      | some i => exact ⟨i, rfl⟩
      | none =>
        exfalso
        rw [List.find?_eq_none] at h
        exact h (M - 1) (List.mem_range.mpr (by omega)) (by simpa using ht)
    rw [hi]
    obtain ⟨hti, himem, hmin⟩ := List.find?_range_eq_some.mp hi
    simp only [beq_iff_eq] at hti
    have hiM : i < M := List.mem_range.mp himem
    have hi0 : 0 < i := by
      rcases Nat.eq_zero_or_pos i with h0 | h0
      · subst h0; simp at hti; exact absurd hti ht1
      · exact h0
    -- `t^(2^(i-1)) = -1`: it squares to 1 and is not 1 by minimality
    have hprev : t ^ 2 ^ (i - 1) = -1 := by
      have hsq : (t ^ 2 ^ (i - 1)) ^ 2 = 1 := by
        rw [← pow_mul, ← pow_succ, Nat.sub_add_cancel hi0]; exact hti
      have hne : t ^ 2 ^ (i - 1) ≠ 1 := by
        have := hmin (i - 1) (by omega)
        simpa using this
      exact (sq_eq_one_iff.mp hsq).resolve_left hne
    set b := c ^ 2 ^ (M - i - 1) with hb
    have hb2 : (b ^ 2) ^ 2 ^ (i - 1) = -1 := by
      rw [hb, ← pow_mul, ← pow_mul]
      have : 2 ^ (M - i - 1) * (2 * 2 ^ (i - 1)) = 2 ^ (M - 1) := by
        rw [← pow_succ', ← pow_add]; congr 1; omega
      rw [this, hc]
    apply tsLoop_sq a fuel i (b ^ 2) (t * b ^ 2) (R * b) hi0 (by omega)
    · rw [mul_pow, hR]; ring
    · exact hb2
    · rw [mul_pow, hprev, hb2]; norm_num

end Loop

/-- Tonelli–Shanks for `a ≠ 0` with a given non-residue `z` (`p - 1 = 2^s q`). -/
def tonelliShanks (p : ℕ) [Fact p.Prime] (a z : ZMod p) : ZMod p :=
  tsLoop (twoVal p) (twoVal p) (z ^ oddPart p) (a ^ oddPart p) (a ^ ((oddPart p + 1) / 2))

/-- Unconditional correctness of Tonelli–Shanks given a non-residue. -/
theorem tonelliShanks_sq {p : ℕ} [hp : Fact p.Prime] (hp2 : p ≠ 2) {a z : ZMod p}
    (ha : IsSquare a) (ha0 : a ≠ 0) (hz : ¬ IsSquare z) : tonelliShanks p a z ^ 2 = a := by
  have hp1 : 1 < p := hp.out.one_lt
  have hodd : Odd p := hp.out.odd_of_ne_two hp2
  have hs : 0 < twoVal p := twoVal_pos hodd hp1
  have hq : Odd (oddPart p) := odd_oddPart hp1
  have hz0 : z ≠ 0 := by rintro rfl; exact hz ⟨0, by simp⟩
  -- `q · 2^(s-1) = p / 2`
  have hhalf : oddPart p * 2 ^ (twoVal p - 1) = p / 2 := by
    have h' : oddPart p * 2 ^ (twoVal p - 1) * 2 = p - 1 := by
      rw [← two_pow_mul_oddPart p, mul_assoc, ← pow_succ, Nat.sub_add_cancel hs, mul_comm]
    generalize oddPart p * 2 ^ (twoVal p - 1) = x at h' ⊢
    obtain ⟨k, hk⟩ := hodd
    omega
  unfold tonelliShanks
  apply tsLoop_sq a _ _ _ _ _ hs le_rfl
  · rw [← pow_mul, Nat.div_mul_cancel (by obtain ⟨k, hk⟩ := hq; omega), pow_succ, mul_comm]
  · rw [← pow_mul, hhalf]
    exact ((ZMod.pow_div_two_eq_neg_one_or_one p hz0).resolve_left
      fun h => hz ((ZMod.euler_criterion p hz0).mpr h))
  · rw [← pow_mul, hhalf]
    exact (ZMod.euler_criterion p ha0).mp ha

/-- Deterministic square root: search `z ≤ millerBound p` for a non-residue (by Euler's
criterion), then run Tonelli–Shanks. -/
def detSqrt (p : ℕ) [Fact p.Prime] (a : ZMod p) : ZMod p :=
  if a = 0 then 0
  else
    match (List.range (millerBound p + 1)).find? (fun z : ℕ => (z : ZMod p) ^ (p / 2) == -1) with
    | some z => tonelliShanks p a z
    | none => 0

/-- **GRH corollary** (via Bach): `detSqrt` computes a square root of every square modulo an
odd prime, with no randomness. -/
theorem detSqrt_sq (hB : BachBound) {p : ℕ} [hp : Fact p.Prime] (hp2 : p ≠ 2) {a : ZMod p}
    (ha : IsSquare a) : detSqrt p a ^ 2 = a := by
  have hp1 : 1 < p := hp.out.one_lt
  have hp3 : 2 < p := lt_of_le_of_ne hp.out.two_le (Ne.symm hp2)
  have : Fact (2 < p) := ⟨hp3⟩
  unfold detSqrt
  by_cases ha0 : a = 0
  · simp only [ha0, ↓reduceIte]; ring
  simp only [ha0, ↓reduceIte]
  cases hfind : (List.range (millerBound p + 1)).find? (fun z : ℕ => (z : ZMod p) ^ (p / 2) == -1)
    with
  | none =>
    exfalso
    obtain ⟨z, hz0, hzle, hz⟩ := exists_small_nonsquare hB hp2
    have hzB : z ≤ millerBound p := by
      have : (z : ℝ) ≤ millerBound p := hzle.trans (two_log_sq_le_millerBound hp1)
      exact_mod_cast this
    have hzne : (z : ZMod p) ≠ 0 := by rintro h; exact hz ⟨0, by simp [h]⟩
    rw [List.find?_eq_none] at hfind
    apply hfind z (List.mem_range.mpr (by omega))
    simpa using (ZMod.pow_div_two_eq_neg_one_or_one p hzne).resolve_left
      fun h => hz ((ZMod.euler_criterion p hzne).mpr h)
  | some z =>
    have hz := List.find?_some hfind
    simp only [beq_iff_eq] at hz
    apply tonelliShanks_sq hp2 ha ha0
    intro hsq
    have hzne : (z : ZMod p) ≠ 0 := by
      rintro h; rw [h, zero_pow (by omega)] at hz; exact one_ne_zero (neg_eq_zero.mp hz.symm)
    rw [(ZMod.euler_criterion p hzne).mp hsq] at hz
    exact ZMod.neg_one_ne_one hz.symm

instance : Fact (Nat.Prime 10009) := ⟨by norm_num⟩

-- sanity checks: square roots of 2, 3, 5 modulo a prime with p ≡ 1 (mod 8) (the hard case)
#eval let p := 10009; [2, 3, 5].map fun a => let r := detSqrt p (a : ZMod p); (r, r ^ 2)

end Leanmath
