# Which frequencies disappear at the optimal two-frequency width?

A single positive average can cancel two specified oscillations at every starting phase. Once its interval is as short as possible, its entire set of cancelled frequencies is forced. This gives a decision rule for adding a third mode. It also gives bounds for triples of simple critical-line zeta zeros without a table of zero heights.

## Exact spectrum of a shortest average

Use log-scale time t and angular frequencies 0 < b < h < 3b. An average is a probability measure μ on an interval. It cancels k if the integral of cos(kt + φ) is zero for every phase φ. The interval may have any center. Write a = 2π/(b+h).

| Question | Lean-verified answer |
| --- | --- |
| What is the least full interval width for b and h? | 2a = 4π/(b+h), attained. |
| Where can an optimal measure put weight? | Only at the center and the two endpoints, up to a set of measure zero. |
| Which other frequencies does **each** optimal measure cancel? | Exactly k = n(b+h) ± b, for integers n. One measure cancels this entire set simultaneously. |
| What happens at any fixed k outside this set? | A positive gap separates 2a from the infimum of the feasible full widths for b, h, k. |
| How much additional width suffices for k > 0? | π/k. The bound is attained as a feasible width by averaging the original measure and its translate by π/k. |

For example, a shortest average for frequencies 1 and 2 has width 4π/3. It also cancels 4. Adding frequency 3 requires a strictly longer interval, even if every sampling location and weight may be changed. The arithmetic test here is membership in the two residue classes ±1 modulo 3. [Lean proof and example](../../lean/audit/ZetaBand.lean)

The optimal construction in centered coordinates uses weights proportional to (1, −2 cos(ba), 1) at (−a, 0, a). For the lower bound, cos(bt) − cos(ht) is positive for 0 < |t| < a and zero at the center and endpoints. Its integral against a cancelling measure is zero. This forces the support of every optimizer onto those three sites. On these sites shifting a frequency by b+h adds whole turns to the centered phase. To exclude widths arbitrarily close to 2a for an off-lattice k, the proof constructs a linear combination of the three cosines that is strictly positive throughout a slightly larger interval. A cancelling measure there would give that function integral zero. The size of this enlargement depends on the frequencies. [Lean proof](../../lean/Leanmath/ZetaPair.lean)

The two-mode width has direct precedents. Kozma and Oravecz's Theorem 3 with K=1 gives the commensurable case after angular-frequency normalization. Equal-spacing three-impulse shapers for two undamped modes also appear in the engineering literature. The result recorded here packages the arbitrary-probability-measure spectrum, the positive gap, and the zeta specialization as kernel-checked statements. Priority for the mathematical classification is unverified. [Kozma and Oravecz](https://arxiv.org/pdf/math/0601021) · [Two-mode input shaping](https://ntrs.nasa.gov/api/citations/19950024365/downloads/19950024365.pdf)

## Consequence for actual zeta zeros

Take T > 0 and three heights T < γ₁ < γ₂ < γ₃ ≤ 2T. The angular frequencies of interest in squarefree counting are γᵢ/2. Let W₂ and W₃ denote the infima of the **full log widths** for the first two and all three selected frequencies. Then

$$
W_2=\frac{8\pi}{\gamma_1+\gamma_2},
\qquad
W_2<W_3\le W_2+\frac{2\pi}{\gamma_3}.
$$

The width W₂ and the displayed upper bound on W₃ each have an explicit feasible average. Lean supplies a positive gap depending on the triple. In this band the third positive frequency lies above h and below b+h, so it falls strictly between successive cancelled frequencies. [Band theorem](../../lean/Leanmath/ZetaPair.lean)

For every integer m, every sufficiently high band contains at least m simple critical-line zeros, and every ordered triple of distinct heights among them satisfies these bounds. All these zeros have real part 1/2, so distinct zeros have distinct heights. The counting argument uses Theorem B at ε=1/6 to bound the simple count below by half the total count, and the dyadic Riemann–von Mangoldt consequence to make the total count exceed 2m. The local theorem takes these two upstream facts as parameters. It checks the counting conventions by definitional equality against Anthropic's original ChallengeDeps.lean. The upstream analytic proofs use Lean 4.33.0-rc2 and were not replayed locally. [Local theorem](../../lean/Leanmath/ZetaBand.lean) · [Theorem B](https://github.com/anthropics/formal-math/blob/11b6c7e3c927df2658f513b6b68ea7f8929c62c5/zeta23/Solution.lean#L60-L62) · [Dyadic count growth](https://github.com/anthropics/formal-math/blob/11b6c7e3c927df2658f513b6b68ea7f8929c62c5/zeta23/Zeta23/Assembly.lean#L754-L757) · [Riemann–von Mangoldt for zeta](https://github.com/anthropics/formal-math/blob/11b6c7e3c927df2658f513b6b68ea7f8929c62c5/zeta23/Zeta23/Final.lean#L234-L236)

These results concern three selected oscillations. The earlier 2.55970 is the proposed multiplicative scale ratio exp(W₃) for the first three positive ordinates. Identifying those ordinates and certifying their particular nonlinear optimum in Lean remain open tasks in this project. [First-three certificate status](lean-certificate.md)

## Reproduction

From the experiment's lean directory, run the local proofs and the optional interface audit. The audit compiles the pinned upstream definition file under the local toolchain. It checks the two counts, specializes the transfer theorem, exercises the example above, and prints the axioms of the four main results.

```sh
lake build
mkdir -p .lake/zeta-band-audit
curl -fsSL https://raw.githubusercontent.com/anthropics/formal-math/11b6c7e3c927df2658f513b6b68ea7f8929c62c5/zeta23/ChallengeDeps.lean -o .lake/zeta-band-audit/ChallengeDeps.lean
lake env lean -R .lake/zeta-band-audit -o .lake/zeta-band-audit/ChallengeDeps.olean .lake/zeta-band-audit/ChallengeDeps.lean
lake env sh -c 'LEAN_PATH=".lake/zeta-band-audit:$LEAN_PATH" lean audit/ZetaBand.lean'
```
