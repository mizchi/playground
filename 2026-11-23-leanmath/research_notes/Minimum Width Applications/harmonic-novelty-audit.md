# Three-harmonic averaging: novelty audit and the cost of clock quantization

The finite-clock result is a worked specialization of the classical discrete moment problem. Cubic interpolation recovers the bracket and the constants. The calculation uses neither zeta ordinates nor the Riemann hypothesis. The local Lean proof verifies the minimax value. Priority of the continuous-duration formula remains unresolved.

## Candidate ledger

| Result | What is established | Priority assessment |
| --- | --- | --- |
| Continuous three-harmonic minimax curve | A closed formula and an attaining law for every duration, over arbitrary probability measures. The value 1/6 is one point of this curve. | Candidate explicit solution of a low-dimensional instance of a classical moment problem. The zero-error duration is already known. [Curve and proof](harmonic-residual.md) · [Kozma–Oravecz, Thm. 2](https://arxiv.org/pdf/math/0601021) |
| Finite-clock penalty | Under the bracket conditions below, only the two ticks bracketing the ideal inner site and the endpoints are needed. The increase above 1/6 is an explicit product of the two offsets. | Classical point-mass bound with three prescribed moments, followed by a sign check for the minimax objective. [Lean proof](../../lean/Leanmath/HarmonicGrid.lean) · [Discrete moment criteria](https://arxiv.org/abs/1504.02989) |
| Concentration near the optimum | At two thirds of a period the mass away from the four optimal times is bounded in terms of the excess residual. | Supporting result. The existing constant follows by integrating the same nonnegative cubic and applying Markov's inequality. Quadratic localization certificates have a broader literature, and the present distance exponent is not established as sharp. [Local bound](harmonic-residual.md#near-optimal-distributions) · [Eftekhari et al.](https://arxiv.org/abs/1804.01490) |
| Zeta-specific application | A strict positive extra width for a third distinct height in one dyadic band. The claimed first-three scale ratio 2.55970 still requires a certified identification of the low zeros and their numerical optimum in Lean. | A separate application. The harmonic results apply to exact frequency ratios 1:2:3. [Band theorem and scope](../ゼータ零点%20最小幅の精密化/pair-gap.md) |

## What a discrete clock costs

Fix angular frequencies 1, 2, 3 and a centered time interval [−2π/3, 2π/3]. Allowed times come in pairs ±t. Their nonnegative weights sum to one and may differ within each pair. The performance criterion is the largest residual across all three modes and all phases. Describe a clock by its finite set G of cosine coordinates z = cos t. A tick at z = −1/2 means both interval endpoints are available.

Suppose G lies in [−1/2, 1], contains −1/2 and contains consecutive coordinates a < b with

$$
\tfrac12\le a\le\tfrac34\le b\le1.
$$

Then the exact minimum is

$$
E_G=\frac16+
\frac{8(3/4-a)(b-3/4)}{3(3+4ab)}.
$$

This formula also applies when one side equals 3/4. The penalty then vanishes. The Lean theorem bounds the penalty by $(4/27)(b-a)^2$. Here the gap is measured in cosine coordinates. Since cosine is 1-Lipschitz, the same upper bound using the squared angular tick spacing also follows. Adding any number of admissible ticks outside the open bracket preserves the exact value. [Main theorem, construction and corollaries](../../lean/Leanmath/HarmonicGrid.lean)

**Five-tick example.** A time grid consisting of 0, ±π/3 and ±2π/3 has a = 1/2 and b = 1. Its optimum is 1/5, attained by uniform weights. Free placement on the same interval achieves 1/6. The clock therefore costs 1/30 of input amplitude, a 20% increase relative to the optimal residual. On these ticks, $2\cos t-2\cos(2t)+\cos(3t)=1$, so any allocation with all amplitudes at most e must satisfy $5e\ge1$. This example has a direct elementary derivation and illustrates the general formula.

**Why distant ticks cannot help.** The continuous certificate has a double root at 3/4. On the discrete grid the polynomial $(z+1/2)(z-a)(z-b)$ is nonnegative, including at every extra tick, and vanishes at the three needed coordinates. Expanding it in $1,\cos t,\cos 2t,\cos 3t$ gives the lower bound. To attain it, set $F(z)=2z^2+z-1$. Assign raw weights $u=(b+1/2)(b+1)(b-3/4)$ at a, $v=(a+1/2)(a+1)(3/4-a)$ at b, and $uF(a)+vF(b)$ at −1/2. Normalize the total and split each mass equally across the reflected times. The signed cosine moments are $(E_G,-E_G,E_G)$. One pass through an unsorted clock finds the bracket; the weights then require only a fixed number of arithmetic operations.

**A boundary check.** Extending the expression to a = −1/4 and b = 1 would predict 1/2. On the clock with coordinates {−1/2, −1/4, 1} the optimum is 5/7. At these nodes $6\cos(3t)-\cos(2t)\ge5$, and weights 0, 32/35, 3/35 give signed moments −1/7, −5/7, 5/7. The rational identities are included in the independent check. [Reproduction script](../../explore/harmonic_residual_check.py)

## Reduction to a classical point-mass bound

Let $I(p)$ be the average of $p(\cos t)$ over a full period. Its moments of degrees zero through three are $(1,0,1/2,0)$. On the ordered nodes $\{-1\}\cup G$, minimize the mass $w$ at $-1$ among probability measures exact for these four moments. This is the discrete power moment LP with objective $f=\mathbf1_{\{-1\}}$. Its fourth divided differences are nonnegative. A tuple excluding $-1$ gives zero, and one containing it gives the reciprocal of a product of four negative differences. The first two nodes are $-1,-1/2$, and $a,b$ are consecutive. These two adjacent pairs form precisely the minimization basis in Prékopa's Theorem A.1 for $m=3$. [Discrete moment LP, §3 and Appendix A](https://rutcor.rutgers.edu/~prekopa/30_2006.pdf)

For a direct proof let $P(z)=(z+1/2)(z-a)(z-b)$. Then $P(z)/P(-1)\le f(z)$ at every node, with equality on the four selected nodes. Integrating the four Lagrange cardinal polynomials gives an exact degree-three quadrature. Its weight at $-1$ is
$$
w=I(P)/P(-1)=\frac{2(a+b)-1-2ab}{2(1+a)(1+b)}.
$$
The weights at $a,b$ are $(b-3/4)/[(1+a)(a+1/2)(b-a)]$ and $(3/4-a)/[(1+b)(b+1/2)(b-a)]$. Positivity selects the bracket around $3/4$. The weight at $-1/2$ is $[(1-a)(1-b)+ab]/[(a+1/2)(b+1/2)]$. The four weights sum to one by exactness on constants. They realize the lower bound even when an inner weight vanishes.

Removing the atom at $-1$ and normalizing leaves cosine moments $(e,-e,e)$ with $e=w/(1-w)$, since the full-circle averages of the first three Chebyshev polynomials $T_k$ vanish and $T_k(-1)=(-1)^k$. To check that no other sign pattern does better, expand $4P=A T_1-B T_2+T_3-D$, where $A=2+(2a-1)(2b-1)$, $B=2(a+b)-1$, and $D=2(a+b)-1-2ab$. As $P\ge0$ on $G$ and $A,B\ge0$, every admissible residual $d$ satisfies $(A+B+1)d\ge D$. The quadrature attains this bound, since $D/(A+B+1)=w/(1-w)$. Subtracting $1/6$ gives the product of the two offsets in the clock formula. This derivation accounts for the constants irrespective of whether the same expression has appeared in clock notation.

## Which parts follow from earlier mathematics?

**Continuous moment feasibility.** For a centered arc of radius $0<r<\pi$, reflection removes sine moments without increasing the maximum residual. The pushforward variable $Z=\cos t$ ranges over $[l,1]$ with $l=\cos r$. Write $y_j=\mathbb E[Z^j]$, $y_0=1$. The three signed responses are $y_1,2y_2-1,4y_3-3y_1$. The degree-three Hausdorff moment criterion is exactly two positive-semidefinite constraints: with $H=(y_{i+j})_{i,j=0}^1$ and $K=(y_{i+j+1})_{i,j=0}^1$, require $K-lH\succeq0$ and $H-K\succeq0$. Bounding the absolute values of the three responses adds six linear inequalities. Thus classical theory already gives an exact optimization problem of fixed size. The explicit value as a function of r is the narrower claim. Yang and Xie give the corresponding feasibility framework directly on an arc. [Curto–Fialkow, §4](https://www.math.uh.edu/~hjm/v017n4/0603CURTO.pdf) · [Laurent, Thm. 5.39](https://homepages.cwi.nl/~monique/files/moment-ima-update.pdf) · [Yang–Xie](https://arxiv.org/abs/1605.02431)

**Discrete moment geometry.** The vector $(z,2z^2-1,4z^3-3z)$ is an invertible affine image of $(z,z^2,z^3)$. Its convex hull over clock nodes has the classical cyclic-polytope face structure. A supporting plane through the left endpoint and two consecutive nodes is therefore expected. Discrete moment realizability also uses polynomials that are nonnegative on the prescribed node set. Our additional calculation selects the bracket around 3/4, proves that its face meets the target ray $(e,-e,e)$ with nonnegative weights, and computes the resulting distance to the origin in the maximum norm. [Gale on cyclic polytopes](https://paisajes.matcuer.unam.mx/IMG/pdf/1963-gale-neighborly-cyclic-polytopes.pdf) · [Infusino–Kuna–Lebowitz–Speer](https://arxiv.org/abs/1504.02989)

**Engineering and sampling design.** Van den Broeck et al. already optimize clocked input shapers by linear programming for flexible systems. A separate signal-processing interpretation follows directly here. Form the four columns $V_{jk}=\sqrt{w_j}\,e^{ikt_j}$ for k = 0, 1, 2, 3. Each has squared norm one, and each off-diagonal Gram entry is a Fourier moment at a difference frequency ±1, ±2 or ±3. The exact grid formula is therefore also the **minimum mutual coherence of these four columns** over all allowed row weights. This identity is a mathematical consequence of the theorem; the local Lean code formalizes the amplitude optimization. For comparison, published optimal-design work for Fourier regression on partial circles maximizes the information determinant, a different objective. [Van den Broeck et al., ISMA 2008](https://citeseerx.ist.psu.edu/document?doi=3453580e3a8cb1cd61a4a759e91e00da041149c2&repid=rep1&type=pdf) · [Dette–Melas–Pepelyshev](https://www.ism.ac.jp/editsec/aism/54/945.html)

## Verification and search scope

From the experiment directory:

```sh
(cd lean && lake build Leanmath.HarmonicGrid)
(cd lean && printf 'import Leanmath.HarmonicGrid\nopen Leanmath.ZetaWidth\n#print axioms harmonic_clock_grid_exact\n#print axioms five_tick_residual\n#print axioms clock_grid_penalty_le\n' | lake env lean --stdin)
uv run --script explore/harmonic_residual_check.py
```

The theorem permits asymmetric competitors and constructs a symmetric optimizer. The independent LP uses reflection symmetry and solves for nonnegative weights on every available cosine coordinate. With seed 314159, 96 irregular grids gave a maximum discrepancy of $6.72\times10^{-15}$ between the linear program and the grid formula. Two grids containing the ideal site and the coarse five-tick case were checked separately.

The literature search covered truncated Hausdorff and trigonometric moments, discrete moment realizability, cyclic-polytope facets, minimax residual vibration, time discretization of positive input shapers and mutual-coherence sampling design. Searches included “discrete moment problem adjacent roots”, “fixed duration minimax residual vibration multiple modes”, and “minimum coherence Fourier sampling restricted interval weights”. The primary sources above establish membership tests, face structure and optimization methods. This search did not locate the same clock penalty or continuous residual curve stated explicitly. Neither bibliographic priority nor novelty of the general methods follows from that observation.
