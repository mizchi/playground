module

public import Mathlib.Data.Set.Card
public import Mathlib.MeasureTheory.Integral.Bochner.Basic
public import Mathlib.Analysis.SpecialFunctions.Trigonometric.Basic
public import Mathlib.NumberTheory.LSeries.RiemannZeta

/-! Positive averages of the oscillatory factors in the squarefree-count explicit formula.
The certificate hypotheses below are real inequalities, not claims about a numerical zero table. -/

public section

open MeasureTheory
open scoped BigOperators NNReal ENNReal

namespace Leanmath.ZetaWidth

/-- Vanishing for every starting phase, in log-scale coordinates. -/
@[expose] def Cancels (μ : Measure ℝ) (b : ℝ) : Prop :=
  ∀ phase : ℝ, ∫ t, Real.cos (b * t + phase) ∂μ = 0

theorem cos_integrable (μ : Measure ℝ) [IsFiniteMeasure μ] (b c : ℝ) :
    Integrable (fun t : ℝ => Real.cos (b * (t - c))) μ := by
  apply (integrable_const (μ := μ) (1 : ℝ)).mono'
  · exact (Real.continuous_cos.comp (continuous_const.mul (continuous_id.sub continuous_const))).aestronglyMeasurable
  · exact Filter.Eventually.of_forall fun t => by simp [Real.norm_eq_abs, abs_le, Real.neg_one_le_cos, Real.cos_le_one]

theorem centered_moment {μ : Measure ℝ} {b : ℝ} (h : Cancels μ b) (c : ℝ) :
    ∫ t, Real.cos (b * (t - c)) ∂μ = 0 := by
  calc
    _ = ∫ t, Real.cos (b*t + -(b*c)) ∂μ := integral_congr_ae <|
      Filter.Eventually.of_forall fun t => by rw [mul_sub, sub_eq_add_neg]
    _ = 0 := h _

/-- Any positive probability law confined to too short an interval cannot erase even
one frequency. The center is arbitrary and the law need not be atomic or symmetric. -/
theorem one_frequency_lower {μ : Measure ℝ} [IsProbabilityMeasure μ] {b c r : ℝ}
    (hb : 0 < b) (h : Cancels μ b) (hs : ∀ᵐ t ∂μ, |t-c| ≤ r) :
    Real.pi ≤ b * (2*r) := by
  by_contra! hn
  have hpos : ∀ᵐ t ∂μ, 0 < Real.cos (b*(t-c)) := by
    filter_upwards [hs] with t ht
    have ht' := abs_le.mp ht
    apply Real.cos_pos_of_mem_Ioo
    constructor <;> nlinarith [mul_le_mul_of_nonneg_left ht'.1 hb.le,
      mul_le_mul_of_nonneg_left ht'.2 hb.le]
  have hz := (integral_eq_zero_iff_of_nonneg_ae (hpos.mono fun _ ht => ht.le)
    (cos_integrable μ b c)).mp (centered_moment h c)
  have hf : ∀ᵐ t ∂μ, False := by
    filter_upwards [hpos, hz] with t hp he
    exact hp.ne' he
  exact (NeZero.ne μ) (by simpa using hf)

/-- Equal weights half a period apart attain that lower bound, for every phase. -/
theorem half_period_average {b : ℝ} (hb : 0 < b) (phase : ℝ) :
    (Real.cos phase + Real.cos (b * (Real.pi / b) + phase)) / 2 = 0 := by
  have he : b * (Real.pi / b) = Real.pi := by field_simp
  rw [he, add_comm Real.pi phase, Real.cos_add_pi]
  ring

/-- RH locates the real part of the pole parameter ρ/2. Its frequency is Im(ρ)/2.
An equal-weight pair at distance 2π/Im(ρ) annihilates that oscillatory factor. -/
theorem rh_zero_half_and_cancel (hRH : RiemannHypothesis) {ρ : ℂ}
    (hz : riemannZeta ρ = 0) (hγ : 0 < ρ.im) :
    (ρ / 2).re = 1/4 ∧ ∀ phase : ℝ,
      (Real.cos phase + Real.cos ((ρ.im/2) * (2*Real.pi/ρ.im) + phase)) / 2 = 0 := by
  have hreal : ρ.re = 1/2 := by
    apply hRH ρ hz
    · rintro ⟨n, hn⟩
      simp [hn] at hγ
    · rintro rfl
      simp at hγ
  constructor
  · simpa [Complex.div_ofNat_re, hreal] using (by norm_num : (1/2 : ℝ)/2 = 1/4)
  · intro phase
    convert half_period_average (b := ρ.im/2) (by positivity) phase using 2; ring_nf

/-- In a band (T, 2T], each individual ordinate has its own two-observation
clock with separation less than 2π/T. A common clock for the whole band is a
different requirement. -/
def HasBandClock (T γ : ℝ) : Prop :=
  ∃ d : ℝ, 0 < d ∧ d < 2*Real.pi/T ∧
    ∀ phase : ℝ, (Real.cos phase + Real.cos ((γ/2)*d + phase)) / 2 = 0

theorem band_clock_of_height {T γ : ℝ} (hT : 0 < T) (hγ : T < γ) :
    HasBandClock T γ := by
  have hg := hT.trans hγ
  refine ⟨Real.pi / (γ/2), by positivity, ?_, half_period_average (by positivity)⟩
  rw [div_lt_div_iff₀ (by positivity) hT]
  nlinarith [mul_lt_mul_of_pos_left hγ Real.pi_pos]

/-- Transport the dyadic zero-density theorem of zeta23 to individual positive
averages. For Theorem B, S T is its set of simple critical-line zeros and N T
counts all zeros with multiplicities. The density theorem is an explicit input
so this project does not import an incompatible version of mathlib. -/
theorem band_clock_count_of_density (S : ℝ → Set ℂ) (N : ℝ → ℕ) (δ : ℝ)
    (hheight : ∀ T, ∀ ρ ∈ S T, T < ρ.im)
    (density : ∀ ε > (0 : ℝ), ∃ T₀ : ℝ, ∀ T ≥ T₀,
      (δ - ε) * (N T : ℝ) ≤ ((S T).ncard : ℝ)) :
    ∀ ε > (0 : ℝ), ∃ T₀ : ℝ, ∀ T ≥ T₀, (δ - ε) * (N T : ℝ) ≤
      (({ρ ∈ S T | HasBandClock T ρ.im} : Set ℂ).ncard : ℝ) := by
  intro ε hε
  obtain ⟨T₀, h⟩ := density ε hε
  refine ⟨max T₀ 1, ?_⟩
  intro T ht
  have hT : 0 < T := lt_of_lt_of_le zero_lt_one ((le_max_right _ _).trans ht)
  have heq : {ρ ∈ S T | HasBandClock T ρ.im} = S T :=
    Set.ext fun ρ => ⟨And.left, fun hρ => ⟨hρ, band_clock_of_height hT (hheight T ρ hρ)⟩⟩
  rw [heq]
  exact h T ((le_max_left _ _).trans ht)

/-- A linear combination of the three cosine moments is nonnegative inside a proposed
optimal interval and strictly positive in its interior except at ±x. If the first
cosine is positive there, no narrower interval can support a cancelling probability law. -/
theorem three_frequency_lower_of_certificate {μ : Measure ℝ} [IsProbabilityMeasure μ]
    {b a : Fin 3 → ℝ} {x y c r : ℝ}
    (mom : ∀ i, Cancels μ (b i)) (hs : ∀ᵐ t ∂μ, |t-c| ≤ r)
    (hnonneg : ∀ u : ℝ, |u| < y → 0 ≤ ∑ i, a i * Real.cos (b i * u))
    (hzeros : ∀ u : ℝ, |u| < y → (∑ i, a i * Real.cos (b i * u)) = 0 → u = x ∨ u = -x)
    (hfirst : 0 < Real.cos (b 0 * x)) : y ≤ r := by
  by_contra! hr
  let score := fun t : ℝ => ∑ i, a i * Real.cos (b i * (t-c))
  have hi (i : Fin 3) : Integrable (fun t : ℝ => a i * Real.cos (b i * (t-c))) μ :=
    (cos_integrable μ (b i) c).const_mul _
  have hzero : ∫ t, score t ∂μ = 0 := by
    dsimp [score]
    rw [integral_finsetSum _ (fun i _ => hi i)]
    simp [integral_const_mul, centered_moment (mom _) c]
  have hae : score =ᵐ[μ] 0 := (integral_eq_zero_iff_of_nonneg_ae
    (hs.mono fun t ht => hnonneg (t-c) (ht.trans_lt hr))
    (integrable_finsetSum _ (fun i _ => hi i))).mp hzero
  have hconst : (fun t : ℝ => Real.cos (b 0*(t-c))) =ᵐ[μ] fun _ => Real.cos (b 0*x) := by
    filter_upwards [hs, hae] with t ht he
    rcases hzeros (t-c) (ht.trans_lt hr) he with htx | htx <;> simp [htx]
  have hint := (integral_congr_ae hconst).symm.trans (centered_moment (mom 0) c)
  simp at hint
  exact hfirst.ne' hint

/-- The corresponding four observations cancel every phase as soon as the three
cosine moment equations hold. Positivity of the recipe follows when 0 < w < 1. -/
theorem four_point_average {b : Fin 3 → ℝ} {x y w : ℝ}
    (hm : ∀ i, w * Real.cos (b i*x) + (1-w) * Real.cos (b i*y) = 0)
    (i : Fin 3) (phase : ℝ) :
    w/2 * (Real.cos (phase + b i*x) + Real.cos (phase - b i*x)) +
    (1-w)/2 * (Real.cos (phase + b i*y) + Real.cos (phase - b i*y)) = 0 := by
  rw [Real.cos_add, Real.cos_sub, Real.cos_add, Real.cos_sub]
  linear_combination Real.cos phase * hm i

/-- Radii of intervals (with arbitrary center) carrying a cancelling probability law. -/
@[expose] def feasibleRadii {ι : Type*} (b : ι → ℝ) : Set ℝ :=
  {r | ∃ (μ : Measure ℝ) (_ : IsProbabilityMeasure μ) (c : ℝ),
    (∀ᵐ t ∂μ, |t-c| ≤ r) ∧ ∀ i, Cancels μ (b i)}

/-- The law placing masses p at ±x and q at ±y, with its response at every phase. -/
theorem symmetric_four_point_law {x y : ℝ} (hx : |x| ≤ y)
    (p q : ℝ≥0) (hnorm : p*2 + q*2 = 1) :
    ∃ (μ : Measure ℝ) (_ : IsProbabilityMeasure μ), (∀ᵐ t ∂μ, |t-0| ≤ y) ∧
      ∀ b phase : ℝ, (∫ t, Real.cos (b*t+phase) ∂μ) =
        (2*((p:ℝ)*Real.cos (b*x)+(q:ℝ)*Real.cos (b*y)))*Real.cos phase := by
  let pair (v : ℝ) := Measure.dirac v + Measure.dirac (-v)
  let μ := p • pair x + q • pair y
  have hy := (abs_nonneg x).trans hx
  have hprob : IsProbabilityMeasure μ := by
    constructor
    simpa [μ, pair, add_apply, smul_apply, NNReal.smul_def, two_mul,
      mul_two, smul_eq_mul, mul_add, ← ENNReal.coe_add] using
      congrArg (fun v : ℝ≥0 => (v : ℝ≥0∞)) hnorm
  refine ⟨μ, hprob, ?_, ?_⟩
  · have hpair (v : ℝ) (hv : |v| ≤ y) : ∀ᵐ t ∂pair v, |t-0| ≤ y := by
      simpa [pair, ae_add_measure_iff, ae_dirac_eq] using hv
    exact ae_add_measure_iff.mpr ⟨Measure.ae_smul_measure (hpair x hx) p,
      Measure.ae_smul_measure (hpair y (by rw [abs_of_nonneg hy])) q⟩
  · intro b phase
    let f := fun t : ℝ => Real.cos (b*t + phase)
    have hi (ν : Measure ℝ) [IsFiniteMeasure ν] : Integrable f ν := by
      apply (integrable_const (μ := ν) (1 : ℝ)).mono'
      · exact (Real.continuous_cos.comp ((continuous_const.mul continuous_id).add continuous_const)).aestronglyMeasurable
      · exact Filter.Eventually.of_forall fun t => by
          simp [f, Real.norm_eq_abs, abs_le, Real.neg_one_le_cos, Real.cos_le_one]
    change ∫ t, f t ∂(p • pair x + q • pair y) = _
    rw [integral_add_measure (hi _) (hi _)]
    have hpint (v : ℝ) : ∫ t, f t ∂pair v = f v + f (-v) := by
      change ∫ t, f t ∂(Measure.dirac v + Measure.dirac (-v)) = _
      rw [integral_add_measure (hi _) (hi _), integral_dirac, integral_dirac]
    rw [integral_smul_nnreal_measure, integral_smul_nnreal_measure, hpint, hpint]
    simp only [NNReal.smul_def, smul_eq_mul, f, mul_neg, Real.cos_add,
      Real.cos_neg, Real.sin_neg]
    ring

/-- Combine the primal and dual certificates into an attained global minimum.
Each inner atom has mass p, each outer atom has mass q. -/
theorem three_frequency_minimum {b a : Fin 3 → ℝ} {x y : ℝ} (hx : |x| ≤ y)
    (p q : ℝ≥0) (hnorm : p*2 + q*2 = 1)
    (hm : ∀ i, (p:ℝ)*Real.cos (b i*x) + (q:ℝ)*Real.cos (b i*y) = 0)
    (hnonneg : ∀ u : ℝ, |u| < y → 0 ≤ ∑ i, a i * Real.cos (b i*u))
    (hzeros : ∀ u : ℝ, |u| < y → (∑ i, a i * Real.cos (b i*u)) = 0 → u = x ∨ u = -x)
    (hfirst : 0 < Real.cos (b 0*x)) : IsLeast (feasibleRadii b) y := by
  obtain ⟨μ, hμ, hs, response⟩ := symmetric_four_point_law hx p q hnorm
  constructor
  · refine ⟨μ, hμ, 0, hs, fun i phase => ?_⟩
    rw [response, hm i]; ring
  · rintro r ⟨μ, hμ, c, hs, mom⟩
    exact three_frequency_lower_of_certificate mom hs hnonneg hzeros hfirst

/-- The outward-rounded decimal width from the external certificate implies the
reported five-decimal scale ratio. Both exponential comparisons are kernel-checked. -/
theorem scale_ratio_of_log_width {L : ℝ} (lo : 0.93989042 ≤ L) (hi : L ≤ 0.93989043) :
    2.559695 < Real.exp L ∧ Real.exp L < 2.559705 := by
  constructor
  · apply lt_of_lt_of_le _ (Real.exp_le_exp.mpr lo)
    have h := (abs_le.mp (Real.exp_bound (x := 0.93989042) (n := 10) (by norm_num) (by decide))).1
    norm_num [Finset.sum_range_succ, Nat.factorial] at h
    linarith
  · apply lt_of_le_of_lt (Real.exp_le_exp.mpr hi)
    have h := (abs_le.mp (Real.exp_bound (x := 0.93989043) (n := 10) (by norm_num) (by decide))).2
    norm_num [Finset.sum_range_succ, Nat.factorial] at h
    linarith

end Leanmath.ZetaWidth
