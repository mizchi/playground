module

public import Leanmath.ZetaWidth
public import Mathlib.Data.Fin.VecNotation
public import Mathlib.Analysis.SpecialFunctions.Trigonometric.Complex

/-! Sharp common positive averages for a pair of nearby frequencies and the
criterion for a third frequency to fit in the same interval. All frequencies
are angular frequencies on the logarithmic observation axis. -/

public section

namespace Leanmath.ZetaWidth

open MeasureTheory
open scoped NNReal

noncomputable def pairRadius (b h : ℝ) : ℝ := 2 * Real.pi / (b+h)

private theorem radius_pos {b h : ℝ} (hb : 0 < b) (bh : b < h) :
    0 < pairRadius b h := div_pos (by positivity) (by linarith)

private theorem radius_eq {b h : ℝ} (hb : 0 < b) (bh : b < h) :
    (b+h)*pairRadius b h = 2*Real.pi := by
  unfold pairRadius
  field_simp [show b+h ≠ 0 by linarith]

private theorem end_cos {b h : ℝ} (hb : 0 < b) (bh : b < h) :
    Real.cos (h*pairRadius b h) = Real.cos (b*pairRadius b h) := by
  have e := radius_eq hb bh
  rw [show h*pairRadius b h = 2*Real.pi - b*pairRadius b h by linarith, Real.cos_two_pi_sub]

private theorem end_cos_neg {b h : ℝ} (hb : 0 < b) (bh : b < h) (hh : h < 3*b) :
    Real.cos (b*pairRadius b h) < 0 := by
  have e := radius_eq hb bh
  have hp := radius_pos hb bh
  apply Real.cos_neg_of_pi_div_two_lt_of_lt <;>
    nlinarith [mul_lt_mul_of_pos_right hh hp, mul_lt_mul_of_pos_right bh hp, Real.pi_pos]

private theorem pair_gap {b h t : ℝ} (hb : 0 < b) (bh : b < h)
    (ht : 0 < |t|) (ha : |t| < pairRadius b h) :
    0 < Real.cos (b*t) - Real.cos (h*t) := by
  have hh : 0 < h := hb.trans bh
  have prod := mul_lt_mul_of_pos_left ha (show 0 < b+h by linarith)
  rw [radius_eq hb bh] at prod
  have lt' := mul_lt_mul_of_pos_right bh ht
  have bp := mul_pos hb ht
  have eqcos (v : ℝ) (hv : 0 < v) : Real.cos (v*t) = Real.cos (v*|t|) := by
    rw [← abs_of_pos hv, ← abs_mul, Real.cos_abs, abs_of_pos hv]
  rw [eqcos b hb, eqcos h hh, Real.cos_sub_cos]
  have hs := Real.sin_pos_of_pos_of_lt_pi (x := (b*|t|+h*|t|)/2)
    (by linarith) (by linarith)
  have hn := Real.sin_neg_of_neg_of_neg_pi_lt (x := (b*|t|-h*|t|)/2)
    (by linarith) (by linarith)
  exact mul_pos_of_neg_of_neg (by linarith) hn

/-- The optimal radius for the first pair is also optimal for the triple
whenever its third cosine has the same endpoint value. -/
theorem three_frequency_minimum_of_end_cos {b h k : ℝ}
    (hb : 0 < b) (bh : b < h) (hh : h < 3*b)
    (hk : Real.cos (k*pairRadius b h) = Real.cos (b*pairRadius b h)) :
    IsLeast (feasibleRadii ![b,h,k]) (pairRadius b h) := by
  let a := pairRadius b h
  let d := Real.cos (b*a)
  have hd : d < 0 := end_cos_neg hb bh hh
  have hden : 0 < 1-d := by linarith
  have hneg : 0 < -d := neg_pos.mpr hd
  let q : ℝ≥0 := ⟨1/(2*(1-d)), by positivity⟩
  let p : ℝ≥0 := ⟨-d/(2*(1-d)), by positivity⟩
  have hnorm : p*2+q*2 = 1 := by
    apply NNReal.eq
    change (-d/(2*(1-d)))*2+(1/(2*(1-d)))*2 = 1
    field_simp [show 1-d ≠ 0 by linarith]; ring
  have hm : (p:ℝ) + q*d = 0 := by
    change (-d)/(2*(1-d)) + (1/(2*(1-d)))*d = 0
    ring
  have score (u : ℝ) : (∑ i : Fin 3, (![1,-1,0] : Fin 3 → ℝ) i *
      Real.cos ((![b,h,k] : Fin 3 → ℝ) i * u)) = Real.cos (b*u) - Real.cos (h*u) := by
    simp [Fin.sum_univ_succ, sub_eq_add_neg]
  apply three_frequency_minimum (a := ![1,-1,0]) (x := 0) (y := a)
    (by simpa [a] using (radius_pos hb bh).le) p q hnorm
  · intro i
    fin_cases i <;> simpa [a, d, end_cos hb bh, hk] using hm
  · intro u hu
    rw [score]
    by_cases h : u = 0
    · simp [h]
    · exact (pair_gap hb bh (abs_pos.mpr h) hu).le
  · intro u hu hzero
    rw [score] at hzero
    by_contra! h
    exact (pair_gap hb bh (abs_pos.mpr h.1) hu).ne' hzero
  · simp

/-- Any cancelling distribution that fits inside the pair's optimal interval
is concentrated on the center and the two endpoints, up to a null set. -/
theorem pair_support {b h c : ℝ} {μ : Measure ℝ} [IsProbabilityMeasure μ]
    (hb : 0 < b) (bh : b < h) (mb : Cancels μ b) (mh : Cancels μ h)
    (support : ∀ᵐ t ∂μ, |t-c| ≤ pairRadius b h) :
    ∀ᵐ t ∂μ, t-c = 0 ∨ |t-c| = pairRadius b h := by
  let f := fun t : ℝ => Real.cos (b*(t-c)) - Real.cos (h*(t-c))
  have hedge (u : ℝ) (he : |u| = pairRadius b h) : Real.cos (h*u) = Real.cos (b*u) := by
    have ec (v : ℝ) (hv : 0 < v) : Real.cos (v*u) = Real.cos (v*pairRadius b h) := by
      rw [← he, ← abs_of_pos hv, ← abs_mul, Real.cos_abs, abs_of_pos hv]
    rw [ec h (hb.trans bh), ec b hb, end_cos hb bh]
  have nn : 0 ≤ᵐ[μ] f := by
    filter_upwards [support] with t ht
    rcases ht.lt_or_eq with hi | he
    · by_cases h0 : t-c = 0
      · simp [f, h0]
      · exact (pair_gap hb bh (abs_pos.mpr h0) hi).le
    · simp [f, hedge _ he]
  have integ := (cos_integrable μ b c).sub (cos_integrable μ h c)
  have hz : ∫ t, f t ∂μ = 0 := by
    simp only [f, integral_sub (cos_integrable μ b c) (cos_integrable μ h c),
      centered_moment mb c, centered_moment mh c, sub_zero]
  have hae := (integral_eq_zero_iff_of_nonneg_ae nn integ).mp hz
  filter_upwards [hae, support] with t he ht
  by_contra! hn
  exact (pair_gap hb bh (abs_pos.mpr hn.1) (lt_of_le_of_ne ht hn.2)).ne' he

/-- For the pair alone the least *full width* is 4π/(b+h). -/
theorem pair_minimum {b h : ℝ} (hb : 0 < b) (bh : b < h) (hh : h < 3*b) :
    IsLeast (feasibleRadii ![b,h]) (pairRadius b h) := by
  have eqs : feasibleRadii ![b,h] = feasibleRadii ![b,h,b] := by
    ext r
    constructor
    · rintro ⟨μ, hμ, c, hs, hm⟩
      refine ⟨μ, hμ, c, hs, ?_⟩
      intro i
      fin_cases i <;> first | exact hm 0 | exact hm 1
    · rintro ⟨μ, hμ, c, hs, hm⟩
      refine ⟨μ, hμ, c, hs, ?_⟩
      intro i
      fin_cases i <;> first | exact hm 0 | exact hm 1
  rw [eqs]
  exact three_frequency_minimum_of_end_cos hb bh hh rfl

/-- On a compact set a nonnegative witness F can be made strictly positive by
a linear combination with G, provided G is positive at all the zeros of F. -/
private theorem amplify_on_compact {K : Set ℝ} (hc : IsCompact K) {F G : ℝ → ℝ}
    (hf : Continuous F) (hg : Continuous G) (hn : ∀ t ∈ K, 0 ≤ F t)
    (hz : ∀ t ∈ K, F t = 0 → 0 < G t) :
    ∃ M : ℝ, ∀ t ∈ K, 0 < M * F t + G t := by
  let bad := K ∩ {t | G t ≤ 0}
  by_cases hbad : bad.Nonempty
  · have compactBad : IsCompact bad := hc.inter_right (isClosed_le hg continuous_const)
    obtain ⟨v, hv, hmin⟩ := compactBad.exists_isMinOn hbad hf.continuousOn
    have hp : 0 < F v := lt_of_le_of_ne (hn v hv.1) (fun eq => (hz v hv.1 eq.symm).not_ge hv.2)
    obtain ⟨lo, hlo⟩ := hc.bddBelow_image hg.continuousOn
    let M := (1+|lo|) / F v
    have hm : 0 < M := by dsimp [M]; positivity
    refine ⟨M, fun t ht => ?_⟩
    by_cases hgt : 0 < G t
    · exact add_pos_of_nonneg_of_pos (mul_nonneg hm.le (hn _ ht)) hgt
    · have htbad : t ∈ bad := ⟨ht, le_of_not_gt hgt⟩
      have h1 := mul_le_mul_of_nonneg_left (hmin htbad) hm.le
      have h2 : lo ≤ G t := hlo ⟨t, ht, rfl⟩
      have h3 : M * F v = 1+|lo| := div_mul_cancel₀ _ hp.ne'
      linarith [neg_abs_le lo]
  · refine ⟨1, fun t ht => ?_⟩
    have hgt : 0 < G t := lt_of_not_ge fun h => hbad ⟨t, ht, h⟩
    linarith [hn t ht]

/-- A positive cosine combination throughout the interval rules out every
cancelling probability measure supported there. -/
private theorem excludes_radius {b a : Fin 3 → ℝ} {r : ℝ}
    (hp : ∀ t : ℝ, |t| ≤ r → 0 < ∑ i, a i * Real.cos (b i*t)) :
    ∀ s ∈ feasibleRadii b, r < s := by
  rintro s ⟨μ, hμ, c, hs, mom⟩
  by_contra! hh
  let f := fun t : ℝ => ∑ i, a i * Real.cos (b i*(t-c))
  have hi (i : Fin 3) : Integrable (fun t : ℝ => a i * Real.cos (b i * (t-c))) μ :=
    (cos_integrable μ (b i) c).const_mul _
  have hz : ∫ t, f t ∂μ = 0 := by
    dsimp [f]
    rw [integral_finsetSum _ (fun i _ => hi i)]
    simp [integral_const_mul, centered_moment (mom _) c]
  have hpos : ∀ᵐ t ∂μ, 0 < f t := hs.mono fun t ht => hp _ (ht.trans hh)
  have hzero := (integral_eq_zero_iff_of_nonneg_ae (hpos.mono fun _ ht => ht.le)
    (integrable_finsetSum _ (fun i _ => hi i))).mp hz
  have hn : ∀ᵐ t ∂μ, False := by
    filter_upwards [hpos, hzero] with t ht he
    exact ht.ne' he
  exact (NeZero.ne μ) (by simpa using hn)

/-- Off resonance the third frequency requires a strictly positive extra
radius, uniform over all distributions and all centers. -/
theorem three_frequency_positive_gap {b h k : ℝ} (hb : 0 < b) (bh : b < h)
    (hk : Real.cos (k*pairRadius b h) ≠ Real.cos (b*pairRadius b h)) :
    ∃ δ > (0 : ℝ), ∀ r ∈ feasibleRadii ![b,h,k], pairRadius b h + δ < r := by
  let a := pairRadius b h
  have ha : 0 < a := radius_pos hb bh
  let D := Real.cos (k*a) - Real.cos (b*a)
  have hd : D ≠ 0 := sub_ne_zero.mpr hk
  let F := fun t : ℝ => Real.cos (b*t) - Real.cos (h*t)
  let G := fun t : ℝ => D*(Real.cos (k*t)-Real.cos (b*t)) + D^2/2*Real.cos (b*t)
  have f0 : F 0 = 0 := by simp [F]
  have fa : F a = 0 := by simp [F, a, end_cos hb bh]
  have fi {t : ℝ} (ht : 0 < t) (ht' : t < a) : 0 < F t :=
    pair_gap hb bh (abs_pos.mpr ht.ne') (by rwa [abs_of_pos ht])
  have contF : Continuous F := by dsimp [F]; fun_prop
  have contG : Continuous G := by dsimp [G]; fun_prop
  have hf : ∀ t ∈ Set.Icc 0 a, 0 ≤ F t := by
    rintro t ⟨hl, hr⟩
    rcases hl.lt_or_eq with hl | rfl
    · rcases hr.lt_or_eq with hr | rfl
      · exact (fi hl hr).le
      · exact fa.ge
    · exact f0.ge
  have hg : ∀ t ∈ Set.Icc 0 a, F t = 0 → 0 < G t := by
    rintro t ⟨hl, hr⟩ hz
    rcases hl.lt_or_eq with hl | rfl
    · rcases hr.lt_or_eq with hr | rfl
      · exact False.elim ((fi hl hr).ne' hz)
      · change 0 < D*D + D^2/2*Real.cos (b*a)
        nlinarith [mul_nonneg (sq_nonneg D) (show 0 ≤ Real.cos (b*a)+1 by
          linarith [Real.neg_one_le_cos (b*a)]), sq_pos_of_ne_zero hd]
    · simpa [G] using (show 0 < D^2/2 by positivity)
  obtain ⟨M, hm⟩ := amplify_on_compact isCompact_Icc contF contG hf hg
  let J := fun t : ℝ => M * F t + G t
  have hpos : 0 < J a := hm a ⟨ha.le, le_rfl⟩
  have hcont : Continuous J := (continuous_const.mul contF).add contG
  obtain ⟨e, he, hball⟩ := Metric.mem_nhds_iff.mp ((isOpen_lt continuous_const hcont).mem_nhds hpos)
  refine ⟨e/2, by positivity, excludes_radius (a := ![M-D+D^2/2, -M, D]) ?_⟩
  intro t ht
  have hj : 0 < J |t| := by
    by_cases hu : |t| ≤ a
    · exact hm _ ⟨abs_nonneg _, hu⟩
    · apply hball
      rw [Metric.mem_ball, Real.dist_eq, abs_of_nonneg (by linarith : 0 ≤ |t|-a)]
      linarith
  have ce (v : ℝ) : Real.cos (v*|t|) = Real.cos (v*t) := by
    rcases le_total 0 t with ht | ht <;> simp [abs_of_nonneg, abs_of_nonpos, ht]
  change 0 < M * (Real.cos (b*|t|)-Real.cos (h*|t|)) +
    (D*(Real.cos (k*|t|)-Real.cos (b*|t|)) + D^2/2*Real.cos (b*|t|)) at hj
  simp only [ce] at hj
  simp [Fin.sum_univ_succ]
  linarith

/-- The frequencies erased by *any* optimal pair filter are precisely the two
arithmetic progressions with step b+h and offsets ±b. The same filter works
simultaneously at every frequency in their union. -/
theorem optimal_filter_spectrum {μ : Measure ℝ} [IsProbabilityMeasure μ] {b h c : ℝ}
    (hb : 0 < b) (bh : b < h) (mb : Cancels μ b) (mh : Cancels μ h)
    (support : ∀ᵐ t ∂μ, |t-c| ≤ pairRadius b h) (k : ℝ) :
    Cancels μ k ↔ ∃ n : ℤ, k = n*(b+h)+b ∨ k = n*(b+h)-b := by
  let a := pairRadius b h
  have ea : (b+h)*a = 2*Real.pi := radius_eq hb bh
  have ha : a ≠ 0 := (radius_pos hb bh).ne'
  constructor
  · intro mk
    have members : a ∈ feasibleRadii ![b,h,k] := by
      refine ⟨μ, inferInstance, c, support, ?_⟩
      intro i
      fin_cases i <;> assumption
    have eqcos : Real.cos (b*a) = Real.cos (k*a) := by
      by_contra hn
      obtain ⟨δ, hd, hδ⟩ := three_frequency_positive_gap hb bh (Ne.symm hn)
      have := hδ a members
      change a+δ < a at this
      linarith
    obtain ⟨n, he | he⟩ := Real.cos_eq_cos_iff.mp eqcos
    · refine ⟨n, Or.inl (mul_right_cancel₀ ha ?_)⟩
      rw [he, add_mul, mul_assoc (n:ℝ) (b+h) a, ea]; ring
    · refine ⟨n, Or.inr (mul_right_cancel₀ ha ?_)⟩
      rw [he, sub_mul, mul_assoc (n:ℝ) (b+h) a, ea]; ring
  · rintro ⟨n, hk⟩
    have sites := pair_support hb bh mb mh support
    have shift (v : ℝ) (mv : Cancels μ v) : Cancels μ (n*(b+h)+v) := by
      intro phase
      let phase' := phase + n*(b+h)*c
      calc
        _ = ∫ t, Real.cos (v*t+phase') ∂μ := by
          apply integral_congr_ae
          filter_upwards [sites] with t ht
          have e : (n*(b+h)+v)*t + phase =
              v*t + phase' + n*((b+h)*(t-c)) := by dsimp [phase']; ring
          rw [e]
          rcases ht with hz | he
          · simp [hz]
          · change |t-c| = a at he
            rcases abs_cases (t-c) with hs | hs
            · rw [hs.1] at he
              rw [he, ea, Real.cos_add_int_mul_two_pi]
            · rw [hs.1] at he
              rw [show t-c = -a by linarith, mul_neg, ea, mul_neg, ← sub_eq_add_neg,
                Real.cos_sub_int_mul_two_pi]
        _ = 0 := mv phase'
    rcases hk with rfl | rfl
    · exact shift b mb
    · apply shift (-b)
      intro phase
      convert mb (-phase) using 1
      congr 1
      funext t
      rw [show -b*t + phase = -(b*t + -phase) by ring, Real.cos_neg]

/-- Average the law and its translate by half a period. Translation preserves
every old cancellation; the second copy cancels the new frequency. -/
private theorem append_frequency {μ : Measure ℝ} [IsProbabilityMeasure μ] {c r k : ℝ}
    (hs : ∀ᵐ t ∂μ, |t-c| ≤ r) (hk : 0 < k) :
    ∃ (ν : Measure ℝ) (_ : IsProbabilityMeasure ν) (c' : ℝ),
      (∀ᵐ t ∂ν, |t-c'| ≤ r + Real.pi/k/2) ∧ Cancels ν k ∧
      ∀ b, Cancels μ b → Cancels ν b := by
  let d := Real.pi / k
  let τ := fun t : ℝ => t+d
  let σ := μ.map τ
  let ν := (1/2 : ℝ≥0) • (μ+σ)
  have hτ : Measurable τ := (continuous_id.add continuous_const).measurable
  have hν : IsProbabilityMeasure ν := by
    constructor
    simp [ν, σ, smul_apply, add_apply, ENNReal.inv_two_add_inv_two]
  have hd : 0 < d := div_pos Real.pi_pos hk
  refine ⟨ν, hν, c+d/2, ?_, ?_⟩
  · apply Measure.ae_smul_measure
    apply ae_add_measure_iff.mpr
    constructor
    · filter_upwards [hs] with t ht
      rw [abs_le] at ht ⊢
      constructor <;> linarith [ht.1, ht.2]
    · apply (ae_map_iff hτ.aemeasurable (by apply measurableSet_le <;> fun_prop)).mpr
      filter_upwards [hs] with t ht
      change |t+d-(c+d/2)| ≤ r+d/2
      rw [abs_le] at ht ⊢
      constructor <;> linarith [ht.1, ht.2]
  · have moment (b phase : ℝ) :
        ∫ t, Real.cos (b*t+phase) ∂ν =
        (1/2 : ℝ) * ((∫ t, Real.cos (b*t+phase) ∂μ) +
          ∫ t, Real.cos (b*t+(phase+b*d)) ∂μ) := by
      let f := fun t : ℝ => Real.cos (b*t+phase)
      have hf : Continuous f := by dsimp [f]; fun_prop
      have hi (η : Measure ℝ) [IsFiniteMeasure η] : Integrable f η := by
        apply (integrable_const (μ := η) (1 : ℝ)).mono' hf.aestronglyMeasurable
        exact Filter.Eventually.of_forall fun t => by
          simp [f, Real.norm_eq_abs, abs_le, Real.neg_one_le_cos, Real.cos_le_one]
      change ∫ t, f t ∂((1/2 : ℝ≥0) • (μ+σ)) = _
      rw [integral_smul_nnreal_measure, integral_add_measure (hi μ) (hi σ)]
      have hm : ∫ t, f t ∂σ = ∫ t, Real.cos (b*t+(phase+b*d)) ∂μ := by
        rw [show σ = μ.map τ from rfl, integral_map hτ.aemeasurable hf.aestronglyMeasurable]
        apply integral_congr_ae
        filter_upwards with t
        change Real.cos (b*(t+d)+phase) = _
        congr 1; ring
      rw [hm]
      simp [NNReal.smul_def, smul_eq_mul, f, mul_add]
    constructor
    · intro phase
      rw [moment, show k*d = Real.pi by dsimp [d]; field_simp]
      simp_rw [← add_assoc, Real.cos_add_pi, integral_neg]
      ring
    · intro b h phase
      rw [moment, h phase, h (phase+b*d)]
      ring

/-- An attained upper bound for an arbitrary positive third frequency. -/
theorem triple_feasible_upper {b h k : ℝ} (hb : 0 < b) (bh : b < h)
    (hh : h < 3*b) (hk : 0 < k) :
    pairRadius b h + Real.pi/k/2 ∈ feasibleRadii ![b,h,k] := by
  obtain ⟨μ, hμ, c, hs, hm⟩ := (pair_minimum hb bh hh).1
  obtain ⟨ν, hν, c', hs', mk, keep⟩ := append_frequency hs hk
  refine ⟨ν, hν, c', hs', ?_⟩
  intro i
  fin_cases i <;> first | exact keep b (hm 0) | exact keep h (hm 1) | exact mk

/-- Below twice the lowest frequency, every distinct third frequency is off
resonance: its reflected phase is strictly closer to zero than the first. -/
private theorem dyadic_off_resonance {b h k : ℝ} (hb : 0 < b) (bh : b < h)
    (hk : h < k) (hband : k ≤ 2*b) :
    Real.cos (k*pairRadius b h) ≠ Real.cos (b*pairRadius b h) := by
  let a := pairRadius b h
  have ha : 0 < a := radius_pos hb bh
  have he : (b+h)*a = 2*Real.pi := radius_eq hb bh
  have h1 := mul_lt_mul_of_pos_right bh ha
  have h2 := mul_lt_mul_of_pos_right hk ha
  have h3 := mul_le_mul_of_nonneg_right hband ha.le
  have hcos := Real.cos_lt_cos_of_nonneg_of_le_pi
    (show 0 ≤ 2*Real.pi-k*a by linarith)
    (show b*a ≤ Real.pi by linarith)
    (show 2*Real.pi-k*a < b*a by linarith)
  rw [Real.cos_two_pi_sub] at hcos
  exact ne_of_gt hcos

/-- Bounds on the common radius of the squarefree frequencies γᵢ/2. -/
@[expose] def TripleWidthBounds (γ₁ γ₂ γ₃ : ℝ) : Prop :=
    let b := γ₁/2
    let h := γ₂/2
    let k := γ₃/2
    let R := 4*Real.pi/(γ₁+γ₂)
    IsLeast (feasibleRadii ![b,h]) R ∧
    (∃ δ > (0 : ℝ), ∀ r ∈ feasibleRadii ![b,h,k], R+δ < r) ∧
    R + Real.pi/γ₃ ∈ feasibleRadii ![b,h,k]

/-- Specialization to the squarefree oscillations γ/2. In one dyadic window
every pair has an exact minimum; any third distinct ordinate incurs a uniform
positive gap above the first pair, and an explicit finite width suffices. -/
theorem three_ordinates_in_band {T γ₁ γ₂ γ₃ : ℝ}
    (hT : 0 < T) (h1 : T < γ₁) (h2 : γ₁ < γ₂) (h3 : γ₂ < γ₃) (h4 : γ₃ ≤ 2*T) :
    TripleWidthBounds γ₁ γ₂ γ₃ := by
  dsimp only [TripleWidthBounds]
  have hb : 0 < γ₁/2 := half_pos (hT.trans h1)
  have bh : γ₁/2 < γ₂/2 := by linarith
  have hh : γ₂/2 < 3*(γ₁/2) := by linarith
  have hR : 4*Real.pi/(γ₁+γ₂) = pairRadius (γ₁/2) (γ₂/2) := by
    unfold pairRadius
    rw [← add_div, div_div_eq_mul_div]; ring
  rw [hR]
  refine ⟨pair_minimum hb bh hh,
    three_frequency_positive_gap hb bh (dyadic_off_resonance hb bh (by linarith) (by linarith)), ?_⟩
  convert triple_feasible_upper hb bh hh (show 0 < γ₃/2 by linarith) using 1; ring_nf

end Leanmath.ZetaWidth
