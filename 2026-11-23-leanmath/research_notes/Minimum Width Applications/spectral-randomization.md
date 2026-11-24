# Minimum latency for dephasing a harmonic ladder with a miscalibrated clock

Research log, 24 November 2026. Units use angular frequency and $\hbar=1$. The results below are unconditional. The link to the zeta project is an extremal problem for characteristic functions. The explicit proof for the ladder uses positivity on a finite interval. The incommensurate example uses the slit-plane bound developed in the project.

## 1. How long must a passive clock wait to erase all coherences to first order?

**Takeaway.** For $d\ge2$ energies $0,\Delta,\ldots,(d-1)\Delta$, the minimum worst-case wait for exact dephasing with zero linear sensitivity to a common relative frequency error is

\[
L_{2}=\frac{4\pi}{\Delta}\frac{d-1}{d}.
\]

There is a unique minimizing probability law up to translation. Draw two independent uniform integers $J_1,J_2\in\{0,\ldots,d-1\}$ and wait $2\pi(J_1+J_2)/(d\Delta)$. This works in any eigenbasis and never requires learning that basis. The hard limit is exactly twice the minimum for nominal dephasing.

**Cited findings.** Boixo, Knill and Somma describe dephasing by random Hamiltonian evolution, identify the characteristic function at the relevant gaps, and bound average absolute evolution time. Their comparison to phase estimation explains the role of controlled evolution and discarded information. Cunningham and Roland develop randomization for an entire eigenpath. The resource here is the maximum elapsed Hamiltonian evolution time for one passive dephasing step. The known minimum for nominal harmonics follows by convex separation from Theorem 2 of Kozma and Oravecz with $N=1$, $K=d-2$ (the single gap follows by the semicircle argument). They credit earlier work for $N=1$. [Boixo, Knill and Somma, §§II and V](https://arxiv.org/html/0903.1652) · [Cunningham and Roland](https://drops.dagstuhl.de/entities/document/10.4230/LIPIcs.TQC.2024.7) · [Kozma and Oravecz, Theorem 2](https://arxiv.org/html/math/0601021)

**Inferences and proof.** Let $H$ have the stated eigenvalues, $\mathcal D$ be complete energy-basis dephasing, and let a fixed probability law $\mu$ on $[0,L]$ specify the wait. Write $F(\omega)=\mathbb E\exp(i\omega t)$. Under the actual generator $(1+\eta)H$, the matrix unit $|k\rangle\langle l|$ acquires the factor $F((l-k)(1+\eta)\Delta)$. Exact dephasing and $\|\mathcal E_\eta-\mathcal D\|_\diamond=O(\eta^2)$ are equivalent to

\[
F(j\Delta)=F'(j\Delta)=0\quad(1\le j<d).
\]

Indeed the two-level superposition for each gap gives a trace-norm residual equal to the modulus of this multiplier. Conversely every entry has zero constant and linear terms by Taylor's theorem, and all norms on this finite-dimensional space of maps are equivalent. The schedule and the target basis are fixed as $\eta$ varies. Multiple waits under the same generator, chosen using only classical randomness independent of the system and drift, reduce to the distribution of their sum. Feedback from quantum measurements, extra control gates and coherent phase estimation can implement different channels and require their own resource model.

Scale time by $\Delta$ so the target frequencies are $j$. Put $h=2\pi/d$ and $a=(d-1)h$. It suffices to classify all feasible laws on $[-a,a]$, since translating a characteristic function multiplies it by a phase and preserves double zeros. For $d\ge3$, use

\[
A(t)=\frac{1-\cos(dt)}{\cos t-\cos h},\qquad B(t)=A(t)\cot(t/2),\qquad
S(t)=-A(t)+\frac{\tan(h/2)}{h}\,t B(t).
\]

The Chebyshev identity $\cos(dt)=T_d(\cos t)$ removes the apparent poles of $A$ and gives an even trigonometric polynomial of degree $d-1$. At $t=\pm h$ modulo $2\pi$ its numerator has a double zero and its denominator a simple zero. In fact $A(kh)=0$ at every grid point. Averaging on one period over the $d$ grid nodes integrates all harmonics up to degree $d-1$ exactly, so $A$ has zero mean. As a polynomial in $\cos t$ it is divisible by $1-\cos t$. Hence $B=\sin(t)A/(1-\cos t)$ is an odd trigonometric polynomial of degree at most $d-1$. Thus $S$ is a linear combination of $\cos jt$ and $t\sin jt$ for $1\le j<d$. Every feasible law integrates $S$ to zero.

Between adjacent grid nodes, $1-\cos(dt)>0$. On $(0,a)$ the denominator $\cos t-\cos h$ is positive up to $h$ and negative after $h$. The function $t\cot(t/2)$ decreases strictly on $(0,2\pi)$ since its derivative is $(\sin t-t)/(2\sin^2(t/2))$. At $h$ it equals $h\cot(h/2)$. These signs give $S(t)>0$ at every point strictly between adjacent grid nodes in $[-a,a]$, by evenness. At every grid node both terms vanish, including $t=0$ by continuity. Positivity of $\mu$ now restricts its support to $kh$, $|k|\le d-1$. For $d=2$ the separating function is simply $t\sin t$ on $[-\pi,\pi]$, with zeros at $-\pi,0,\pi$.

Shift this grid to $0,h,\ldots,2(d-1)h$ and let $W(z)=\sum_k p_k z^k$ encode its weights. Then $F(\omega)=W(e^{ih\omega})$. The conditions imply that all the nontrivial $d$th roots of unity are zeros of $W$ of multiplicity at least two. Since $\deg W\le 2(d-1)$ and $W(1)=1$,

\[
W(z)=d^{-2}(1+z+\cdots+z^{d-1})^2.
\]

Both endpoint masses are positive. Every schedule of diameter at most $2a$ fits inside $[-a,a]$ after translation, so this forces the full width. The coefficients define a feasible probability law, proving existence and uniqueness. On a nonnegative physical clock the earliest possible start is zero. The resulting mean is $L_2/2$ and its variance is $(2\pi^2/(3\Delta^2))(1-d^{-2})$. These moments describe the width minimizer.

**Gaps and prior-art boundary.** The triangular weights and their Fejér frequency response are standard signal processing. The two-mode case $d=3$ is contained in the undamped simultaneous ZVD–ZVD input-shaper literature, which treats frequency ratios from 1 to 3 with five impulses. Logan's sharp count of real zeros, with multiplicity, gives $2\pi/\Delta$ from the lowest double zero even for arbitrary clocks after centering and symmetrizing. It need not exploit the other specified locations. Margolus studies distinguishable successive states and makes the simple-zero clock connection explicit. In the infinite harmonic limit, Strang–Fix theory and Blu, Thévenaz and Unser's minimum-support approximation kernels include the triangular cardinal B-spline of width $4\pi/\Delta$ in this scaling. The finite-clock laws here converge to that continuous triangular law. Searches spanning multiple-mode shapers, finite Strang–Fix conditions, prescribed multiple Fourier zeros, positive-definite functions and minimum-support windows located these precedents. Priority for the all-$d$ finite, arbitrary-measure extremum and its uniqueness remains unverified. [Singhose, Crain and Seering, §§3–4](https://doi.org/10.1049/ip-cta:19971439) · [Logan, Part III](https://epubs.siam.org/doi/abs/10.1137/0514024) · [Margolus](https://arxiv.org/abs/2111.00297) · [Blu, Thévenaz and Unser](https://www.ee.cuhk.edu.hk/~tblu/monsite/pdfs/blu0101.pdf) · [Triangular window definition and implementation](https://github.com/JuliaDSP/DSP.jl/blob/master/src/windows.jl)

## 2. What error and resource savings does the optimal clock deliver?

**Takeaway.** The joint sequence stays below two full periods $4\pi/\Delta$ for any number of levels. Cascading a separately optimized first-order-robust clock for every positive gap takes $(2\pi/\Delta)H_{d-1}$. Their duration ratio is $d H_{d-1}/[2(d-1)]$. At $d=8$ the separate clock takes 1.482 times as long. A 1% common calibration drift leaves at most 0.005830 of any off-diagonal entry for the joint clock, with a diamond-norm error bound of 0.01210.

**Cited findings.** Cascading individual vibration shapers adds their durations. Direct design exploits shared zeros among the different modes. The first-order robustness constraint and the separate versus simultaneous comparison are standard in vibration control. [Singhose, Crain and Seering](https://doi.org/10.1049/ip-cta:19971439)

**Inferences and check.** If $1\le j<d$, the exact magnitude of the multiplier at fractional drift $\eta$ is

\[
r_j(\eta)=\left[\frac{\sin(\pi j\eta)}{d\sin(\pi j(1+\eta)/d)}\right]^2,
\]

with removable singularities filled by continuity. This follows from the geometric series for the uniform grid and convolution. In particular, for fixed $d$, $r_j(\eta)=[\pi j/(d\sin(\pi j/d))]^2\eta^2+O(\eta^3)$. The largest-gap coefficient is asymptotic to $d^2$ as $d\to\infty$.

To control entangled inputs, use the deterministic time shift that centers each uniform wait at zero. Its distribution is $Y=2\pi(J-(d-1)/2)/(d\Delta)$. Let $K_\eta$ be its random unitary channel. Since $K_0=\mathcal D$ and $\mathcal D K_\eta=K_\eta\mathcal D=\mathcal D$, two independent centered waits satisfy $K_\eta^2-\mathcal D=(K_\eta-\mathcal D)^2$. The commutator generator has diamond norm at most $\mathrm{diam}(H)$ by subtracting the spectral midpoint of $H$ before bounding its two multiplication maps. Integration of the unitary flow gives $\|K_\eta-K_0\|_\diamond\le |\eta|\,\mathrm{diam}(H)\,\mathbb E|Y|$. Summing the equally spaced deviations gives $\mathbb E|Y|=\pi b_d/(2\Delta)$, where $b_d=1$ for even $d$ and $1-d^{-2}$ for odd $d$. Physical forward waits add a deterministic evolution, whose output unitary preserves $\mathcal D$ and the diamond distance. For the physical channel,

\[
\max_{1\le j<d}r_j(\eta)\le\|\mathcal E_\eta-\mathcal D\|_\diamond
\le \min\left\{2,\left(\tfrac\pi2(d-1)b_d\,|\eta|\right)^2\right\}.
\]

The left side is witnessed by a two-level superposition. This uses the diamond norm itself, without the conventional factor of one half for distinguishing advantage. A single optimized double notch at $j\Delta$ uses the $d=2$ law with width $2\pi/(j\Delta)$, giving the harmonic-sum comparison when all are cascaded.

| Levels $d$ | Exact positive weights, before dividing by $d^2$ | $\Delta L_2$ | Separate / joint |
|---:|---|---:|---:|
| 3 | 1, 2, 3, 2, 1 | 8.377580 | 1.1250 |
| 4 | 1, 2, 3, 4, 3, 2, 1 | 9.424778 | 1.2222 |
| 8 | 1 through 8 and back to 1 | 10.995574 | 1.4816 |

The script checks the two exact root conditions by folding the integer coefficients of $W$ and $zW'$ modulo $z^d-1$. In each case all residue classes have the same coefficient. It separately samples the sign of $S$ inside every grid cell. Run `python3 -B explore/spectral_randomization.py`. [Reproducibility script](../../explore/spectral_randomization.py)

**Gaps.** The uniform calibration factor is fixed during a run. Sharp bounds for dynamics that change the eigenvectors, time-dependent errors during evolution, and optimum higher-multiplicity clocks require additional arguments. The $r$-fold convolution gives an explicit upper bound $2\pi r(d-1)/(d\Delta)$ for multiplicity $r$.

## 3. What latency can be certified when a small residual coherence is allowed?

**Takeaway.** For $H=\Delta\,\mathrm{diag}(0,1,1+\sqrt2)$, any passive clock leaving at most 1% of each individual coherence requires $L>5.06/\Delta$. Optimizing only the smallest gap would give $2\arccos(0.01)/\Delta\approx3.122/\Delta$. For the equally spaced qutrit the exact 1% threshold is $4.165540419/\Delta$. This question concerns nominal accuracy at fixed gaps, without derivative conditions.

**Cited findings.** Attenuation of the input off-diagonal entries by a characteristic function is the characteristic-function description of randomized evolution. The bound on expected absolute evolution time in that model can be read from a single slow gap. A maximum-duration bound can also depend on how the other gaps are distributed. [Boixo, Knill and Somma, Theorems 1–2](https://arxiv.org/html/0903.1652)

**Inferences and proof.** A finite-tolerance slit inequality applies to any distinct positive frequencies $\omega_k$, with prescribed attenuations $|F(\omega_k)|\le\epsilon_k$. Center the clock on $[-a,a]$, set $R>\max\omega_k$, and introduce both signs $x_i=\pm\omega_k$ with the same tolerance. Define

\[
s_i=\sqrt{R^2-x_i^2},\quad w_i=\frac{x_i}{R+s_i},\quad B(w)=\prod_i\frac{w-w_i}{1-w_i w},\quad b=|B(0)|.
\]

Every feasible clock must satisfy

\[
1\le b e^{aR}+b\sum_i \frac{\epsilon_i e^{a(R-s_i)}}{|w_i|\,|B'(w_i)|}.
\]

To prove this, map the plane cut along $(-\infty,-R]\cup[R,\infty)$ to the disk by $w=z/(R+\sqrt{R^2-z^2})$. On the branch positive at zero, $\Re\sqrt{R^2-z^2}\ge |\Im z|$, by squaring and using $|R^2-z^2|\ge |z|^2-R^2$. Thus $G(w)=F(z)e^{-a\sqrt{R^2-z^2}}$ has modulus at most one. Integrate $G(w)B(0)/(wB(w))$ on a circle approaching the unit circle. The normalized integral has limiting upper bound $b$ in modulus, since $|B|$ tends uniformly to 1 there. The residues at zero and at $w_i$ are $e^{-aR}$ and $G(w_i)B(0)/(w_iB'(w_i))$. The triangle inequality yields the display. At zero tolerance it reduces to $aR\ge-\log b$. With positive tolerances the right side increases with $a$, giving certificates by direct substitution.

In units $\Delta=1$, use all three gaps $1,\sqrt2,1+\sqrt2$, with $R=3.04$, $a=2.53$. Interval arithmetic with outward rounding encloses the right side in $(0.99968239573755202,0.99968239573755203)$, proving the strict exclusion. The script uses integer square roots and a Taylor remainder bound for the exponential. An exact feasible upper bound follows by adding three independent binary waits, one uniform on $\{0,\pi/\omega_k\}$ per gap, for total width $\pi\sum_k1/\omega_k\approx6.664324407$. Necessary pairwise error constraints also follow from a 1% diamond-norm requirement. [Interval calculation](../../explore/spectral_randomization.py)

For $H=\Delta\,\mathrm{diag}(0,1,2)$, the exact minimum at individual tolerance $0\le\epsilon\le1$ is

\[
\Delta L(\epsilon)=
\begin{cases}
2\arccos\!\dfrac{3\epsilon-1}{2(1-\epsilon)},&0\le\epsilon\le 1/2,\\
2\arccos\epsilon,&1/2\le\epsilon\le1.
\end{cases}
\]

Center any candidate and average it with its reflection, which preserves the width constraint and can only reduce each modulus. The zero-tolerance grid already has half-width $2\pi/3$ in dimensionless units, so it suffices to consider $a\le2\pi/3$. Put $X=\cos(\Delta t)$, $m=\cos(a)$, where $a=\Delta L/2$. Then $X\in[m,1]$, $|\mathbb E X|\le\epsilon$, and $|2\mathbb E X^2-1|\le\epsilon$. The chord inequality $X^2\le(1+m)X-m$ gives $m\le(3\epsilon-1)/(2(1-\epsilon))$ for $\epsilon<1$. Also $m\le\epsilon$. For the first branch put mass $(\epsilon-m)/(1-m)$ at $t=0$ and split the rest equally at $t=\pm a/\Delta$. This attains residuals $\epsilon,-\epsilon$. On the second branch symmetric endpoint masses attain residuals $\epsilon,2\epsilon^2-1$, both within tolerance. Translation implements each law with forward waits. At 1% the first construction uses mass $4949/14750$ at its center.

**Secondary proposition.** For the same equally spaced qutrit, the optimal width for a diamond-norm tolerance $\delta=0.01$ is $4.170681440881/\Delta$. More generally, let $a=\Delta L/2$, $m=\cos a$ and $v=1+2m$. On the range $0\le v\le\sqrt5$ (hence $0\le a\le 2\pi/3$), the optimal achievable diamond error at this width is

\[
D_{\min}(a)=\frac{16v}{(5+v)^2}.
\]

Reflection of a centered schedule complex-conjugates its Schur multiplier in the energy basis. Simultaneous entrywise conjugation of inputs and outputs, including any ancillary system, preserves the trace norm. Thus reflection fixes the distance to ideal dephasing, and convexity permits symmetrization. For the centered even law put $x=F(\Delta)$, $y=F(2\Delta)$, both real. The channel difference multiplies matrix entries by a real symmetric matrix $C$ with zero diagonal, adjacent entries $x$ and corner entries $y$.

To maximize over all inputs and ancillas, it suffices to take a pure input $\sum_i|i\rangle u_i$. Put $p_i=\|u_i\|^2$. The output difference on the span of the orthogonal vectors $|i\rangle u_i$ is unitarily equivalent to $M=\mathrm{diag}(\sqrt p)C\,\mathrm{diag}(\sqrt p)$. Zero $p_i$ can be omitted. On three vertices, diagonal sign changes and an overall sign make the three real off-diagonal entries nonnegative, preserving the trace norm. The resulting matrix $|M|_{\rm entry}$ has trace zero, nonnegative determinant (twice the product of its off-diagonal entries) and spectral radius $\lambda_{\max}$. Its trace norm therefore equals $2\lambda_{\max}$. The Rayleigh quotient has a nonnegative maximizing unit vector $z$. Setting $s_i=\sqrt{p_i}z_i$ gives $\sum_i s_i\le1$ by Cauchy–Schwarz. Every probability vector $s$ is realized with $p_i=s_i$, $z_i=\sqrt{s_i}$. Maximizing over $p,z$ thus gives $2\max_{s_i\ge0,\,\sum s_i=1}s^T|C|_{\rm entry}s$. For fixed $u=s_0+s_2$ equal endpoints maximize their product, yielding the exact diamond norm

\[
\max_{0\le u\le1}\left(4|x|u(1-u)+|y|u^2\right).
\]

The pure-input characterization for Hermiticity-preserving maps is Theorem 3.51 of Watrous. The three-dimensional reduction above is specific to this calculation. [Theorem 3.51, The Theory of Quantum Information](https://johnwatrous.com/wp-content/uploads/TQI.pdf)

The moment bound already proved gives $y\le(1+v)x-v$. Substituting $u=4/(5+v)$, using $|x|\ge x$ and $|y|\ge-y$, cancels the coefficient of $x$ and gives $16v/(5+v)^2$ for every law. To attain it, put mass $(x-m)/(1-m)$ at the center and split the rest equally at the ends, with $x=2v/(5+v)$. The mass is feasible since $x\ge m$ is equivalent to $v^2\le5$, and $x\le1$ on this range. Then $y=(1+v)x-v=v(v-3)/(5+v)\le0$. For $v>0$ the displayed quadratic in $u$ is concave with maximum at $4/(5+v)$ for these $x,y$. At $v=0$ both coherences vanish. As this error increases strictly for $0\le v\le\sqrt5$, any smaller positive tolerance in the covered range has the unique threshold $v=25\delta/(8-5\delta+\sqrt{64-80\delta})$. Narrower clocks beyond this branch are excluded by nesting of the allowed support intervals. At $\delta=0.01$ the centered mass is approximately $0.334022621$. The script independently diagonalizes the three-by-three output matrices by their characteristic cubic for a grid of input populations and at the predicted maximizing input. [Numerical check](../../explore/spectral_randomization.py)

**Gaps.** The irrational example certifies a lower bound and an explicit achievable upper bound. Its exact threshold remains open in this log. The equally spaced pairwise result and the global channel result below optimize different observable errors.
