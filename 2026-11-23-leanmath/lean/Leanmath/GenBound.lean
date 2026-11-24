/-
# Unconditional facts about `G(n)`, and their GRH corollaries

The pattern behind every "GRH derandomizes X" result:

1. (elementary, unconditional) the bad inputs of X lie in a proper subgroup `H < (ZMod n)ˣ`;
2. (elementary, unconditional) a proper subgroup misses some unit `≤ G(n)`;
3. (analytic, GRH) `G(n) ≤ 2 ln² n`.

This file contains step 2 and, as a worked instance, the least quadratic non-residue.

-/
import Leanmath.Hypotheses

namespace Leanmath



theorem genBound_spec (n : ℕ) [NeZero n] :
    Subgroup.closure {u : (ZMod n)ˣ | (u : ZMod n).val ≤ genBound n} = ⊤ := by
  have hne : {x : ℕ | Subgroup.closure {u : (ZMod n)ˣ | (u : ZMod n).val ≤ x} = ⊤}.Nonempty := by
    refine ⟨n, ?_⟩
    have huniv : {u : (ZMod n)ˣ | (u : ZMod n).val ≤ n} = Set.univ := by
      ext u
      simp only [Set.mem_ofPred_eq, Set.mem_univ, iff_true]
      exact (ZMod.val_lt (u : ZMod n)).le
    show Subgroup.closure {u : (ZMod n)ˣ | (u : ZMod n).val ≤ n} = ⊤
    rw [huniv, Subgroup.closure_univ]
  exact Nat.sInf_mem hne

/-- Step 2: every proper subgroup misses a unit of size at most `G(n)`. -/
theorem exists_not_mem_le_genBound {n : ℕ} [NeZero n] (H : Subgroup (ZMod n)ˣ) (hH : H ≠ ⊤) :
    ∃ u : (ZMod n)ˣ, (u : ZMod n).val ≤ genBound n ∧ u ∉ H := by
  by_contra h
  push Not at h
  apply hH
  rw [eq_top_iff, ← genBound_spec n, Subgroup.closure_le]
  intro u hu
  exact h u hu

/-- The subgroup of squares of units. -/
def squares (n : ℕ) : Subgroup (ZMod n)ˣ := (powMonoidHom 2 : (ZMod n)ˣ →* (ZMod n)ˣ).range

/-- Unconditional: the least quadratic non-residue mod an odd prime is at most `G(p)`. -/
theorem exists_nonsquare_le_genBound {p : ℕ} [hp : Fact p.Prime] (hp2 : p ≠ 2) :
    ∃ a : ℕ, 0 < a ∧ a ≤ genBound p ∧ ¬ IsSquare (a : ZMod p) := by
  -- the squares form a proper subgroup
  have hchar : ringChar (ZMod p) ≠ 2 := by rwa [ZMod.ringChar_zmod_n]
  obtain ⟨x, hx⟩ := FiniteField.exists_nonsquare hchar
  have hx0 : x ≠ 0 := by rintro rfl; exact hx ⟨0, by simp⟩
  have hproper : squares p ≠ ⊤ := by
    intro htop
    have : Units.mk0 x hx0 ∈ squares p := htop ▸ Subgroup.mem_top _
    obtain ⟨v, hv⟩ := this
    apply hx
    refine ⟨(v : ZMod p), ?_⟩
    have := congrArg (fun w : (ZMod p)ˣ => (w : ZMod p)) hv
    simpa [powMonoidHom_apply, sq] using this.symm
  obtain ⟨u, hle, hu⟩ := exists_not_mem_le_genBound (squares p) hproper
  refine ⟨(u : ZMod p).val, ?_, hle, ?_⟩
  · exact Nat.pos_of_ne_zero fun h0 => u.ne_zero ((ZMod.val_eq_zero _).mp h0)
  · rintro ⟨r, hr⟩
    rw [ZMod.natCast_zmod_val] at hr
    have hr0 : r ≠ 0 := by rintro rfl; simp at hr
    apply hu
    refine ⟨Units.mk0 r hr0, ?_⟩
    ext
    simp [powMonoidHom_apply, sq, hr]

/-- **GRH corollary** (via Bach): deterministic search over `a ≤ 2 ln² p` finds a quadratic
non-residue, hence Tonelli–Shanks becomes a deterministic polynomial-time square root. -/
theorem exists_small_nonsquare (hB : BachBound) {p : ℕ} [hp : Fact p.Prime] (hp2 : p ≠ 2) :
    ∃ a : ℕ, 0 < a ∧ (a : ℝ) ≤ 2 * Real.log p ^ 2 ∧ ¬ IsSquare (a : ZMod p) := by
  obtain ⟨a, ha0, hle, hns⟩ := exists_nonsquare_le_genBound hp2
  refine ⟨a, ha0, ?_, hns⟩
  calc (a : ℝ) ≤ genBound p := by exact_mod_cast hle
    _ ≤ 2 * Real.log p ^ 2 := hB p hp.out.one_lt

/-- The subgroup of `ℓ`-th powers of units. -/
def powers (n ℓ : ℕ) : Subgroup (ZMod n)ˣ := (powMonoidHom ℓ : (ZMod n)ˣ →* (ZMod n)ˣ).range

/-- For `ℓ` prime with `ℓ ∣ p - 1`, the `ℓ`-th powers are a proper subgroup (cyclicity of
`(ZMod p)ˣ` has an element of order `ℓ` by Cauchy, so `x ↦ x^ℓ` is not injective, hence not
surjective).  Hence `n_ℓ(p) ≤ G(p)` and, under GRH, deterministic `ℓ`-th roots mod `p`. -/
theorem powers_ne_top {p ℓ : ℕ} [Fact p.Prime] (hℓ : ℓ.Prime) (hdvd : ℓ ∣ p - 1) :
    powers p ℓ ≠ ⊤ := by
  intro htop
  have : Fact ℓ.Prime := ⟨hℓ⟩
  have hcard : ℓ ∣ Nat.card (ZMod p)ˣ := by rwa [Nat.card_eq_fintype_card, ZMod.card_units p]
  obtain ⟨g, hg⟩ := exists_prime_orderOf_dvd_card' ℓ hcard
  have hsurj : Function.Surjective (powMonoidHom ℓ : (ZMod p)ˣ →* (ZMod p)ˣ) :=
    MonoidHom.range_eq_top.mp htop
  have hinj := Finite.injective_iff_surjective.mpr hsurj
  have h1 : g = 1 := hinj (by simp [powMonoidHom_apply, ← hg, pow_orderOf_eq_one])
  rw [h1, orderOf_one] at hg
  exact hℓ.ne_one hg.symm

theorem exists_nonpower_le_genBound {p ℓ : ℕ} [Fact p.Prime] (hℓ : ℓ.Prime) (hdvd : ℓ ∣ p - 1) :
    ∃ u : (ZMod p)ˣ, (u : ZMod p).val ≤ genBound p ∧ u ∉ powers p ℓ :=
  exists_not_mem_le_genBound _ (powers_ne_top hℓ hdvd)

/-- **GRH corollary** (via Bach): for `ℓ ∣ p - 1`, some `ℓ`-th power non-residue is
`≤ 2 ln² p`, so deterministic `ℓ`-th roots mod `p` (Adleman–Manders–Miller) need no randomness. -/
theorem exists_small_nonpower (hB : BachBound) {p ℓ : ℕ} [hp : Fact p.Prime] (hℓ : ℓ.Prime)
    (hdvd : ℓ ∣ p - 1) :
    ∃ u : (ZMod p)ˣ, ((u : ZMod p).val : ℝ) ≤ 2 * Real.log p ^ 2 ∧ u ∉ powers p ℓ := by
  obtain ⟨u, hle, hu⟩ := exists_nonpower_le_genBound hℓ hdvd
  exact ⟨u, (by exact_mod_cast hle : ((u : ZMod p).val : ℝ) ≤ genBound p).trans
    (hB p hp.out.one_lt), hu⟩

end Leanmath
