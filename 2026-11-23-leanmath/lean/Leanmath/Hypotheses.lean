/-
# Analytic hypotheses, stated explicitly

We never use `axiom`.  Every conditional theorem in this library takes its analytic input as an
ordinary hypothesis `(h : RH)`, `(h : GRH)` or `(h : BachBound)`.  `#print axioms` therefore stays
clean, and "which hypothesis does this result really use?" can be read off the signature.

Why there are three hypotheses: almost all of the "derandomization" consequences need
**GRH for Dirichlet L-functions** (or even for Dedekind zeta functions), not just RH for `ζ`.
A proof of RH for `ζ` alone gives only the RH-level items (explicit prime gaps, Robin's
inequality, the Turing machine that searches for a counterexample to RH...).
-/
import Mathlib

namespace Leanmath

/-- The Riemann Hypothesis, exactly as Mathlib states it. -/
abbrev RH : Prop := RiemannHypothesis

/-- GRH for Dirichlet L-functions: every zero of `L(s, χ)` in the critical strip `0 < Re s < 1`
lies on the line `Re s = 1/2`.  (For non-primitive `χ` the extra Euler-factor zeros lie on
`Re s = 0`, outside the strip, so this formulation is harmless.) -/
def GRH : Prop :=
  ∀ (N : ℕ) [NeZero N] (χ : DirichletCharacter ℂ N) (s : ℂ),
    DirichletCharacter.LFunction χ s = 0 → 0 < s.re → s.re < 1 → s.re = 1 / 2

/-- `genBound n` (written `G(n)` in the README): the least `x` such that the residues
`1, …, x` that are units already generate `(ZMod n)ˣ`.  This is the single combinatorial quantity
that controls deterministic Miller–Rabin, square roots and `ℓ`-th roots mod `p`, primitive root
search, and so on. -/
noncomputable def genBound (n : ℕ) : ℕ :=
  sInf {x : ℕ | Subgroup.closure {u : (ZMod n)ˣ | (u : ZMod n).val ≤ x} = ⊤}

/-- **Bach (1990)**, *Explicit bounds for primality testing and related problems*,
Math. Comp. 55: assuming GRH, every proper subgroup of `(ZMod n)ˣ` misses some unit `< 2 (ln n)²`;
equivalently `G(n) ≤ 2 (ln n)²`.

The implication `GRH → BachBound` is analytic (explicit formula + zero counting) and is far
beyond current Mathlib; it is the one bridge we deliberately leave as an input.  Everything
downstream of `BachBound` is elementary and is what this library formalizes. -/
def BachBound : Prop := ∀ n : ℕ, 1 < n → (genBound n : ℝ) ≤ 2 * Real.log n ^ 2

end Leanmath
