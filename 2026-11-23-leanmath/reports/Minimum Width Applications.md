# The shortest clock for robust dephasing

**One random forward wait suffices to dephase an equally spaced quantum spectrum, and its shortest possible worst-case duration is exactly known even when the clock must tolerate first-order drift.** For $d\ge2$ levels separated by $\Delta$, the optimum is $4\pi(d-1)/(d\Delta)$. No positive timing law with fewer restrictions on its support can finish sooner. This unconditional result gives a direct application of the positive Fourier averages studied in the zeta project. An arithmetic application uses ordinary RH: a finite positive average over dilations exposes a secondary term of order $1/x$ in a weighted squarefree count. The general results below are paper proofs. The control application additionally supplies an executable interval certificate; none of the three results has been verified in Lean. [Earlier minimum-width report](ゼータ零点%20最小幅の精密化.md) [Clock proof](../research_notes/Minimum%20Width%20Applications/spectral-randomization.md) [Squarefree proof](../research_notes/Minimum%20Width%20Applications/xi-approximation.md)

## Two uniform draws attain the exact latency

Take energies $0,\Delta,\ldots,(d-1)\Delta$ with $\hbar=1$. Draw independent integers $J_1,J_2$ uniformly from $\{0,\ldots,d-1\}$ and evolve once for time $2\pi(J_1+J_2)/(d\Delta)$. Averaging over this classical randomness erases every off-diagonal entry in the energy basis. With a common fractional calibration error $\eta$ in the generator, the error of the averaged channel in diamond norm is $O(\eta^2)$ for fixed $d$. The timing law is the unique minimizer up to translation among all compactly supported probability laws, including continuous and asymmetric ones. The resource is the maximum forward evolution time for a single passive dephasing step. [Boixo, Knill and Somma on random-time dephasing](https://arxiv.org/abs/0903.1652) [Proof and error bounds](../research_notes/Minimum%20Width%20Applications/spectral-randomization.md)

**Proof.** Scale time by $\Delta$. The clock's characteristic function $F$ multiplies the coherence at gap $j$ by $F(j(1+\eta))$, so the two requirements give $F(j)=F'(j)=0$ for $1\le j<d$. Write $h=2\pi/d$ and $a=(d-1)h$. For $d\ge3$ define
$$
 A(t)=\frac{1-\cos(dt)}{\cos t-\cos h},\qquad
 B(t)=A(t)\cot(t/2),\qquad S(t)=-A(t)+\frac{\tan(h/2)}h\,tB(t).
$$
The Chebyshev identity gives $A$ harmonic degree $d-1$; its mean is zero since it vanishes at all $d$ equally spaced nodes. Also $B=\sin(t)A/(1-\cos t)$ is a sine polynomial of degree at most $d-1$, so every feasible law integrates $S$ to zero. On $[-a,a]$, strict decrease of $t\cot(t/2)$ for $0<t<2\pi$ makes $S$ positive between consecutive grid nodes, with zeros at the nodes; for $d=2$, use $t\sin t$ on $[-\pi,\pi]$. Consequently any law fitting in this interval is concentrated on the grid. After translation, its weight polynomial must have a double zero at every nontrivial $d$th root of unity and degree at most $2(d-1)$, forcing $W(z)=(1+\cdots+z^{d-1})^2/d^2$. Both endpoint masses are positive, proving the claimed minimum as well as uniqueness. [Full sign and degree checks](../research_notes/Minimum%20Width%20Applications/spectral-randomization.md)

For eight levels the joint clock has $\Delta L=10.995574288$. Combining seven individually optimized robust clocks takes **1.481633 times as long**. At $+1\%$ drift the largest remaining fraction of a coherence is 0.005829254; the diamond-norm error is at most 0.012091. Triangular weights are classical. Nominal harmonic dephasing and five-impulse, two-mode robust shapers have precedents. The specific contribution examined here is the sharp all-$d$ bound and uniqueness over arbitrary positive timing laws under the double-zero conditions. Its publication priority remains unestablished. [Kozma–Oravecz, Theorem 2](https://arxiv.org/pdf/math/0601021) [Singhose, Crain and Seering on multiple-mode shaping](https://doi.org/10.1049/ip-cta:19971439) [Minimum-support approximation kernels](https://www.ee.cuhk.edu.hk/~tblu/monsite/pdfs/blu0101.pdf)

## A finite average reveals the squarefree secondary term

At each sufficiently large $x$, put $T=16\sqrt{\log x}$, $m=N(T)$, $h=2\pi/T$, where $N(T)$ counts positive nontrivial zeta-zero ordinates $\gamma\le T$ with multiplicity. Construct
$$
 P_T(z)=\prod_{0<\gamma\le T}(1-2\cos(\pi\gamma/T)z+z^2)=\sum_{k=0}^{2m}p_kz^k .
$$
Put probability $p_k/P_T(1)$ at $t=(k-m)h$ and call the resulting law $\nu_T$. Positivity for every sufficiently large $T$ uses the project's earlier paper proof under ordinary RH, whose threshold has not been made explicit. Define $M_T(r)=\int e^{rt}d\nu_T(t)$ and $w(y)=e^{-(\log y)^2/2}/\sqrt{2\pi e}$. The observable uses the unit-integral weight $w_T(y)=\int e^{-t/4} w(ye^{-t})d\nu_T(t)/M_T(3/4)$. **The weight changes with $x$.** Its dilation parameters take only $O(\sqrt{\log x}\log\log x)$ values across a scale ratio $O(\log x)$; each Gaussian has infinite support. [Coefficient-positivity proof](../research_notes/Minimum%20Width%20Applications/coefficient-positivity.md) [Arithmetic construction](../research_notes/Minimum%20Width%20Applications/xi-approximation.md)

For the squarefree indicator $\mu^2$, the result is
$$
 \sum_{n\ge1}\mu^2(n)w_T(n/x)=\frac{6x}{\pi^2}+\frac{C_T}{x}+O(x^{-3/2}),
 \qquad C_T=\frac{\pi^2}{6\zeta(3)}\frac{M_T(5/4)}{M_T(3/4)} .
$$
The finite filter cancels the prescribed poles of $\zeta(s)/\zeta(2s)$ with their multiplicities. Gaussian decay controls the rest of the contour, and the pole at $s=-1$ gives the positive correction. In fact $C_T=(3/2)(1-\kappa/T+O(\log T/T^2))$ with $\kappa\approx1.1938635536$. Keeping $C_T$ gives the displayed $O(x^{-3/2})$ remainder. **Substituting $3/2$ leaves a leading discrepancy $-3\kappa/(2xT)$**, of larger order. A known positive continuous distribution associated with $\xi$ yields the limiting coefficient unconditionally when all dilations are allowed. The finite construction quantifies the cost of using finitely many. [Contour proof and coefficient expansion](../research_notes/Minimum%20Width%20Applications/xi-approximation.md) [Squarefree Dirichlet series](https://arxiv.org/pdf/1912.04972) [Nakamura on the completed-zeta distribution](https://arxiv.org/abs/1504.03438)

## Three-mode control delivers verifiable command timings

For three undamped modes at angular frequencies $\omega_1(1,a,b)$, the interval verifier certifies a unique shortest positive shaper for **each** $(a,b)\in[1.40,1.55]\times[1.68,1.85]$. It constructs four impulses, then proves a lower bound over every positive measure. Its 1,264 closed parameter boxes cover the entire rectangle. The first comparison below uses convolution of three individual zero-vibration shapers; the second uses the best of three optimal pair-plus-single cascades. The filter is redesigned for each known triplet. [Control theorem and verifier](../research_notes/Minimum%20Width%20Applications/three-mode-control.md)

| Nominal modes | Optimal $\omega_1 D$ | Delay saved vs. three singles | Delay saved vs. best pair + single |
|---|---:|---:|---:|
| $1:1.5:1.8$ | 6.575712456 | 5.80986% | 0.10136% |
| Silicon nitride membrane, 517, 747, 927 kHz | 6.671833475 | 5.60509% | 0.02467% |

For the membrane's three measured frequencies, the nominal command times are **0, 0.682107480, 1.371771199, and 2.053878679 µs**. The stronger baseline is only about 0.507 ns slower. This is an undamped model calculation on published frequencies. The experiment that measured the modes did not test these commands. Simultaneous four-impulse engineering designs predate this work. [Yang et al., Supplementary Table I](https://arxiv.org/pdf/1902.01270) [Earlier three-mode control design](https://doi.org/10.1109/TMECH.2005.856217)

## Reproduce the checks

Run these commands from the experiment directory with standard Python 3, with assertions enabled. The research notes and the reproduction scripts are in English.

```sh
python3 -B explore/spectral_randomization.py
python3 -B 'research_notes/Minimum Width Applications/verify_xi_count.py'
python3 -B explore/three_mode_shaper.py --hz 517000 747000 927000
python3 -B explore/three_mode_shaper.py
```

The first command checks both root conditions by exact integer arithmetic for sample ladder sizes and prints the eight-level values above. The second directly sums over squarefree integers and reports, at $x=192$, $S(x)=116.7295541085$, $C_T=1.45150894$, and a residual of $-9.390\times10^{-6}$ after both terms. It is a floating-point normalization check. The third prints the four command times and weights; the last validates the continuum of plants and reports a **433-basis-point minimum saving bound** against three singles. The analytic proofs carry the unbounded-$d$ and large-$x$ statements. [Clock script](../explore/spectral_randomization.py) [Arithmetic script](../research_notes/Minimum%20Width%20Applications/verify_xi_count.py) [Control script](../explore/three_mode_shaper.py)
