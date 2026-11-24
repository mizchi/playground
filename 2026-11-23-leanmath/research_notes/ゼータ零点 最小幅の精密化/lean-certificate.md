# What Lean establishes for positive zero-frequency averages

The project uses Lean 4.36.0 and mathlib revision `fc4f59e`. In mathlib, `RiemannHypothesis : Prop` defines a statement about every nontrivial zero. A theorem that takes `hRH : RiemannHypothesis` depends on a proof supplied by its caller. [Pinned definition](https://github.com/leanprover-community/mathlib4/blob/fc4f59e577feb1a4956671bc04ac18aaaf8cd842/Mathlib/NumberTheory/LSeries/RiemannZeta.lean#L183-L186)

Anthropic's Theorem B bounds the number of simple critical-line zeros in sufficiently high windows (T, 2T]. For each ε > 0, their count is at least (2/3 − ε) times the total count with multiplicities. This quantifier leaves every fixed initial window outside the scope of the estimate if its threshold is large enough. The outcome is compatible with different values for the first three ordinates. [Upstream proof](https://github.com/anthropics/formal-math/blob/11b6c7e3c927df2658f513b6b68ea7f8929c62c5/zeta23/Solution.lean#L60-L66)

## A consequence of the published density bound

For an ordinate γ in (T, 2T] with T > 0, two equal observations separated by 2π/γ in log scale annihilate the γ/2 oscillation in every starting phase. This distance is strictly less than 2π/T. Adding this property to the simple critical-line zeros in Theorem B preserves their count, so the same (2/3 − ε) lower bound holds for zeros with this property. The separation can depend on the zero. A common filter for several zeros requires a separate argument.

The local theorem `band_clock_count_of_density` takes the density bound as an explicit parameter. For Theorem B, specialize S and N as below. The membership proof extracts T < Im(ρ) directly from the upstream definition of the window. The upstream proof and the local project pin different Lean toolchains. The adapter was typechecked locally with the upstream counting definitions and exact type of Theorem B as an argument. The upstream analytic proof was not rebuilt locally. [Upstream counting definitions](https://github.com/anthropics/formal-math/blob/11b6c7e3c927df2658f513b6b68ea7f8929c62c5/zeta23/ChallengeDeps.lean#L40-L67)

```lean
-- In the notation of zeta23, with the local theorem in scope:
-- S T := zerosIn T (2*T) ∩ {ρ | ρ.re = 1/2} ∩ {ρ | zeroMult ρ = 1}
-- N T := Ncount T (2*T)
-- Leanmath.ZetaWidth.band_clock_count_of_density S N (2/3)
--   (fun _ _ h => h.1.1.2.1) two_thirds_simple_on_critical_line
```

## What the three-frequency certificate still owes

`three_frequency_minimum` takes three frequencies, candidate sites ±x and ±y, two nonnegative atom masses, three exact cosine moment identities, and sign information for a linear combination of the three cosines. It constructs a probability measure attaining radius y. It proves that every interval supporting a cancelling probability measure, with any center, has radius at least y. The resulting minimum log width is 2y. Its proof uses no hypothesis about ζ. The one-frequency lower bound and an explicit equal-weight optimal pair are also formalized. [Lean source](../../lean/Leanmath/ZetaWidth.lean)

`scale_ratio_of_log_width` uses rigorous Taylor bounds to prove that 0.93989042 ≤ L ≤ 0.93989043 implies 2.559695 < exp(L) < 2.559705. This proves the rounding step to 2.55970.

A complete theorem naming the first three zeros must still identify their certified ordinate intervals, prove that the nonlinear moment equations have a solution in the reported box, and verify the score's sign and zero set and the first-cosine inequality for that solution. The Python certificate checks the latter numerical conditions using the published ordinate intervals. The global uniqueness of the four atom masses also needs its sine-moment argument in Lean. Neither the density theorem nor even RH alone supplies these finite numerical computations. [Numerical certificate](../../explore/three_zero_width.py)

## Reproduction

From the experiment directory:

```sh
cd lean
lake build Leanmath.ZetaWidth
printf 'import Leanmath.ZetaWidth\n#print axioms Leanmath.ZetaWidth.three_frequency_minimum\n#print axioms Leanmath.ZetaWidth.scale_ratio_of_log_width\n#print axioms Leanmath.ZetaWidth.band_clock_count_of_density\n' | lake env lean --stdin
```

The subsequent [two-frequency spectrum and three-frequency gap](pair-gap.md) gives unconditional results for real frequencies and an application to triples of simple critical-line zeros in high bands.
