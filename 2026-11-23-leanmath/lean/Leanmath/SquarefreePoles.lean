module

public import Mathlib.Analysis.Meromorphic.Order
public import Mathlib.NumberTheory.LSeries.ZetaZeros

/-! Local pole orders of the squarefree Dirichlet series under the ordinary RH. -/

public section
namespace Leanmath

private theorem critical_re_of_positive_zero (hRH : RiemannHypothesis) {s : ℂ}
    (hz : riemannZeta s = 0) (hs : 0 < s.im) : s.re = 1/2 := by
  apply hRH s hz
  · rintro ⟨n, rfl⟩
    simp at hs
  · rintro rfl
    simp at hs

/-- A zero of positive height and multiplicity m produces a pole of exactly
order m at half that zero. No simplicity assumption or zero table is needed. -/
theorem squarefree_pole_under_rh (hRH : RiemannHypothesis) {ρ : ℂ}
    (hz : riemannZeta ρ = 0) (hγ : 0 < ρ.im) :
    ∃ m : ℕ, 0 < m ∧ analyticOrderAt riemannZeta ρ = m ∧
      meromorphicOrderAt (fun s : ℂ => riemannZeta s / riemannZeta (2*s)) (ρ/2) =
        ((-(m : ℤ)) : WithTop ℤ) := by
  have hhalf : 0 < (ρ/2).im := by simpa [Complex.div_ofNat_im] using half_pos hγ
  have ne_one {s : ℂ} (h : 0 < s.im) : s ≠ 1 := by rintro rfl; simp at h
  have ha := analyticOn_riemannZeta ρ (ne_one hγ)
  have hb := analyticOn_riemannZeta (ρ/2) (ne_one hhalf)
  have hnum : riemannZeta (ρ/2) ≠ 0 := by
    intro h
    have := critical_re_of_positive_zero hRH h hhalf
    rw [Complex.div_ofNat_re, critical_re_of_positive_zero hRH hz hγ] at this
    norm_num at this
  have hfinite : analyticOrderAt riemannZeta ρ ≠ ⊤ := by
    apply analyticOn_riemannZeta.analyticOrderAt_ne_top_of_isPreconnected
      (isConnected_compl_singleton_of_one_lt_rank (by simp) _).isPreconnected
      (x := (2 : ℂ)) (by simp) (ne_one hγ)
    rw [analyticOrderAt_eq_zero.mpr (.inr (riemannZeta_ne_zero_of_one_le_re (by norm_num)))]
    simp
  obtain ⟨m, hm⟩ := ENat.ne_top_iff_exists.mp hfinite
  refine ⟨m, ?_, ?_, ?_⟩
  · by_contra! h
    have := ha.analyticOrderAt_eq_zero.mp (show analyticOrderAt riemannZeta ρ = 0 by
      simpa [show m = 0 by omega] using hm.symm)
    exact this hz
  · exact hm.symm
  · let g := fun s : ℂ => 2*s
    have hg : AnalyticAt ℂ g (ρ/2) := by dsimp [g]; fun_prop
    have hd : deriv g (ρ/2) ≠ 0 := by simp [g]
    have he : g (ρ/2) = ρ := by dsimp [g]; ring
    have hc : AnalyticAt ℂ riemannZeta (g (ρ/2)) := he.symm ▸ ha
    change meromorphicOrderAt (riemannZeta / (riemannZeta ∘ g)) _ = _
    rw [meromorphicOrderAt_div hb.meromorphicAt (hc.comp hg).meromorphicAt,
      meromorphicOrderAt_comp_of_deriv_ne_zero hg hd, he, hb.meromorphicOrderAt_eq,
      hb.analyticOrderAt_eq_zero.mpr hnum, ha.meromorphicOrderAt_eq, ← hm]
    simp

open Filter in
open scoped Topology in
/-- An analytic smoothing multiplier removes the pole exactly when it vanishes
to at least the multiplicity of the original zeta zero. Removability is stated
on a punctured neighborhood so the value assigned by division at zero is irrelevant. -/
theorem squarefree_removable_iff_under_rh (hRH : RiemannHypothesis) {ρ : ℂ}
    (hz : riemannZeta ρ = 0) (hγ : 0 < ρ.im) {K : ℂ → ℂ} (hK : AnalyticAt ℂ K (ρ/2)) :
    (∃ F : ℂ → ℂ, AnalyticAt ℂ F (ρ/2) ∧
      (fun s => K s * (riemannZeta s / riemannZeta (2*s))) =ᶠ[𝓝[≠] (ρ/2)] F) ↔
      analyticOrderAt riemannZeta ρ ≤ analyticOrderAt K (ρ/2) := by
  obtain ⟨m, hm, hord, hpole⟩ := squarefree_pole_under_rh hRH hz hγ
  let Q := fun s : ℂ => riemannZeta s / riemannZeta (2*s)
  have hQ : MeromorphicAt Q (ρ/2) := by
    by_contra h
    change meromorphicOrderAt Q _ = _ at hpole
    rw [meromorphicOrderAt_of_not_meromorphicAt h] at hpole
    norm_cast at hpole
    omega
  change (∃ F, AnalyticAt ℂ F (ρ/2) ∧ K * Q =ᶠ[𝓝[≠] (ρ/2)] F) ↔ _
  rw [← (hK.meromorphicAt.mul hQ).meromorphicOrderAt_nonneg_iff,
    meromorphicOrderAt_mul hK.meromorphicAt hQ, hK.meromorphicOrderAt_eq,
    show meromorphicOrderAt Q (ρ/2) = _ from hpole, hord]
  cases analyticOrderAt K (ρ/2) with
  | top => simp
  | coe n =>
    change ((0 : ℤ) : WithTop ℤ) ≤ (((n : ℤ) + -(m : ℤ) : ℤ) : WithTop ℤ) ↔
      (m : ℕ∞) ≤ (n : ℕ∞)
    rw [WithTop.coe_le_coe, ENat.natCast_le_natCast]
    omega

end Leanmath
