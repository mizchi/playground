# A positive finite dilation average for the squarefree counting measure

Assume the ordinary Riemann hypothesis. At scale $x$ one can choose $O(\sqrt{\log x}\,\log\log x)$ dilations of a single log-Gaussian so that the weighted count of squarefree integers equals its density term plus an explicit term of order $1/x$, with an absolute remainder $O(x^{-3/2})$. The coefficient of $1/x$ tends to $3/2$. Its first correction has a sign and an explicit constant. The exact moment formula and its error use the classical zero count, the functional equation and RH. The assertion that all dilation weights are positive also uses a research lemma from this project. These are paper proofs. No Lean verification or claim of priority is attached.

## 1. Which arithmetic observable can we estimate?

Write $\xi(s)=s(s-1)\pi^{-s/2}\Gamma(s/2)\zeta(s)/2$, $\Xi(v)=\xi(1/2+iv)$ and $F(u)=\Xi(2u)/\Xi(0)$. Use the Fourier transform $\widehat\nu(u)=\int e^{iut}\,d\nu(t)$. Let $N(T)$ count positive nontrivial zero ordinates $\gamma\le T$, with multiplicity. Put $m=N(T)$, $h=2\pi/T$, and form
$$
 P_T(z)=\prod_{0<\gamma\le T}(1-2\cos(\pi\gamma/T)z+z^2)=\sum_{k=0}^{2m}p_k z^k,
 \qquad \nu_T=\sum_{j=-m}^m \frac{p_{m+j}}{P_T(1)}\delta_{jh}.
$$
Its total mass is one, it is symmetric, and its support has diameter $4\pi N(T)/T$. Write $Q_T(u)=\widehat\nu_T(u)$, $M_T(a)=Q_T(-ia)$. Take
$$
 w(y)=\frac{e^{-(\log y)^2/2}}{\sqrt{2\pi e}},\qquad
 w_T(y)=\frac{1}{M_T(3/4)}\int e^{-t/4} w(ye^{-t})\,d\nu_T(t)\quad(y>0).
$$
The factors in the denominator are strictly positive even before proving coefficient positivity, as the sinh product in section 2 shows. The weight has integral one, since the change of variables $y=e^t v$ contributes $e^t$ and $\int w(v)dv=1$. The underlying Gaussian has infinite support. The dilation *parameters* form a finite set with largest-to-smallest scale ratio $e^{4\pi N(T)/T}=O(T^2)=O(\log x)$ at the heights below.

**Theorem.** For every sufficiently large real $x$, choose $T=16\sqrt{\log x}$. Then
$$
 \boxed{\displaystyle
 S(x):=\sum_{n\ge1}\mu^2(n)w_T(n/x)
 =\frac{6x}{\pi^2}
 +\frac{\pi^2}{6\zeta(3)x}\frac{M_T(5/4)}{M_T(3/4)}
 +O(x^{-3/2}).}
$$
The implied constant is independent of $x$ and its associated $T$. No hypothesis about simple zeros, zero separation or negative moments of $\zeta'$ occurs. Section 2 evaluates the coefficient and yields
$$
 S(x)=\frac{6x}{\pi^2}
 +\frac{3}{2x}\left(1-\frac{\kappa}{T}
     +O\!\left(\frac{\log T}{T^2}\right)\right)+O(x^{-3/2}),
 \qquad \kappa=1.1938635536\ldots .
$$
The first display gives the smaller error because it retains the directly computable finite moment ratio.

**Proof.** The Euler product gives $\sum\mu^2(n)n^{-s}=\zeta(s)/\zeta(2s)$ on $\Re s>1$. The Mellin transform of $w$ is $e^{(s^2-1)/2}$, so inversion and absolute convergence give $S(x)$ as $1/(2\pi i)$ times the integral on $\Re s=2$ of
$$
 x^s e^{(s^2-1)/2}\frac{\zeta(s)}{\zeta(2s)}
 \frac{Q_T((s-1/4)/i)}{M_T(3/4)}.
$$
This Dirichlet series and the role of its critical poles in the ordinary squarefree count are classical. [Mossinghoff, Oliveira e Silva and Trudgian, §1](https://arxiv.org/pdf/1912.04972)

Take $Y=T/4$. On the initial line, $|\zeta(s)/\zeta(2s)|\ll1$ and $M_T(3/4)\ge1$. In the sine product in section 2, each factor on $u=t-7i/4$ has absolute value at most $1+CT^2$, by $\sin(\pi\gamma/(2T))\ge\gamma/T$ and the positive first ordinate. Hence $|Q_T(t-7i/4)|\le\exp(O(T\log^2T))$. The Gaussian bound $e^{-t^2/2}$ shows that the two omitted tails total $O(x^2e^{-T^2/64})$ for large $T$. This deliberately rough bound works for signed coefficients as well.

On the rectangle $-3/2\le\Re s\le2,\ |\Im s|\le Y$, put $u=(s-1/4)/i$. Here $|u|\le T/3$ for large $T$. The quotient $R_T=Q_T/F$ is analytic and has neither a pole nor a zero in this disk under RH, by section 2. As a meromorphic identity,
$$
 \frac{\zeta(s)}{\zeta(2s)}Q_T(u)
 = R_T(u)\frac{s(2s-1)\pi^{-s}\Gamma(s)\zeta(s)}{\Xi(0)} .
$$
This removes every nontrivial denominator zero in the rectangle with its multiplicity. For $|\Im s|\ge2$, Stirling's formula and the polynomial growth of $\zeta$ on fixed vertical strips bound the last factor by a polynomial times $e^{-\pi|\Im s|/2}$. On the new vertical line the bounded central part stays away from poles. Section 2 gives $|R_T(u)|\le\exp(C(1+|\Im s|^2)\log T/T)$, so, after multiplication by the Gaussian, the integrand is $O(x^{\Re s} e^{-|\Im s|^2/4})$ on the three new sides. The new vertical integral is $O(x^{-3/2})$, and the horizontals are $O(x^2e^{-T^2/64})=O(x^{-2})$.

The only residues crossed occur at $1,-1$. The factor $s\Gamma(s)$ is regular at zero. At $1$, normalization gives $x/\zeta(2)$. At $-1$ the Mellin Gaussian equals one, the moment is $M_T(-5/4)=M_T(5/4)$, and the functional equation gives $\zeta(-1)/(2\zeta'(-2))=\pi^2/(6\zeta(3))$. This proves the first formula. The second follows from the moment calculation below.

**A comparator that explains the role of finitely many dilations.** Take the continuous probability measure with transform $F(u)$ in place of $\nu_T$. Its strictly positive density decays faster than exponentially in both tails. Its moments are $\xi(1/2+2a)/\Xi(0)$, so its ratio is $\xi(3)/\xi(2)=9\zeta(3)/\pi^2$. The same contour shift now works on the entire strip using the gamma identity with $R=1$ and gives $6x/\pi^2+3/(2x)+O(x^{-3/2})$ unconditionally. The finite construction replaces this unbounded continuum by $2N(T)+1$ translates and quantifies the change in the secondary term. Both formulas concern the specified weighted sums. [Lagarias and Montague, (1.1)–(1.2), Lemma 3.1](https://arxiv.org/pdf/1106.4348) [Nakamura, the completed Riemann zeta distribution](https://arxiv.org/abs/1504.03438)

A fixed compactly supported probability measure cannot give the same conclusion under RH. Its transform is an entire function of exponential type whose zeros up to radius $Y$, counted with multiplicities, number $O(Y)$ by Jensen's formula and the normalization at the origin. The denominator critical zeros number $\asymp Y\log Y$. Their orders must be matched, since RH puts every $\rho/2$ on $\Re s=1/4$, where $\zeta(s)\ne0$. Thus at least one pole remains. In fact a globally $o(x^{1/4})$ error after the density term would contradict this pole. For fixed compact dilations the part of the Mellin transform integrated over $0<x<1$ is entire by Gaussian decay. On $x\ge1$, if the discrepancy were $o(x^{1/4})$, its Mellin transform $H$ would satisfy $vH(\rho/2+v)\to0$ as $v\downarrow0$, by bounding its tail by $\epsilon v\int_1^\infty x^{-1-v}dx$. A meromorphic pole cannot satisfy this. The moving cutoff is mathematically material.

## 2. How accurately does the finite product approximate the completed zeta distribution?

On $|u|<T/2$, continue $R_T(u)=Q_T(u)/F(u)$ at any common zero and normalize its analytic logarithm by $\log R_T(0)=0$. Then uniformly on $|u|\le T/3$,
$$
 |\log R_T(u)|\ll |u|^2\log T/T,\qquad
 \log R_T(u)=b_Tu^2+O(|u|^4\log T/T^3),\quad
 b_T=\kappa/T+O(\log T/T^2).
$$
In particular, uniformly for $a$ in any fixed real compact interval,
$$
 M_T(a)=\frac{\xi(1/2+2a)}{\Xi(0)}
 \exp\!\left(-\frac{\kappa a^2}{T}+O(\log T/T^2)\right).
$$
Taking $a=5/4,3/4$ subtracts squared arguments with difference one. This gives the first correction to the arithmetic residue. Taking two derivatives at zero shows that the centered lattice law has variance smaller than that of the limiting law by $2\kappa/T+O(\log T/T^2)$.

**Exact comparison.** Centering the polynomial gives
$$
 Q_T(u)=\prod_{0<\gamma\le T}
 \left(1-\frac{\sin^2(\pi u/T)}{\sin^2(\pi\gamma/(2T))}\right).
$$
At $u=-ia$ each factor is $1+\sinh^2(\pi a/T)/\sin^2(\pi\gamma/(2T))\ge1$. The positive zeros of an individual factor are $g=\gamma/2$ and $jT-g,jT+g$ for $j\ge1$, listed with multiplicity, also when $g=T/2$. An even entire function of order one with these paired zeros and value one at zero has the convergent product of $1-u^2/\lambda^2$ over the positive zeros. Under RH the same Hadamard representation applies to $F$ with $g=\gamma/2$ for all $\gamma>0$. On removing common factors we get
$$
 \log R_T(u)
 =\sum_{k\ge1}\frac{u^{2k}}{k}
 \left[\sum_{\gamma>T}(\gamma/2)^{-2k}
 -\sum_{\gamma\le T}\sum_{j\ge1}\{(jT-\gamma/2)^{-2k}+(jT+\gamma/2)^{-2k}\}\right].
$$
All remaining ordinates are at least $T/2$. The sum of *absolute* tail and image contributions at $k=1,2$ is respectively $O(\log T/T)$, $O(\log T/T^3)$: use partial summation with $N(v)\ll v\log(2+v)$ for the tail, and $jT-g\ge (j-1/2)T$ for the images. Summing the geometric majorant at $|u|\le T/3$ gives the two uniform estimates above. This comparison also explains why no small separation between two actual zeros enters the argument.

**The first correction.** The partial-fraction expansion of $\csc^2$ gives the exact quadratic coefficient
$$
 b_T=T^{-2}\int_0^\infty G(v/T)\,dN(v),\qquad
 G(y)=
 \begin{cases}
 4/y^2-\pi^2\csc^2(\pi y/2),&0<y\le1,\\
 4/y^2,&y>1.
 \end{cases}
$$
The standard Riemann–von Mangoldt formula has error $O(\log(2+v))$ about $v(\log(v/(2\pi))-1)/(2\pi)$, with the main term assigned value zero at zero. Partial summation on either side of the jump at one contributes $O(\log T/T^2)$ in total, including the endpoint if it is a zero ordinate. Indeed $G,G'$ are bounded near zero and $G'=-8/y^3$ for $y>1$. [Hasanalizade, Shen and Wong, explicit zero count](https://arxiv.org/abs/2107.06506)

The coefficient of $\log(T/(2\pi))$ vanishes because $\int_0^\infty G=0$. On $(0,1)$ a primitive is $2\pi\cot(\pi y/2)-4/y$, which tends to zero at the origin and to $-4$ at one. The tail integral is $4$. Thus $\kappa=(2\pi)^{-1}\int_0^\infty G(y)\log y\,dy$. Integrating by parts on $(0,1)$, then expanding the cotangent in partial fractions and integrating the positive series, yields
$$
 \kappa=\frac{4+\int_0^1(4/y^2-2\pi\cot(\pi y/2)/y)\,dy}{2\pi}
 =\frac{4+2\sum_{j\ge1}j^{-1}\log((2j+1)/(2j-1))}{2\pi}>0 .
$$
This cancellation removes the $\log T$ factor in the fixed-frequency error.

**Status of positivity.** The earlier project note proves on paper that, subject to RH, $p_k>0$ simultaneously for all $0\le k\le2N(T)$ once $T$ is sufficiently large. It gives no numerical threshold and has no independent publication or mathlib verification. Here is a route through the three steps needed for that additional input. [English proof of eventual coefficient positivity](coefficient-positivity.md)

1. For $|z|<1$, zero counting and Stieltjes integration give $\log P_T(z)=Tf(z)+E_T(z)$, where $[z^n]f=\operatorname{Si}(n\pi)/(\pi^2n^2)\asymp n^{-2}$, and $[z^n]E_T=O(\log T)$ uniformly. Exponentiation gives positive coefficients at least up to $c\sqrt{T/\log T}$.
2. A Cauchy integral at a positive saddle $r=e^{-s}$ extends the edge range to $k\le m-C_0T$, then symmetry covers the other edge. For $K/T\le s\le1/4$, the local Stieltjes error for $D^jE_T(re^{i\theta})$ is $O(s^{-j}\log(2+Ts))$ when $j=1,2,3$, $|\theta|\le s$, $D=z\,d/dz$. On $s\le|\theta|\le\pi$ the angular change in its real part is $O(\log(2+T|\theta|)(1+\log(1+|\theta|/s)))$. These follow by retaining the local zero-count error $O(\log(2+v))$ in the integration. The main drop of $T\Re f$ is at least $cT\theta^2/s$ on the inner arc and $cT|\theta|$ on the outer arc. Choosing fixed large $K$ absorbs both errors. The central Gaussian contribution is positive and dominates the two tails since its curvature is $\asymp T/s$ and cubic derivative $O(T/s^2)$. For $r\le e^{-1/4}$ the analogous curvature and cubic bounds are $\asymp Tr$ and $O(Tr)$. At $s=K/T$ the saddle degree is $m+O_K(T)$.
3. Moving the circle back to radius one from $e^{-K/T}$ bounds $|Q_T(u)|\le C_K e^{-c|u|}$ on $|u|\le T/2$. Here each root has modulus one and the normalized radial factor is bounded by $\exp(CK^2\sum\gamma^{-2})$. The pointwise limit is $F$ by the paired RH product. Fourier inversion and dominated convergence give $p_{m+j}/(hP_T(1))=\phi(hj)+o(1)$ uniformly in $j$, with $\phi$ the continuous probability density with transform $F$. Its minimum on $[-2\pi C_0,2\pi C_0]$ is positive by the Riemann kernel formula. This covers the remaining central band. [Lagarias and Montague, Lemma 3.1](https://arxiv.org/pdf/1106.4348)

If that auxiliary lemma is withheld, the moment expansion, the analytic cancellation, and **both displayed arithmetic asymptotics still follow for the defined signed average** from the proofs in sections 1–2. The positivity interpretation for all sufficiently large heights depends on that lemma. The three finite examples below check coefficient signs separately.

## 3. What do direct arithmetic data and earlier constructions establish?

Run the standard-library program from the experiment directory.

```sh
python3 'research_notes/Minimum Width Applications/verify_xi_count.py'
```

It builds the finite polynomial, checks the sign of every coefficient, sieves all multiples of squares through $N=\lceil xe^{mh+9}\rceil$, and sums positive values over the surviving integers. It evaluates the secondary coefficient independently from the finite moments and Apéry's series for $\zeta(3)$. The first ten ordinates come from the rounded leading digits of Odlyzko's table. Each experimental cutoff lies strictly between recorded ordinates. [Reproduction script](verify_xi_count.py) [Odlyzko's high-precision ordinates](https://www-users.cse.umn.edu/~odlyzko/zeta_tables/zeros2)

| $x$ | $T$ | $m$ | Sieve cutoff $N$ | Direct $S(x)$ | $x(S(x)-6x/\pi^2)$ | Predicted coefficient | $S(x)$ minus both terms | Truncation bound |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 12 | 25.22174 | 3 | 205307 | 7.3983906605 | 1.23918526 | 1.44960732 | −1.754e−2 | 7.47e−15 |
| 48 | 31.48059 | 4 | 864199 | 29.2101872406 | 1.42494488 | 1.44929554 | −5.073e−4 | 2.99e−14 |
| 192 | 36.68677 | 5 | 3663114 | 116.7295541085 | 1.44970609 | 1.45150894 | −9.390e−6 | 1.19e−13 |

For each dilation the summand decreases beyond $N$. Discarding the squarefree restriction in the tail and comparing with the integral from $N$, completing the square gives a common upper bound $x\,\overline\Phi_{\rm normal}(\log(N/(xe^{mh}))-1)\le x\,\overline\Phi_{\rm normal}(8)$. The probability weights after this change of variables are proportional to $p_{m+j}e^{3jh/4}$. This is an analytic truncation bound for exact input and arithmetic. The tabulated evaluation uses binary64 floats and has no interval roundoff certificate. The three observations check the location and normalization of the secondary term. They cannot establish an effective asymptotic threshold or the claimed error rate by themselves.

**Literature comparison.** The positive completed-zeta distribution, its characteristic function and the Fourier-kernel expression are prior results. Romik obtains polynomial expansions in orthogonal bases for $\Xi$. Griffin, Ono, Rolen, Thorner, Tripp and Wagner study effective hyperbolicity of the Jensen polynomials, which are built from Taylor coefficients. Neither cited construction specifies these logarithmic dilation weights using the actual first $N(T)$ ordinates. Searches for squarefree smoothing, completed-zeta distributions, Jensen polynomials and Fourier approximations did not establish priority for the finite-product bias constant or the arithmetic observable. These are results derived here with the above hypotheses. [Nakamura](https://arxiv.org/abs/1504.03438) [Romik](https://danromik.com/resources/papers/riemannxi-acta-online-first.pdf) [Griffin et al.](https://arxiv.org/abs/1910.01227) [Mossinghoff, Oliveira e Silva and Trudgian](https://arxiv.org/pdf/1912.04972)
