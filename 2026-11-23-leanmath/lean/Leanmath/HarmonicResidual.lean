module

public import Leanmath.ZetaWidth
public import Mathlib.Analysis.SpecialFunctions.Trigonometric.Inverse

/-! Minimax residual amplitude of the first three harmonics on a bounded
observation interval. The competing laws can be arbitrary probability measures. -/

public section
namespace Leanmath.ZetaWidth

open MeasureTheory
open scoped NNReal

/-- The same amplitude cap for each of the three modes at every phase. -/
@[expose] def HarmonicBound (μ : Measure ℝ) (e : ℝ) : Prop :=
  ∀ k ∈ ({1,2,3} : Set ℝ), ∀ phase : ℝ, |∫ t, Real.cos (k*t + phase) ∂μ| ≤ e

private theorem centered_bound {μ : Measure ℝ} {e : ℝ} (h : HarmonicBound μ e)
    (c k : ℝ) (hk : k ∈ ({1,2,3} : Set ℝ)) :
    |∫ t, Real.cos (k*(t-c)) ∂μ| ≤ e := by
  simpa only [sub_eq_add_neg, mul_add, mul_neg] using h k hk (-(k*c))

/-- Uniform error budgets achievable on an interval of radius r, at each of
the angular frequencies 1, 2, 3 and for every starting phase. -/
@[expose] def harmonicErrors (r : ℝ) : Set ℝ :=
  {e | ∃ (μ : Measure ℝ) (_ : IsProbabilityMeasure μ) (c : ℝ),
    (∀ᵐ t ∂μ, |t-c| ≤ r) ∧ HarmonicBound μ e}

/-- The nonnegative polynomial giving the dual lower bound on z ≥ -x. -/
private theorem cubic_certificate (x v z : ℝ) :
    (3+4*v^2-8*x*v)*z - (4*v-2*x)*(2*z^2-1) + (4*z^3-3*z) -
      (4*v-2*x-4*x*v^2) = 4*(z+x)*(z-v)^2 := by ring

/-- The minimax curve on radii arccos(-x), with 0 ≤ x ≤ 1/√2. -/
theorem three_harmonic_minimax {x : ℝ} (hx : 0 ≤ x) (hx2 : 2*x^2 ≤ 1) :
    IsLeast (harmonicErrors (Real.arccos (-x))) ((1-2*x^2)/(3+2*x-4*x^2)) := by
  let r := Real.arccos (-x)
  let v := (1+x)/(1+2*x)
  let s := v-x
  let e := s/(1+2*s)
  have hx1 : x ≤ 1 := by nlinarith [sq_nonneg (x-1)]
  have dpos : 0 < 1+2*x := by linarith
  have hv : v*(1+2*x) = 1+x := div_mul_cancel₀ _ dpos.ne'
  have hvhalf : 1/2 ≤ v := by dsimp [v]; rw [le_div_iff₀ dpos]; linarith
  have hv1 : v ≤ 1 := by dsimp [v]; rw [div_le_one dpos]; linarith
  have hs : 0 ≤ s := by
    change 0 ≤ (1+x)/(1+2*x)-x
    rw [sub_nonneg, le_div_iff₀ dpos]; nlinarith
  have hrel : 2*v*x = 1-s := by dsimp [s]; linarith
  have he : e*(1+2*s) = s := div_mul_cancel₀ _ (by positivity)
  have he0 : 0 ≤ e := by dsimp [e]; positivity
  have hes : e ≤ s := by dsimp [e]; rw [div_le_iff₀ (by positivity)]; nlinarith
  have heq : (1-2*x^2)/(3+2*x-4*x^2) = e := by
    have hd : 0 < 3+2*x-4*x^2 := by linarith
    apply (div_eq_iff hd.ne').mpr
    have hs' : s*(1+2*x) = 1-2*x^2 := by dsimp [s]; linarith
    linear_combination (1+2*x)*(-he) + (2*e-1)*hs'
  rw [heq]
  let q := (e+x)/(v+x)
  have hvx : 0 < v+x := by linarith
  have hq : 0 ≤ q := div_nonneg (by linarith) hvx.le
  have hq1 : q ≤ 1 := by
    apply (div_le_one hvx).mpr
    dsimp [s] at hes
    linarith
  have m1 : q*v+(1-q)*(-x) = e := by
    have := div_mul_cancel₀ (e+x) hvx.ne'
    change q*(v+x) = e+x at this; linarith
  have raw2 : q*v^2+(1-q)*(-x)^2 = s*e+v*x := by rw [← m1]; dsimp [s]; ring
  have m2 : q*(2*v^2-1)+(1-q)*(2*(-x)^2-1) = -e := by
    linear_combination 2*raw2 + he + hrel
  have raw3 : q*v^3+(1-q)*(-x)^3 = s*(q*v^2+(1-q)*(-x)^2)+v*x*e := by
    rw [← m1]; dsimp [s]; ring
  have m3 : q*(4*v^3-3*v)+(1-q)*(4*(-x)^3-3*(-x)) = e := by
    linear_combination 4*raw3 + 2*s*m2 - 3*m1 + 2*e*hrel - 2*he
  have cr : Real.cos r = -x := Real.cos_arccos (by linarith) (by linarith)
  have cv : Real.cos (Real.arccos v) = v := Real.cos_arccos (by linarith) hv1
  constructor
  · let p₁ : ℝ≥0 := ⟨q/2, by positivity⟩
    let p₂ : ℝ≥0 := ⟨(1-q)/2, by linarith⟩
    have norm : p₁*2+p₂*2 = 1 := by apply NNReal.eq; change q/2*2+(1-q)/2*2 = 1; ring
    have hpos : |Real.arccos v| ≤ r := by
      rw [abs_of_nonneg (Real.arccos_nonneg _)]
      exact Real.arccos_le_arccos (by linarith)
    obtain ⟨μ, hμ, hsupp, resp⟩ := symmetric_four_point_law hpos p₁ p₂ norm
    refine ⟨μ, hμ, 0, hsupp, ?_⟩
    intro k hk phase
    rw [resp]
    change |(2*(q/2*Real.cos (k*Real.arccos v) + (1-q)/2*Real.cos (k*r)))*Real.cos phase| ≤ e
    have hcoef : |2*(q/2*Real.cos (k*Real.arccos v) + (1-q)/2*Real.cos (k*r))| = e := by
      rw [show ∀ a b : ℝ, 2*(q/2*a+(1-q)/2*b) = q*a+(1-q)*b by intros; ring]
      rcases hk with hk | hk | hk
      · subst k; simp only [one_mul, cv, cr, m1, abs_of_nonneg he0]
      · subst k; rw [Real.cos_two_mul, Real.cos_two_mul, cv, cr, m2, abs_neg, abs_of_nonneg he0]
      · change k = 3 at hk; subst k
        rw [Real.cos_three_mul, Real.cos_three_mul, cv, cr, m3, abs_of_nonneg he0]
    rw [abs_mul, hcoef]
    exact mul_le_of_le_one_right he0 (abs_le.mpr ⟨Real.neg_one_le_cos _, Real.cos_le_one _⟩)
  · rintro d ⟨μ, hμ, c, hsupp, herr⟩
    let A := 3+4*v^2-8*x*v
    let B := 4*v-2*x
    let D := 4*v-2*x-4*x*v^2
    have hA : 0 ≤ A := by dsimp [A]; nlinarith [mul_nonneg (show 0 ≤ v-1/2 by linarith) (show 0 ≤ v+1/2 by linarith)]
    have hB : 0 ≤ B := by dsimp [B]; linarith
    have hD : D = (A+B+1)*e := by
      dsimp [A,B,D]
      linear_combination -q*cubic_certificate x v v - (1-q)*cubic_certificate x v (-x) +
        (3+4*v^2-8*x*v)*m1 - (4*v-2*x)*m2 + m3
    let f := fun k t : ℝ => Real.cos (k*(t-c))
    have hi (k : ℝ) : Integrable (f k) μ := cos_integrable μ k c
    have hm (k : ℝ) (hk : k ∈ ({1,2,3} : Set ℝ)) : |∫ t, f k t ∂μ| ≤ d :=
      centered_bound herr c k hk
    have pointwise : ∀ᵐ t ∂μ, D ≤ A*f 1 t - B*f 2 t + f 3 t := by
      filter_upwards [hsupp] with t ht
      have hz := Real.cos_le_cos_of_nonneg_of_le_pi (abs_nonneg (t-c)) (Real.arccos_le_pi (-x)) ht
      change Real.cos r ≤ Real.cos |t-c| at hz
      rw [cr, Real.cos_abs] at hz
      dsimp [f,A,B,D]
      rw [one_mul, Real.cos_two_mul, Real.cos_three_mul]
      have := cubic_certificate x v (Real.cos (t-c))
      nlinarith [mul_nonneg (show 0 ≤ Real.cos (t-c)+x by linarith) (sq_nonneg (Real.cos (t-c)-v))]
    have hint := integral_mono_ae (integrable_const D)
      ((((hi 1).const_mul A).sub ((hi 2).const_mul B)).add (hi 3)) pointwise
    change (∫ _ : ℝ, D ∂μ) ≤ ∫ t, A*f 1 t - B*f 2 t + f 3 t ∂μ at hint
    rw [integral_const, probReal_univ, one_smul, integral_add
      (f := fun t => A*f 1 t - B*f 2 t) (g := f 3)
      (((hi 1).const_mul A).sub ((hi 2).const_mul B)) (hi 3),
      integral_sub (f := fun t => A*f 1 t) (g := fun t => B*f 2 t)
      ((hi 1).const_mul A) ((hi 2).const_mul B), integral_const_mul, integral_const_mul] at hint
    have h1 := (abs_le.mp (hm 1 (by simp))).2
    have h2 := (abs_le.mp (hm 2 (by simp))).1
    have h3 := (abs_le.mp (hm 3 (by simp))).2
    apply le_of_mul_le_mul_left (a := A+B+1) _ (by linarith)
    linarith [mul_le_mul_of_nonneg_left h1 hA, mul_le_mul_of_nonneg_left h2 hB]


/-- On short arcs all three cosine polynomials lie below their endpoint
chords. The worst residual is the largest of the three explicit obstructions. -/
theorem three_harmonic_short_arc {l : ℝ} (hl : 0 ≤ l) (hl1 : l ≤ 1) :
    IsLeast (harmonicErrors (Real.arccos l))
      (max l (max ((2*l+1)/(2*l+3)) ((4*l*(1+l))/(4*l*(1+l)+2)))) := by
  let r := Real.arccos l
  let L := fun k : ℝ => if k = 1 then 1 else if k = 2 then 2*l+2 else 4*l*(1+l)+1
  let e := max l (max ((2*l+1)/(2*l+3)) ((4*l*(1+l))/(4*l*(1+l)+2)))
  change IsLeast _ e
  have hle : l ≤ e := le_max_left _ _
  have he0 := hl.trans hle
  have h2 : (2*l+1)/(2*l+3) ≤ e := (le_max_left _ _).trans (le_max_right _ _)
  have h3 : (4*l*(1+l))/(4*l*(1+l)+2) ≤ e := (le_max_right _ _).trans (le_max_right _ _)
  have he1 : e ≤ 1 := by
    dsimp [e]; apply max_le hl1
    apply max_le <;> apply (div_le_one (by positivity)).mpr <;> linarith
  have cr : Real.cos r = l := Real.cos_arccos (by linarith) hl1
  have slope (k : ℝ) (hk : k ∈ ({1,2,3}:Set ℝ)) : 1 ≤ L k ∧ L k-1 ≤ (L k+1)*e := by
    rcases hk with hk | hk | hk
    · subst k; simp [L]; linarith
    · subst k; norm_num [L]
      constructor
      · linarith
      · have := (div_le_iff₀ (by positivity)).mp h2; linarith
    · change k = 3 at hk; subst k; norm_num [L]
      constructor
      · positivity
      · have := (div_le_iff₀ (by positivity)).mp h3; linarith
  have endpoint (k : ℝ) (hk : k ∈ ({1,2,3}:Set ℝ)) :
      Real.cos (k*r) = 1-L k*(1-l) := by
    rcases hk with hk | hk | hk
    · subst k; simp [L, cr]
    · subst k; norm_num [L, Real.cos_two_mul, cr]; ring
    · change k = 3 at hk; subst k; norm_num [L, Real.cos_three_mul, cr]; ring
  have chord (k : ℝ) (hk : k ∈ ({1,2,3}:Set ℝ)) (w : ℝ) (hw : l ≤ Real.cos w) :
      Real.cos (k*w) ≤ 1-L k*(1-Real.cos w) := by
    have hprod := mul_nonneg (sub_nonneg.mpr hw) (sub_nonneg.mpr (Real.cos_le_one w))
    rcases hk with hk | hk | hk
    · subst k; simp [L]
    · subst k; norm_num [L, Real.cos_two_mul]; linarith
    · change k = 3 at hk; subst k; norm_num [L, Real.cos_three_mul]
      nlinarith [mul_nonneg hprod (show 0 ≤ Real.cos w + l + 1 by linarith)]
  constructor
  · let p := (1-e)/(1-l)
    have hp : 0 ≤ p := div_nonneg (by linarith) (by linarith)
    have hp1 : p ≤ 1 := by
      rcases hl1.lt_or_eq with h | h
      · exact (div_le_one (by linarith)).mpr (by linarith)
      · simp [p, h]
    have hmass : p*(1-l) = 1-e := by
      rcases hl1.lt_or_eq with h | h
      · exact div_mul_cancel₀ _ (by linarith)
      · have : e = 1 := by linarith
        simp [h, this]
    let pin : ℝ≥0 := ⟨(1-p)/2, by linarith⟩
    let pout : ℝ≥0 := ⟨p/2, by positivity⟩
    have norm : pin*2+pout*2 = 1 := by apply NNReal.eq; change (1-p)/2*2+p/2*2 = 1; ring
    obtain ⟨μ, hμ, hμr, resp⟩ := symmetric_four_point_law (x := 0)
      (by simpa using Real.arccos_nonneg l : |(0:ℝ)| ≤ r) pin pout norm
    refine ⟨μ, hμ, 0, hμr, ?_⟩
    intro k hk phase
    rw [resp, mul_zero, Real.cos_zero, endpoint k hk, abs_mul]
    have id : 2*((pin:ℝ)*1+(pout:ℝ)*(1-L k*(1-l))) = 1-L k*(1-e) := by
      change 2*((1-p)/2*1+p/2*(1-L k*(1-l))) = _
      linear_combination -(L k)*hmass
    have ⟨hL, hLo⟩ := slope k hk
    have hr : |1-L k*(1-e)| ≤ e := abs_le.mpr ⟨by linarith,
      by nlinarith [mul_nonneg (sub_nonneg.mpr hL) (sub_nonneg.mpr he1)]⟩
    rw [id]
    exact (mul_le_of_le_one_right (abs_nonneg _) (abs_le.mpr
      ⟨Real.neg_one_le_cos _, Real.cos_le_one _⟩)).trans hr
  · rintro d ⟨μ, hμ, c, hsupp, herr⟩
    let f := fun k t : ℝ => Real.cos (k*(t-c))
    have hi (k : ℝ) : Integrable (f k) μ := cos_integrable μ k c
    have hm (k : ℝ) (hk : k ∈ ({1,2,3}:Set ℝ)) : |∫ t, f k t ∂μ| ≤ d :=
      centered_bound herr c k hk
    have hw : ∀ᵐ t ∂μ, l ≤ Real.cos (t-c) := by
      filter_upwards [hsupp] with t ht
      have h := Real.cos_le_cos_of_nonneg_of_le_pi (abs_nonneg (t-c)) (Real.arccos_le_pi l) ht
      change Real.cos r ≤ _ at h; rwa [cr, Real.cos_abs] at h
    have h₁ := (abs_le.mp (hm 1 (by simp))).2
    have ld : l ≤ d := by
      have hb : ∀ᵐ t ∂μ, l ≤ f 1 t := by simpa [f] using hw
      have := integral_mono_ae (integrable_const l) (hi 1) hb
      simpa using this.trans h₁
    have lower (k : ℝ) (hk : k ∈ ({1,2,3}:Set ℝ)) : L k-1 ≤ d*(L k+1) := by
      have hc : ∀ᵐ t ∂μ, L k-1 ≤ L k*f 1 t - f k t := by
        filter_upwards [hw] with t ht
        simp only [f, one_mul]; linarith [chord k hk (t-c) ht]
      have hh := integral_mono_ae (integrable_const (L k-1)) (((hi 1).const_mul (L k)).sub (hi k)) hc
      change (∫ _ : ℝ, L k-1 ∂μ) ≤ ∫ t, L k*f 1 t - f k t ∂μ at hh
      rw [integral_const, probReal_univ, one_smul, integral_sub (f := fun t => L k*f 1 t)
        (g := f k) ((hi 1).const_mul (L k)) (hi k), integral_const_mul] at hh
      have hₖ := (abs_le.mp (hm k hk)).1
      linarith [mul_le_mul_of_nonneg_left h₁ (show 0 ≤ L k by linarith [(slope k hk).1])]
    apply max_le ld; apply max_le
    · apply (div_le_iff₀ (by positivity)).mpr
      have := lower 2 (by simp); norm_num [L] at this; linarith
    · apply (div_le_iff₀ (by positivity)).mpr
      have := lower 3 (by simp); norm_num [L] at this; linarith


/-- The optimal worst-mode residual, expressed as a function of interval radius. -/
noncomputable def harmonicErrorCurve (r : ℝ) : ℝ :=
  let l := Real.cos r
  if 3*Real.pi/4 ≤ r then 0
  else if 0 ≤ l then max l (max ((2*l+1)/(2*l+3)) ((4*l*(1+l))/(4*l*(1+l)+2)))
  else (1-2*l^2)/(3-2*l-4*l^2)

/-- The curve is attained and bounds every probability measure, for every radius. -/
theorem harmonic_error_curve_exact {r : ℝ} (hr : 0 ≤ r) :
    IsLeast (harmonicErrors r) (harmonicErrorCurve r) := by
  let t := Real.sqrt 2 / 2
  have ht : 0 ≤ t := by dsimp [t]; positivity
  have ht2 : 2*t^2 = 1 := by dsimp [t]; nlinarith [Real.sq_sqrt (by norm_num : (0:ℝ) ≤ 2)]
  have hc : Real.cos (3*Real.pi/4) = -t := by
    rw [show 3*Real.pi/4 = Real.pi-Real.pi/4 by ring, Real.cos_pi_sub, Real.cos_pi_div_four]
  unfold harmonicErrorCurve
  dsimp only
  split_ifs with hwide hpos
  · have aeq : Real.arccos (-t) = 3*Real.pi/4 :=
      Real.arccos_eq_of_eq_cos (by positivity) (by linarith [Real.pi_pos]) hc.symm
    obtain ⟨μ, hμ, c, hs, hm⟩ := (three_harmonic_minimax ht ht2.le).1
    rw [aeq] at hs
    rw [ht2, sub_self, zero_div] at hm
    refine ⟨⟨μ, hμ, c, hs.mono (fun _ h => h.trans hwide), hm⟩, ?_⟩
    rintro d ⟨ν, _, _, _, hd⟩
    exact (abs_nonneg _).trans (hd 1 (by simp) 0)
  · simpa only [Real.arccos_cos hr (show r ≤ Real.pi by linarith [Real.pi_pos])] using
      three_harmonic_short_arc hpos (Real.cos_le_one r)
  · have hrπ : r ≤ Real.pi := by linarith [Real.pi_pos]
    have hm := Real.cos_le_cos_of_nonneg_of_le_pi hr (show 3*Real.pi/4 ≤ Real.pi by linarith [Real.pi_pos])
      (le_of_not_ge hwide)
    rw [hc] at hm
    have hx2 : 2*(-Real.cos r)^2 ≤ 1 := by
      nlinarith [mul_self_le_mul_self (a := -Real.cos r) (by linarith) (show -Real.cos r ≤ t by linarith)]
    have h := three_harmonic_minimax (x := -Real.cos r) (by linarith) hx2
    simpa only [neg_neg, Real.arccos_cos hr hrπ, neg_sq, mul_neg, ← sub_eq_add_neg] using h

/-- At radius 2π/3 the exact minimax residual is one sixth. -/
theorem two_thirds_pi_residual : IsLeast (harmonicErrors (2*Real.pi/3)) (1/6) := by
  have harc : Real.arccos (-(1/2:ℝ)) = 2*Real.pi/3 := by
    apply Real.arccos_eq_of_eq_cos (by positivity) (by linarith [Real.pi_pos])
    rw [show 2*Real.pi/3 = Real.pi-Real.pi/3 by ring, Real.cos_pi_sub, Real.cos_pi_div_three]
  have h := three_harmonic_minimax (x := 1/2) (by norm_num) (by norm_num)
  rwa [harc, show (1-2*(1/2:ℝ)^2)/(3+2*(1/2)-4*(1/2)^2) = 1/6 by norm_num] at h

/-- A near-optimal law at radius 2π/3 has little mass away from the four
extremal sites. The distances in this statement are in the cosine coordinate. -/
theorem near_optimal_harmonic_mass {μ : Measure ℝ} [IsProbabilityMeasure μ] {c η : ℝ}
    (hs : ∀ᵐ t ∂μ, |t-c| ≤ 2*Real.pi/3) (hb : HarmonicBound μ (1/6+η)) {d : ℝ} (hd : 0 < d) :
    d^3 * μ.real {t | d ≤ Real.cos (t-c)+1/2 ∧ d ≤ |Real.cos (t-c)-3/4|} ≤ 21/16*η := by
  let f := fun k t : ℝ => Real.cos (k*(t-c))
  let g := fun t : ℝ => 18*f 1 t - 16*f 2 t + 8*f 3 t - 7
  have factor (t : ℝ) : g t = 32*(Real.cos (t-c)+1/2)*(Real.cos (t-c)-3/4)^2 := by
    dsimp [g,f]; rw [one_mul, Real.cos_two_mul, Real.cos_three_mul]; ring
  have hi (k : ℝ) : Integrable (f k) μ := cos_integrable μ k c
  have ig : Integrable g μ :=
    ((((hi 1).const_mul 18).sub ((hi 2).const_mul 16)).add ((hi 3).const_mul 8)).sub (integrable_const 7)
  have nn : 0 ≤ᵐ[μ] g := by
    filter_upwards [hs] with t ht
    have h := Real.cos_le_cos_of_nonneg_of_le_pi (abs_nonneg (t-c))
      (show 2*Real.pi/3 ≤ Real.pi by linarith [Real.pi_pos]) ht
    rw [show 2*Real.pi/3 = Real.pi-Real.pi/3 by ring, Real.cos_pi_sub,
      Real.cos_pi_div_three, Real.cos_abs] at h
    rw [Pi.zero_apply, factor]
    exact mul_nonneg (mul_nonneg (by norm_num) (by linarith)) (sq_nonneg _)
  have sub : {t | d ≤ Real.cos (t-c)+1/2 ∧ d ≤ |Real.cos (t-c)-3/4|} ⊆ {t | 32*d^3 ≤ g t} := by
    intro t ht
    obtain ⟨h1, h2⟩ := ht
    change 32*d^3 ≤ g t
    have hsq := mul_self_le_mul_self hd.le h2
    rw [abs_mul_abs_self] at hsq
    rw [factor]; nlinarith [mul_le_mul h1 hsq (mul_self_nonneg d) (show 0 ≤ Real.cos (t-c)+1/2 by linarith)]
  have hmark := mul_meas_ge_le_integral_of_nonneg nn ig (32*d^3)
  have hmass := mul_le_mul_of_nonneg_left (measureReal_mono (μ := μ) sub) (show 0 ≤ 32*d^3 by positivity)
  have eval : (∫ t, g t ∂μ) = 18*(∫ t, f 1 t ∂μ) - 16*(∫ t, f 2 t ∂μ) +
      8*(∫ t, f 3 t ∂μ) - 7 := by
    dsimp only [g]
    rw [integral_sub (f := fun t => 18*f 1 t - 16*f 2 t + 8*f 3 t) (g := fun _ => 7)
      ((((hi 1).const_mul 18).sub ((hi 2).const_mul 16)).add ((hi 3).const_mul 8)) (integrable_const 7),
      integral_add (f := fun t => 18*f 1 t - 16*f 2 t) (g := fun t => 8*f 3 t)
      (((hi 1).const_mul 18).sub ((hi 2).const_mul 16)) ((hi 3).const_mul 8),
      integral_sub (f := fun t => 18*f 1 t) (g := fun t => 16*f 2 t)
      ((hi 1).const_mul 18) ((hi 2).const_mul 16)]
    simp only [integral_const_mul, integral_const, probReal_univ, one_smul]
  have hm (k : ℝ) (hk : k ∈ ({1,2,3}:Set ℝ)) : |∫ t, f k t ∂μ| ≤ 1/6+η := centered_bound hb c k hk
  rw [eval] at hmark
  linarith [(abs_le.mp (hm 1 (by simp))).2, (abs_le.mp (hm 2 (by simp))).1,
    (abs_le.mp (hm 3 (by simp))).2]

end Leanmath.ZetaWidth
