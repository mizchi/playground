/-
# Miller's deterministic primality test, conditional on `BachBound`

Structure of the classical proof (Miller 1976, Bach 1990), split so that GRH enters exactly once:

* `strongLiars_le_proper` (elementary, unconditional; Monier–Rabin): for odd composite `n` the
  strong liars lie in a proper subgroup of `(ZMod n)ˣ`.
* `exists_witness_le_genBound` (unconditional): so some witness is `≤ G(n)`.
* `exists_small_witness` (BachBound): so some witness is `≤ 2 ln² n`.
* `millerDet_iff` (BachBound): the executable `millerDet` decides primality.

Experiment `explore/gn.c` checks `W(n) ≤ G(n) ≤ 2 ln² n` numerically for `n ≤ 4·10⁵`.
-/
import Leanmath.GenBound

namespace Leanmath

/-- `m = 2^s · d` with `d` odd (for `m > 0`); computable. -/
def splitTwo (m : ℕ) : ℕ × ℕ :=
  if h : m ≠ 0 ∧ m % 2 = 0 then
    let r := splitTwo (m / 2)
    (r.1 + 1, r.2)
  else (0, m)
termination_by m
decreasing_by omega

lemma splitTwo_spec (m : ℕ) :
    2 ^ (splitTwo m).1 * (splitTwo m).2 = m ∧ (m ≠ 0 → Odd (splitTwo m).2) := by
  induction m using Nat.strong_induction_on with
  | _ m ih =>
    rw [splitTwo]
    split_ifs with h
    · obtain ⟨h1, h2⟩ := ih (m / 2) (by omega)
      refine ⟨?_, fun _ => h2 (by omega)⟩
      simp only [pow_succ]
      rw [mul_comm _ 2, mul_assoc, h1]
      omega
    · refine ⟨by simp, fun hm => Nat.odd_iff.mpr (by omega)⟩

/-- `s` in `n - 1 = 2^s · d`. -/
def twoVal (n : ℕ) : ℕ := (splitTwo (n - 1)).1

/-- `d` in `n - 1 = 2^s · d`. -/
def oddPart (n : ℕ) : ℕ := (splitTwo (n - 1)).2

/-- `a` is a strong liar for `n`: writing `n - 1 = 2^s · d` with `d` odd,
`a^d = 1` or `a^(2^r d) = -1` for some `r < s`. -/
def IsStrongLiar (n : ℕ) (a : ZMod n) : Prop :=
  a ^ oddPart n = 1 ∨ ∃ r < twoVal n, a ^ (2 ^ r * oddPart n) = -1

lemma two_pow_mul_oddPart (n : ℕ) : 2 ^ twoVal n * oddPart n = n - 1 :=
  (splitTwo_spec (n - 1)).1

lemma odd_oddPart {n : ℕ} (h : 1 < n) : Odd (oddPart n) :=
  (splitTwo_spec (n - 1)).2 (by omega)

lemma twoVal_pos {n : ℕ} (hodd : Odd n) (h : 1 < n) : 0 < twoVal n := by
  obtain ⟨k, rfl⟩ := hodd
  unfold twoVal
  rw [splitTwo]
  split_ifs with hc
  · dsimp only; omega
  · omega

/-- Every strong liar is a Fermat liar. -/
lemma pow_sub_one_of_liar {n : ℕ} {a : ZMod n} (h : IsStrongLiar n a) : a ^ (n - 1) = 1 := by
  rw [← two_pow_mul_oddPart n]
  rcases h with h | ⟨r, hr, h⟩
  · rw [mul_comm, pow_mul, h, one_pow]
  · obtain ⟨j, hj⟩ : ∃ j, twoVal n = r + (j + 1) := ⟨twoVal n - r - 1, by omega⟩
    have hexp : 2 ^ twoVal n * oddPart n = 2 ^ r * oddPart n * 2 ^ (j + 1) := by
      rw [hj, pow_add]; ring
    rw [hexp, pow_mul, h]
    exact Even.neg_one_pow ⟨2 ^ j, by ring⟩

/-- Monier–Rabin, prime-power case `n = p^e`, `e ≥ 2`: the Fermat liars form a proper subgroup,
since `(1 + p)^(p^e - 1) ≢ 1 (mod p²)`. -/
lemma strongLiars_le_proper_primePow {p e : ℕ} (hp : p.Prime) (he : 2 ≤ e) :
    ∃ H : Subgroup (ZMod (p ^ e))ˣ, H ≠ ⊤ ∧
      ∀ u : (ZMod (p ^ e))ˣ, IsStrongLiar (p ^ e) u → u ∈ H := by
  refine ⟨(powMonoidHom (p ^ e - 1) : (ZMod (p ^ e))ˣ →* (ZMod (p ^ e))ˣ).ker, ?_, ?_⟩
  · intro htop
    have hcop : Nat.Coprime (1 + p) (p ^ e) :=
      (Nat.coprime_add_self_left.mpr (Nat.coprime_one_left p)).pow_right e
    have hmem : ZMod.unitOfCoprime (1 + p) hcop ∈
        (powMonoidHom (p ^ e - 1) : (ZMod (p ^ e))ˣ →* (ZMod (p ^ e))ˣ).ker :=
      htop ▸ Subgroup.mem_top _
    rw [MonoidHom.mem_ker, powMonoidHom_apply, Units.ext_iff, Units.val_pow_eq_pow_val,
      ZMod.coe_unitOfCoprime, Units.val_one] at hmem
    have hdvd : p ^ 2 ∣ p ^ e := pow_dvd_pow p he
    have h2 := congrArg (ZMod.castHom hdvd (ZMod (p ^ 2))) hmem
    rw [map_pow, map_one, map_natCast] at h2
    have hpp : (p : ZMod (p ^ 2)) * p = 0 := by
      rw [← Nat.cast_mul, ← sq, ZMod.natCast_self]
    have key : ∀ N : ℕ, ((1 + p : ℕ) : ZMod (p ^ 2)) ^ N = 1 + (N : ZMod (p ^ 2)) * p := by
      intro N
      induction N with
      | zero => simp
      | succ N ih =>
        rw [pow_succ _ N, ih]
        push_cast
        linear_combination (N : ZMod (p ^ 2)) * hpp
    rw [key] at h2
    have h3 : (((p ^ e - 1) * p : ℕ) : ZMod (p ^ 2)) = 0 := by
      rw [Nat.cast_mul]
      linear_combination h2
    have h4 : p * p ∣ (p ^ e - 1) * p := by
      rw [← sq]; exact (ZMod.natCast_eq_zero_iff _ _).mp h3
    have h5 : p ∣ p ^ e - 1 := Nat.dvd_of_mul_dvd_mul_right hp.pos h4
    have h6 : p ∣ p ^ e := dvd_pow_self p (by omega)
    have h7 : p ^ e - (p ^ e - 1) = 1 := by
      have := Nat.one_le_pow e p hp.pos
      omega
    have h8 := Nat.dvd_sub h6 h5
    rw [h7] at h8
    exact hp.not_dvd_one h8
  · intro u hu
    rw [MonoidHom.mem_ker, powMonoidHom_apply, Units.ext_iff, Units.val_pow_eq_pow_val,
      Units.val_one]
    exact pow_sub_one_of_liar hu

/-- Monier–Rabin, split case `n = n₁ n₂` with coprime `n₁, n₂ > 2`.  Let `k` be the largest
`r < s` such that some unit has `u^(2^r d) = -1` and put `m = 2^k d`.  Then
`H = {u | u^m = ±1}` contains every strong liar, and the CRT lift of `(u₀, 1)` (with
`u₀^m = -1`) satisfies `b^m ≡ (-1, 1)`, so `b ∉ H`.  No Carmichael hypothesis is needed. -/
lemma strongLiars_le_proper_coprime {n₁ n₂ : ℕ} (h₁ : 2 < n₁) (h₂ : 2 < n₂)
    (hcop : n₁.Coprime n₂) (hodd : Odd (n₁ * n₂)) :
    ∃ H : Subgroup (ZMod (n₁ * n₂))ˣ, H ≠ ⊤ ∧
      ∀ u : (ZMod (n₁ * n₂))ˣ, IsStrongLiar (n₁ * n₂) u → u ∈ H := by
  classical
  have hN1 : 1 < n₁ * n₂ := by nlinarith
  have hs : 0 < twoVal (n₁ * n₂) := twoVal_pos hodd hN1
  have hd : Odd (oddPart (n₁ * n₂)) := odd_oddPart hN1
  let P : ℕ → Prop := fun r =>
    ∃ u : (ZMod (n₁ * n₂))ˣ, u ^ (2 ^ r * oddPart (n₁ * n₂)) = -1
  have hP0 : P 0 := ⟨-1, by simp [hd.neg_one_pow]⟩
  set k := Nat.findGreatest P (twoVal (n₁ * n₂) - 1) with hk_def
  have hk : P k := Nat.findGreatest_spec (Nat.zero_le _) hP0
  set m := 2 ^ k * oddPart (n₁ * n₂) with hm_def
  let H : Subgroup (ZMod (n₁ * n₂))ˣ :=
    { carrier := {u | u ^ m = 1 ∨ u ^ m = -1}
      one_mem' := by simp
      mul_mem' := by
        rintro a b (ha | ha) (hb | hb) <;> simp [mul_pow, ha, hb]
      inv_mem' := by
        rintro a (ha | ha) <;> simp [inv_pow, ha] }
  refine ⟨H, ?_, ?_⟩
  · intro htop
    obtain ⟨u₀, hu₀⟩ := hk
    have hu₀' : (u₀ : ZMod (n₁ * n₂)) ^ m = -1 := by
      rw [← Units.val_pow_eq_pow_val, hu₀, Units.val_neg, Units.val_one]
    let e := ZMod.chineseRemainder hcop
    let b : ZMod (n₁ * n₂) := e.symm ((e (u₀ : ZMod (n₁ * n₂))).1, 1)
    have hbm : e (b ^ m) = (-1, 1) := by
      rw [map_pow, RingEquiv.apply_symm_apply, Prod.pow_mk, one_pow, ← Prod.pow_fst, ← map_pow,
        hu₀', map_neg, map_one]
      rfl
    have hm0 : m ≠ 0 := Nat.mul_ne_zero (by positivity) hd.pos.ne'
    have hb2 : b ^ (m * 2) = 1 := by
      apply e.injective
      rw [pow_mul, map_pow, hbm, map_one]
      ext <;> simp
    have hbu : IsUnit b := IsUnit.of_pow_eq_one hb2 (by omega)
    have hmem : hbu.unit ∈ H := htop ▸ Subgroup.mem_top _
    have : Fact (2 < n₁) := ⟨h₁⟩
    have : Fact (2 < n₂) := ⟨h₂⟩
    rcases hmem with h | h
    · have hb : b ^ m = 1 := by simpa using congrArg Units.val h
      rw [hb, map_one] at hbm
      exact ZMod.neg_one_ne_one (congrArg Prod.fst hbm).symm
    · have hb : b ^ m = -1 := by simpa using congrArg Units.val h
      rw [hb, map_neg, map_one] at hbm
      exact ZMod.neg_one_ne_one (congrArg Prod.snd hbm)
  · intro u hu
    show u ^ m = 1 ∨ u ^ m = -1
    rcases hu with hu | ⟨r, hr, hu⟩
    · left
      ext
      rw [Units.val_pow_eq_pow_val, hm_def, mul_comm (2 ^ k), pow_mul, hu, one_pow, Units.val_one]
    · have hu' : u ^ (2 ^ r * oddPart (n₁ * n₂)) = -1 := by
        ext; rw [Units.val_pow_eq_pow_val, hu, Units.val_neg, Units.val_one]
      have hrk : r ≤ k := Nat.le_findGreatest (by omega) ⟨u, hu'⟩
      have hm : m = 2 ^ r * oddPart (n₁ * n₂) * 2 ^ (k - r) := by
        rw [hm_def, mul_right_comm, ← pow_add, Nat.add_sub_cancel' hrk]
      rw [hm, pow_mul, hu']
      exact neg_one_pow_eq_or _ _

/-- **Monier–Rabin**: for odd composite `n` the strong liars are contained in a proper
subgroup of `(ZMod n)ˣ`. -/
theorem strongLiars_le_proper {n : ℕ} (hodd : Odd n) (h1 : 1 < n) (hc : ¬ n.Prime) :
    ∃ H : Subgroup (ZMod n)ˣ, H ≠ ⊤ ∧ ∀ u : (ZMod n)ˣ, IsStrongLiar n u → u ∈ H := by
  set p := n.minFac with hp_def
  have hp : p.Prime := Nat.minFac_prime (by omega)
  have hpn : p ∣ n := Nat.minFac_dvd n
  have hp2 : p ≠ 2 := by
    intro h
    rw [h] at hpn
    exact (Nat.not_even_iff_odd.mpr hodd) (even_iff_two_dvd.mpr hpn)
  set e := n.factorization p with he_def
  have he1 : 0 < e := hp.factorization_pos_of_dvd (by omega) hpn
  have hsplit : p ^ e * (n / p ^ e) = n := Nat.ordProj_mul_ordCompl_eq_self n p
  by_cases hrest : n / p ^ e = 1
  · have hn : n = p ^ e := by rw [hrest, mul_one] at hsplit; exact hsplit.symm
    have he : 2 ≤ e := by
      by_contra hlt
      have he' : e = 1 := by omega
      rw [he', pow_one] at hn
      exact hc (hn ▸ hp)
    rw [hn]
    exact strongLiars_le_proper_primePow hp he
  · have hcop : (p ^ e).Coprime (n / p ^ e) := (Nat.coprime_ordCompl hp (by omega)).pow_left e
    have hp3 : 2 < p := lt_of_le_of_ne hp.two_le (Ne.symm hp2)
    have h₁ : 2 < p ^ e := lt_of_lt_of_le hp3 (Nat.le_self_pow he1.ne' p)
    have hodd₂ : Odd (n / p ^ e) := Odd.of_dvd_nat hodd (Nat.div_dvd_of_dvd (Nat.ordProj_dvd n p))
    have hne0 : n / p ^ e ≠ 0 := by
      intro h0; rw [h0, mul_zero] at hsplit; omega
    have h₂ : 2 < n / p ^ e := by
      rcases hodd₂ with ⟨t, ht⟩
      omega
    rw [← hsplit]
    exact strongLiars_le_proper_coprime h₁ h₂ hcop (by rw [hsplit]; exact hodd)

/-- Unconditional: some base `a ≤ G(n)` is a Miller witness for odd composite `n`. -/
theorem exists_witness_le_genBound {n : ℕ} (hodd : Odd n) (h1 : 1 < n) (hc : ¬ n.Prime) :
    ∃ a : ℕ, 0 < a ∧ a ≤ genBound n ∧ a < n ∧ a.Coprime n ∧ ¬ IsStrongLiar n a := by
  have : NeZero n := ⟨by omega⟩
  have : Fact (1 < n) := ⟨h1⟩
  obtain ⟨H, hH, hliar⟩ := strongLiars_le_proper hodd h1 hc
  obtain ⟨u, hle, hu⟩ := exists_not_mem_le_genBound H hH
  refine ⟨(u : ZMod n).val, ?_, hle, ZMod.val_lt _, ZMod.val_coe_unit_coprime u, ?_⟩
  · exact Nat.pos_of_ne_zero fun h0 => u.ne_zero ((ZMod.val_eq_zero _).mp h0)
  · rw [ZMod.natCast_zmod_val]
    exact fun h => hu (hliar u h)

/-- **GRH corollary** (via Bach): Miller's test with bases `a ≤ 2 ln² n` is a correct
deterministic primality test. -/
theorem exists_small_witness (hB : BachBound) {n : ℕ} (hodd : Odd n) (h1 : 1 < n)
    (hc : ¬ n.Prime) :
    ∃ a : ℕ, 0 < a ∧ (a : ℝ) ≤ 2 * Real.log n ^ 2 ∧ ¬ IsStrongLiar n a := by
  obtain ⟨a, ha0, hle, -, -, hw⟩ := exists_witness_le_genBound hodd h1 hc
  refine ⟨a, ha0, ?_, hw⟩
  calc (a : ℝ) ≤ genBound n := by exact_mod_cast hle
    _ ≤ 2 * Real.log n ^ 2 := hB n h1

/-! ## An executable version, and its correctness under `BachBound` -/

def powMod (a e n : ℕ) : ℕ :=
  if h : e = 0 then 1 % n
  else
    let r := powMod (a * a % n) (e / 2) n
    if e % 2 = 1 then r * a % n else r
termination_by e
decreasing_by omega

theorem powMod_eq (a e n : ℕ) : powMod a e n = a ^ e % n := by
  induction e using Nat.strong_induction_on generalizing a with
  | _ e ih =>
    rw [powMod]
    split_ifs with h0 h1
    · subst h0; simp
    · rw [ih _ (by omega), ← Nat.pow_mod, Nat.mod_mul_mod]
      conv_rhs => rw [← Nat.div_add_mod e 2, h1, pow_succ, pow_mul, sq]
    · rw [ih _ (by omega), ← Nat.pow_mod]
      conv_rhs => rw [← Nat.div_add_mod e 2, show e % 2 = 0 by omega, add_zero, pow_mul, sq]

/-- Strong probable prime test to base `a` (for odd `n > 2`). -/
def sprp (n a : ℕ) : Bool :=
  powMod a (oddPart n) n == 1 ||
    (List.range (twoVal n)).any fun r => powMod a (2 ^ r * oddPart n) n == n - 1

lemma mod_eq_one_iff {n : ℕ} (hn : 1 < n) (x : ℕ) : x % n = 1 ↔ (x : ZMod n) = 1 := by
  rw [show (1 : ZMod n) = ((1 : ℕ) : ZMod n) from Nat.cast_one.symm,
    ZMod.natCast_eq_natCast_iff' x 1 n, Nat.mod_eq_of_lt hn]

lemma mod_eq_neg_one_iff {n : ℕ} (hn : 1 < n) (x : ℕ) :
    x % n = n - 1 ↔ (x : ZMod n) = -1 := by
  have : (-1 : ZMod n) = ((n - 1 : ℕ) : ZMod n) := by
    rw [Nat.cast_sub (by omega), ZMod.natCast_self]; simp
  rw [this, ZMod.natCast_eq_natCast_iff', Nat.mod_eq_of_lt (by omega : n - 1 < n)]

theorem sprp_iff {n : ℕ} (hn : 1 < n) (a : ℕ) : sprp n a = true ↔ IsStrongLiar n (a : ZMod n) := by
  simp only [sprp, IsStrongLiar, Bool.or_eq_true, beq_iff_eq, List.any_eq_true, List.mem_range,
    powMod_eq, mod_eq_one_iff hn, mod_eq_neg_one_iff hn, Nat.cast_pow]

lemma pow_two_pow_eq_one {F : Type*} [Field F] :
    ∀ (s : ℕ) (x : F), x ^ 2 ^ s = 1 → x = 1 ∨ ∃ r < s, x ^ 2 ^ r = -1
  | 0, x, h => Or.inl (by simpa using h)
  | s + 1, x, h => by
    have h' : (x ^ 2 ^ s) ^ 2 = 1 := by rw [← pow_mul, ← pow_succ]; exact h
    rcases sq_eq_one_iff.mp h' with h1 | h1
    · rcases pow_two_pow_eq_one s x h1 with h2 | ⟨r, hr, h2⟩
      · exact Or.inl h2
      · exact Or.inr ⟨r, by omega, h2⟩
    · exact Or.inr ⟨s, by omega, h1⟩

/-- For a prime `p`, every nonzero residue is a strong liar. -/
theorem liar_of_prime {p : ℕ} [Fact p.Prime] {a : ZMod p} (ha : a ≠ 0) : IsStrongLiar p a := by
  have hF := ZMod.pow_card_sub_one_eq_one ha
  rw [← two_pow_mul_oddPart p, mul_comm, pow_mul] at hF
  rcases pow_two_pow_eq_one _ _ hF with h | ⟨r, hr, h⟩
  · exact Or.inl h
  · exact Or.inr ⟨r, hr, by rw [mul_comm, pow_mul]; exact h⟩

/-- `2 (ln n)² ≤ 2 (⌊log₂ n⌋ + 1)²`, a computable over-approximation of Bach's bound. -/
def millerBound (n : ℕ) : ℕ := 2 * (Nat.log 2 n + 1) ^ 2

lemma two_log_sq_le_millerBound {n : ℕ} (hn : 1 < n) :
    2 * Real.log n ^ 2 ≤ (millerBound n : ℝ) := by
  set L := Nat.log 2 n
  have h1 : (n : ℝ) < 2 ^ (L + 1) := by exact_mod_cast Nat.lt_pow_succ_log_self one_lt_two n
  have hlog0 : 0 ≤ Real.log n := Real.log_natCast_nonneg n
  have h2 : Real.log n < (L + 1 : ℕ) * Real.log 2 := by
    rw [← Real.log_pow]
    exact Real.log_lt_log (by positivity) (by exact_mod_cast h1)
  have h3 : Real.log 2 < 1 := by have := Real.log_two_lt_d9; norm_num at this; linarith
  have h4 : Real.log n ≤ (L + 1 : ℕ) := by
    have : (0 : ℝ) ≤ (L + 1 : ℕ) := by positivity
    nlinarith [Real.log_pos (by norm_num : (1 : ℝ) < 2)]
  unfold millerBound
  push_cast at h4 ⊢
  nlinarith

/-- Miller's deterministic test. -/
def millerDet (n : ℕ) : Bool :=
  if n < 2 then false
  else if n % 2 = 0 then n == 2
  else (List.range (min (millerBound n) (n - 1) + 1)).all fun a =>
    a < 2 || (Nat.gcd a n == 1 && sprp n a)

/-- **Main theorem.** Under Bach's bound (a consequence of GRH), Miller's deterministic test is
correct.  The only non-elementary input is the hypothesis `hB`. -/
theorem millerDet_iff (hB : BachBound) (n : ℕ) : millerDet n = true ↔ n.Prime := by
  unfold millerDet
  split_ifs with h2 hev
  · simp only [false_iff]
    intro hp; have := hp.two_le; omega
  · simp only [beq_iff_eq]
    constructor
    · rintro rfl; exact Nat.prime_two
    · intro hp
      rcases hp.eq_two_or_odd with h | h
      · exact h
      · omega
  · have hodd : Odd n := Nat.odd_iff.mpr (by omega)
    have h1 : 1 < n := by omega
    rw [List.all_eq_true]
    constructor
    · intro hall
      by_contra hc
      obtain ⟨a, ha0, hle, han, hcop, hw⟩ := exists_witness_le_genBound hodd h1 hc
      have ha2 : 2 ≤ a := by
        by_contra h
        have ha1 : a = 1 := by omega
        subst ha1
        exact hw (Or.inl (by simp))
      have hbound : a ≤ millerBound n := by
        have : (a : ℝ) ≤ millerBound n :=
          ((by exact_mod_cast hle : (a : ℝ) ≤ genBound n).trans (hB n h1)).trans
            (two_log_sq_le_millerBound h1)
        exact_mod_cast this
      have hmem := hall a (List.mem_range.mpr (by omega))
      simp only [Bool.or_eq_true, decide_eq_true_eq, Bool.and_eq_true, beq_iff_eq] at hmem
      rcases hmem with h | ⟨-, hs⟩
      · omega
      · exact hw ((sprp_iff h1 a).mp hs)
    · intro hp a ha
      have : Fact n.Prime := ⟨hp⟩
      simp only [Bool.or_eq_true, decide_eq_true_eq, Bool.and_eq_true, beq_iff_eq]
      by_cases ha2 : a < 2
      · exact Or.inl ha2
      right
      have han : a < n := by rw [List.mem_range] at ha; omega
      refine ⟨(Nat.coprime_of_lt_prime (by omega) han hp).symm, ?_⟩
      rw [sprp_iff h1]
      apply liar_of_prime
      intro h0
      rw [ZMod.natCast_eq_zero_iff] at h0
      exact absurd (Nat.le_of_dvd (by omega) h0) (by omega)

-- sanity checks
#eval (List.range 60).filter millerDet           -- primes < 60
#eval millerDet 3215031751                      -- composite, 2/3/5/7-strong pseudoprime
#eval millerDet 2147483647                      -- 2^31 - 1, prime

end Leanmath
