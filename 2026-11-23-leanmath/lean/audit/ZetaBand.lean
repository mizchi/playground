import ChallengeDeps
import Leanmath.ZetaBand

/-! Interface check against the unmodified upstream ChallengeDeps module.
See the research log for the pinned input and commands. -/

open Filter Leanmath.ZetaWidth

example (T : ℝ) : dyadicCount T = Ncount T (2*T) := rfl
example (T : ℝ) : (simpleCriticalBand T).ncard = N0simple T (2*T) := rfl

example
    (theoremB : ∀ ε > 0, ∃ T₀ : ℝ, ∀ T ≥ T₀,
      (2/3-ε)*(Ncount T (2*T) : ℝ) ≤ N0simple T (2*T))
    (growth : Tendsto (fun T => (Ncount T (2*T) : ℝ)) atTop atTop) (m : ℕ) :
    ∀ᶠ T in atTop, m ≤ N0simple T (2*T) ∧
      ∀ ρ₁ ∈ simpleCriticalBand T, ∀ ρ₂ ∈ simpleCriticalBand T, ∀ ρ₃ ∈ simpleCriticalBand T,
        ρ₁.im < ρ₂.im → ρ₂.im < ρ₃.im → TripleWidthBounds ρ₁.im ρ₂.im ρ₃.im :=
  eventually_many_band_triples theoremB growth m

-- At b=1 and h=2 a shortest filter also cancels 4, but cannot cancel 3.
example {μ : MeasureTheory.Measure ℝ} [MeasureTheory.IsProbabilityMeasure μ] {c : ℝ}
    (m₁ : Cancels μ 1) (m₂ : Cancels μ 2)
    (hs : ∀ᵐ t ∂μ, |t-c| ≤ pairRadius 1 2) : Cancels μ 4 ∧ ¬ Cancels μ 3 := by
  have spec := optimal_filter_spectrum (by norm_num : (0:ℝ) < 1)
    (by norm_num : (1:ℝ) < 2) m₁ m₂ hs
  constructor
  · exact (spec 4).mpr ⟨1, Or.inl (by norm_num)⟩
  · intro h
    obtain ⟨n, hn | hn⟩ := (spec 3).mp h
    all_goals
      have eq := hn
      norm_cast at eq
      omega

#print axioms Leanmath.ZetaWidth.pair_minimum
#print axioms Leanmath.ZetaWidth.optimal_filter_spectrum
#print axioms Leanmath.ZetaWidth.three_ordinates_in_band
#print axioms Leanmath.ZetaWidth.eventually_many_band_triples
