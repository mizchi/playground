module

public import Leanmath.ZetaPair
public import Mathlib.Analysis.Analytic.Order

/-! Application of the positive-width bounds to the counting statements in
Anthropic's zeta23. The two upstream results are explicit theorem parameters
because the upstream and local mathlib revisions differ. -/

public section

namespace Leanmath.ZetaWidth

open Filter Set

/-- Nontrivial zeta zeros in the positive-ordinate window (T, 2T]. -/
@[expose] def dyadicZeros (T : ℝ) : Set ℂ :=
  {ρ | (riemannZeta ρ = 0 ∧ 0 < ρ.re ∧ ρ.re < 1) ∧ T < ρ.im ∧ ρ.im ≤ 2*T}

@[expose] noncomputable def zetaMultiplicity (ρ : ℂ) : ℕ :=
  (analyticOrderAt riemannZeta ρ).toNat

/-- The counting convention of zeta23's Ncount. -/
@[expose] noncomputable def dyadicCount (T : ℝ) : ℕ := ∑ᶠ ρ ∈ dyadicZeros T, zetaMultiplicity ρ

/-- The set counted by zeta23's N0simple. -/
@[expose] def simpleCriticalBand (T : ℝ) : Set ℂ :=
  dyadicZeros T ∩ {ρ | ρ.re = 1/2} ∩ {ρ | zetaMultiplicity ρ = 1}

/-- For every requested population, all sufficiently high bands contain at
least that many simple critical-line zeros. Every ordered triple of those
zeros obeys the strict width gap and the attained upper bound. -/
theorem eventually_many_band_triples
    (density : ∀ ε > (0 : ℝ), ∃ T₀ : ℝ, ∀ T ≥ T₀,
      (2/3-ε)*(dyadicCount T : ℝ) ≤ ((simpleCriticalBand T).ncard : ℝ))
    (growth : Tendsto (fun T => (dyadicCount T : ℝ)) atTop atTop) (m : ℕ) :
    ∀ᶠ T in atTop, m ≤ (simpleCriticalBand T).ncard ∧
      ∀ ρ₁ ∈ simpleCriticalBand T, ∀ ρ₂ ∈ simpleCriticalBand T, ∀ ρ₃ ∈ simpleCriticalBand T,
        ρ₁.im < ρ₂.im → ρ₂.im < ρ₃.im → TripleWidthBounds ρ₁.im ρ₂.im ρ₃.im := by
  obtain ⟨T₀, h₀⟩ := density (1/6) (by norm_num)
  filter_upwards [eventually_ge_atTop T₀, eventually_gt_atTop (0:ℝ),
    growth.eventually_ge_atTop (2*(m:ℝ))] with T ht hp hn
  constructor
  · have h := h₀ T ht
    exact_mod_cast (show (m:ℝ) ≤ ((simpleCriticalBand T).ncard:ℝ) by linarith)
  · intro ρ₁ h₁ ρ₂ _ ρ₃ h₃ h₁₂ h₂₃
    exact three_ordinates_in_band hp h₁.1.1.2.1 h₁₂ h₂₃ h₃.1.1.2.2

end Leanmath.ZetaWidth
