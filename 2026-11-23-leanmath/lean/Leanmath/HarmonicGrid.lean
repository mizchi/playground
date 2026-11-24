module

public import Leanmath.HarmonicResidual

/-! Three-harmonic minimax error at duration 2/3 of a period on a symmetric
clock grid. Grid points are specified by their cosine coordinates. -/

public section
namespace Leanmath.ZetaWidth

/-- All nonnegative allocations to the allowed times ±arccos(z). Separate
positive and negative time weights allow asymmetric competitors. -/
@[expose] def clockErrors (G : Finset ℝ) : Set ℝ :=
  {d | ∃ p n : ℝ → ℝ, (∀ z ∈ G, 0 ≤ p z ∧ 0 ≤ n z) ∧
    (∑ z ∈ G, (p z+n z)) = 1 ∧ ∀ k ∈ ({1,2,3}:Set ℝ), ∀ phase : ℝ,
      |∑ z ∈ G, (p z*Real.cos (k*Real.arccos z+phase) +
        n z*Real.cos (k*(-Real.arccos z)+phase))| ≤ d}

private theorem cubic (a b z : ℝ) :
    (3+4*a*b-2*(a+b))*z - (2*(a+b)-1)*(2*z^2-1) + (4*z^3-3*z) -
      (2*(a+b)-1-2*a*b) = 4*(z+1/2)*(z-a)*(z-b) := by ring

/-- An exact grid penalty determined by the two allowed cosine coordinates
bracketing 3/4. Additional admissible points outside their gap do not change
the minimum. The full permitted interval has angular width 4π/3. -/
theorem harmonic_clock_grid_exact (G : Finset ℝ) {a b : ℝ}
    (ha : 1/2 ≤ a) (ha' : a ≤ 3/4) (hb : 3/4 ≤ b) (hb' : b ≤ 1) (hab : a < b)
    (h₀ : -(1/2:ℝ) ∈ G) (h₁ : a ∈ G) (h₂ : b ∈ G)
    (grid : ∀ z ∈ G, -(1/2) ≤ z ∧ z ≤ 1 ∧ (z ≤ a ∨ b ≤ z)) :
    IsLeast (clockErrors G) (1/6 + 8*(3/4-a)*(b-3/4)/(3*(3+4*a*b))) := by
  classical
  let A := 3+4*a*b-2*(a+b)
  let B := 2*(a+b)-1
  let D := 2*(a+b)-1-2*a*b
  let e := D/(3+4*a*b)
  have den : 0 < 3+4*a*b := by nlinarith [mul_nonneg (show 0 ≤ a by linarith) (show 0 ≤ b by linarith)]
  have he : (A+B+1)*e = D := by
    rw [show A+B+1 = 3+4*a*b by dsimp [A,B]; ring]
    exact mul_div_cancel₀ _ den.ne'
  have hA : 0 ≤ A := by dsimp [A]; nlinarith [mul_nonneg (by linarith : 0 ≤ a-1/2) (by linarith : 0 ≤ b-1/2)]
  have hB : 0 ≤ B := by dsimp [B]; linarith
  have hpos : 0 < A+B+1 := by linarith
  have formula : 1/6 + 8*(3/4-a)*(b-3/4)/(3*(3+4*a*b)) = e := by
    dsimp [e,D]; field_simp [den.ne']; ring
  have he0 : 0 ≤ e := by rw [← formula]; positivity
  rw [formula]
  have cos₁ (z : ℝ) (hz : z ∈ G) : Real.cos (Real.arccos z) = z :=
    Real.cos_arccos (by linarith [(grid z hz).1]) (grid z hz).2.1
  constructor
  · let f := fun z : ℝ => 2*z^2+z-1
    let u := (b+1/2)*(b+1)*(b-3/4)
    let v := (a+1/2)*(a+1)*(3/4-a)
    let o := u*f a+v*f b
    let total := o+u+v
    have fpos {z : ℝ} (hz : 1/2 ≤ z) : 0 ≤ f z := by
      dsimp [f]; nlinarith [mul_nonneg (by linarith : 0 ≤ 2*z-1) (by linarith : 0 ≤ z+1)]
    have hu : 0 ≤ u := by dsimp [u]; positivity
    have hv : 0 ≤ v := by dsimp [v]; positivity
    have ho : 0 ≤ o := add_nonneg (mul_nonneg hu (fpos ha)) (mul_nonneg hv (fpos (by linarith)))
    have ht : 0 < total := by
      rcases ha'.lt_or_eq with h | h
      · have : 0 < v := by dsimp [v]; positivity
        dsimp [total]; linarith
      · have hh : 0 < b-3/4 := by linarith
        have : 0 < u := by dsimp [u]; positivity
        dsimp [total]; linarith
    let w := fun z : ℝ => ((if z = -(1/2) then o else 0) + (if z = a then u else 0) +
      (if z = b then v else 0))/total
    have sumw (F : ℝ → ℝ) : (∑ z ∈ G, w z*F z) = (o*F (-(1/2))+u*F a+v*F b)/total := by
      simp only [w, div_mul_eq_mul_div, ← Finset.sum_div, add_mul, ite_mul, zero_mul, Finset.sum_add_distrib]
      simp [show -(2:ℝ)⁻¹ ∈ G by simpa using h₀,h₁,h₂]
    have norm : (∑ z ∈ G, w z) = 1 := by simpa [total, ht.ne'] using sumw (fun _ => 1)
    let m := (o*(-(1/2))+u*a+v*b)/total
    have m₂ : (o*(2*(-(1/2))^2-1)+u*(2*a^2-1)+v*(2*b^2-1))/total = -m := by
      dsimp [m]; rw [← neg_div]; dsimp [o,f]; ring
    have m₃ : (o*(4*(-(1/2))^3-3*(-(1/2)))+u*(4*a^3-3*a)+v*(4*b^3-3*b))/total = m := by
      dsimp [m]; congr 1; dsimp [o,f,u,v]; ring
    have mval : m = e := by
      apply (mul_right_inj' hpos.ne').mp
      rw [he]
      dsimp only [m]
      rw [← mul_div_assoc, div_eq_iff ht.ne']
      dsimp [A,B,D,total,o,f,u,v]; ring
    refine ⟨fun z => w z/2, fun z => w z/2, ?_, ?_, ?_⟩
    · intro z _; dsimp [w]; constructor <;> positivity
    · simpa only [add_halves] using norm
    · intro k hk phase
      have pair (z : ℝ) : w z/2*Real.cos (k*Real.arccos z+phase) +
          w z/2*Real.cos (k*(-Real.arccos z)+phase) =
          (w z*Real.cos (k*Real.arccos z))*Real.cos phase := by
        rw [mul_neg, Real.cos_add, Real.cos_add, Real.cos_neg, Real.sin_neg]; ring
      simp only [pair, ← Finset.sum_mul, abs_mul]
      have center : |∑ z ∈ G, w z*Real.cos (k*Real.arccos z)| = e := by
        rw [sumw]
        rcases hk with h | h | h
        · subst k; simp only [one_mul, cos₁ _ h₀, cos₁ _ h₁, cos₁ _ h₂]
          change |m| = e; rw [mval, abs_of_nonneg he0]
        · subst k; simp only [Real.cos_two_mul, cos₁ _ h₀, cos₁ _ h₁, cos₁ _ h₂, m₂, abs_neg, mval, abs_of_nonneg he0]
        · change k = 3 at h; subst k
          simp only [Real.cos_three_mul, cos₁ _ h₀, cos₁ _ h₁, cos₁ _ h₂, m₃, mval, abs_of_nonneg he0]
      rw [center]
      exact mul_le_of_le_one_right he0 (abs_le.mpr ⟨Real.neg_one_le_cos _, Real.cos_le_one _⟩)
  · rintro d ⟨p,n,hpn,hnorm,herr⟩
    let w := fun z => p z+n z
    let M := fun (F : ℝ → ℝ) => ∑ z ∈ G, w z*F z
    have bound (k : ℝ) (hk : k ∈ ({1,2,3}:Set ℝ)) : |M (fun z => Real.cos (k*Real.arccos z))| ≤ d := by
      simpa only [M,w,add_mul, add_zero, mul_neg, Real.cos_neg] using herr k hk 0
    have h₁ : M id ≤ d := by
      convert (abs_le.mp (bound 1 (by simp))).2 using 1
      apply Finset.sum_congr rfl; intro z hz; simp [cos₁ _ hz]
    have h₂ : -d ≤ M (fun z => 2*z^2-1) := by
      convert (abs_le.mp (bound 2 (by simp))).1 using 1
      apply Finset.sum_congr rfl; intro z hz; simp [Real.cos_two_mul, cos₁ _ hz]
    have h₃ : M (fun z => 4*z^3-3*z) ≤ d := by
      convert (abs_le.mp (bound 3 (by simp))).2 using 1
      apply Finset.sum_congr rfl; intro z hz; simp [Real.cos_three_mul, cos₁ _ hz]
    have point (z : ℝ) (hz : z ∈ G) : D ≤ A*z-B*(2*z^2-1)+(4*z^3-3*z) := by
      have ⟨hl, _, gap⟩ := grid z hz
      have hs : 0 ≤ (z-a)*(z-b) := by
        rcases gap with l | r
        · exact mul_nonneg_of_nonpos_of_nonpos (by linarith) (by linarith)
        · exact mul_nonneg (by linarith) (by linarith)
      dsimp [D,A,B]
      nlinarith [cubic a b z, mul_nonneg (show 0 ≤ z+1/2 by linarith) hs]
    have cert : D ≤ A*M id-B*M (fun z => 2*z^2-1)+M (fun z => 4*z^3-3*z) := by
      calc
        D = ∑ z ∈ G, w z*D := by rw [← Finset.sum_mul, show (∑ z ∈ G, w z) = 1 from hnorm, one_mul]
        _ ≤ ∑ z ∈ G, w z*(A*z-B*(2*z^2-1)+(4*z^3-3*z)) :=
          Finset.sum_le_sum fun z hz => mul_le_mul_of_nonneg_left (point z hz) (add_nonneg (hpn z hz).1 (hpn z hz).2)
        _ = _ := by
          dsimp [M]; simp only [Finset.mul_sum, ← Finset.sum_sub_distrib, ← Finset.sum_add_distrib]
          apply Finset.sum_congr rfl; intro z _; ring
    apply le_of_mul_le_mul_left (a := A+B+1) _ hpos
    linarith [mul_le_mul_of_nonneg_left h₁ hA, mul_le_mul_of_nonneg_left h₂ hB]

/-- A clock with allowed times 0, ±π/3, ±2π/3 has optimum 1/5. -/
theorem five_tick_residual :
    IsLeast (clockErrors {-(1/2:ℝ), 1/2, 1}) (1/5) := by
  have h := harmonic_clock_grid_exact {-(1/2:ℝ), 1/2, 1} (a := 1/2) (b := 1)
    (by norm_num) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by simp) (by simp) (by simp) (by
      intro z hz; simp only [Finset.mem_insert, Finset.mem_singleton] at hz
      rcases hz with rfl | rfl | rfl <;> norm_num)
  convert h using 1; norm_num

/-- An a priori bound when only the width of the gap in cosine coordinates is known. -/
theorem clock_grid_penalty_le {a b : ℝ} (ha : 1/2 ≤ a)
    (hb : 3/4 ≤ b) :
    8*(3/4-a)*(b-3/4)/(3*(3+4*a*b)) ≤ (4/27)*(b-a)^2 := by
  have hab : 3/8 ≤ a*b := by
    nlinarith [mul_le_mul ha hb (by norm_num : (0:ℝ) ≤ 3/4) (by linarith : 0 ≤ a)]
  apply (div_le_iff₀ (show 0 < 3*(3+4*a*b) by linarith)).mpr
  nlinarith [mul_nonneg (sq_nonneg (b-a)) (show 0 ≤ a*b-3/8 by linarith),
    sq_nonneg ((3/4-a)-(b-3/4))]

end Leanmath.ZetaWidth
