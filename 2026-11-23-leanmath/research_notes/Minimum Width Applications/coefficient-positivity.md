# Eventual positivity of all coefficients of the zeta-ordinate polynomial

**Proposition.** Assume the ordinary Riemann hypothesis. Let $N(T)$ count the positive imaginary parts $\gamma\le T$ of the nontrivial zeros of the Riemann zeta function, with multiplicity. Set $m=N(T)$. There exists an absolute $T_0$ such that, for every real $T\ge T_0$, every coefficient $p_k$ in
$$
 P_T(z)=\prod_{0<\gamma\le T}(1-2\cos(\pi\gamma/T)z+z^2)
       =\sum_{k=0}^{2m}p_kz^k
$$
is strictly positive. The endpoint convention includes the entire multiplicity at $\gamma=T$.

The consequence for squarefree counting is that the $2m+1$ grid weights $p_k/P_T(1)$ define a probability distribution. Its centered transform vanishes at $\gamma/2$ to at least the corresponding multiplicity. This appendix supplies the paper proof of the positivity input. The proof gives an existence threshold, with no explicit numerical value, independent publication or Lean formalization. [Arithmetic application](xi-approximation.md)

## 1. The logarithm and the edge coefficients

For $v>0$ write
$$
 N(v)=M(v)+R(v),\qquad M(v)=\frac{v}{2\pi}\left(\log\frac{v}{2\pi}-1\right),
 \qquad |R(v)|\le C\log(2+v).
$$
Assign $M(0)=R(0)=0$. This follows for large $v$ from the Riemann–von Mangoldt formula and for bounded $v$ by increasing $C$. In particular $\sum_{\gamma>0}\gamma^{-2}<\infty$. These estimates hold unconditionally. [Hasanalizade, Shen and Wong](https://arxiv.org/abs/2107.06506)

Use the analytic logarithm $L_T=\log P_T$ on $|z|<1$ with $L_T(0)=0$. Define $D=z\,d/dz$. Directly expanding the root factors yields
$$
 [z^n]L_T=-\frac{2}{n}\int_{(0,T]}\cos(n\pi v/T)\,dN(v)
       =T A_n+[z^n]E_T,\qquad
 A_n=\frac{\operatorname{Si}(n\pi)}{\pi^2n^2}
 \quad(n\ge1).
$$
Here $\operatorname{Si}(x)=\int_0^x \sin(t)\,dt/t$. To compute the $dM$ integral, put $v=Ty$, discard the constant times $\int_0^1\cos(n\pi y)\,dy=0$, and integrate $\int_0^1\log y\,\cos(n\pi y)\,dy$ by parts. The boundary at zero vanishes since $y\log y\to0$. Integrating the $dR$ part by parts gives an endpoint $O(\log T/n)$ and an integral bounded by $C T^{-1}\int_0^T\log(2+v)\,dv=O(\log T)$, uniformly in $n$. In particular,
$$
 L_T(z)=T f(z)+E_T(z),\qquad f(z)=\sum_{n\ge1} A_nz^n,\qquad
 |[z^n]E_T|\ll\log T .
$$
The alternating half-period integrals for the sine integral give absolute constants $0<c<C$ with $c\le\operatorname{Si}(n\pi)\le C$ for all $n\ge1$. Integration by parts in the tail of the Dirichlet integral gives $\operatorname{Si}(n\pi)=\pi/2+O(1/n)$. Hence $A_n\asymp n^{-2}$. For $1\le n\le c_1\sqrt{T/\log T}$, for a sufficiently small absolute $c_1>0$, the coefficient $[z^n]L_T$ is positive. Exponentiation then proves $p_k>0$ throughout that range, since the coefficient of degree $k$ depends only on the first $k$ coefficients of $L_T$. Also $p_0=1$.

## 2. Uniform estimates for circles approaching the unit circle

Write $r=e^{-s}$, $0<s\le s_0=1/4$, and $d(u)=\operatorname{dist}(u,2\pi\mathbb Z)$. Consider $G_s(u)=\log(1-e^{-s+iu})$ and $H_s(u)=\Re G_s(u)$. Angular differentiation of this logarithm corresponds to $iD$. For $j\ge1$ differentiation of the rational expressions for its derivatives gives
$$
 |\partial_u D^jG_s(u)|\ll_j(s+d(u))^{-j-1},\qquad
 |H_s'(u)|\ll(s+d(u))^{-1},\qquad
 |H_s''(u)|\ll(s+d(u))^{-2}.
$$
Indeed $|1-re^{iu}|^2=(1-r)^2+4r\sin^2(u/2)\asymp(s+d(u))^2$. Constants in this section are absolute except for their indicated dependence on $j$.

For any $2\pi$-periodic differentiable kernel $B$, apply Stieltjes integration by parts to the two evaluations at $\theta\pm\pi v/T$. With $\mathcal R(u)=R(Tu/\pi)$ this gives the exact error formula
$$
 \begin{aligned}
 &\int_{(0,T]}\!\big[B(\theta+\pi v/T)+B(\theta-\pi v/T)\big]\,dR(v)\\
 &\quad=R(T)\big[B(\theta+\pi)+B(\theta-\pi)\big]
 -\int_0^\pi\!\mathcal R(u)\big[B'(\theta+u)-B'(\theta-u)\big]\,du .
 \end{aligned}
$$
Thus the boundary at $\pm\pi$ remains in the estimates even if $T$ is a zero ordinate.

**Local derivatives.** For $1/T\le s\le s_0$, $|\theta|\le s$, and $j=1,2,3$, we have
$$
 |D^j E_T(re^{i\theta})|\ll_j s^{-j}\log(2+Ts). \tag{1}
$$
Use the previous formula with $B=D^jG_s$. Its endpoint is $O_j(\log T)$, because $\theta\pm\pi$ is bounded away from $2\pi\mathbb Z$. This is $O_j(s^{-j}\log(2+Ts))$, as $\log(2+T)\le\log(2+Ts)+\log(1/s)$. In the integral bound, the circle triangle inequality gives $|u|\le d(u+\theta)+s$ for $-\pi\le u\le\pi$. Folding the circle onto $0\le d(u+\theta)\le\pi$ bounds the integral by a constant times
$$
 \int_0^\pi\frac{\log(2+T(v+s))}{(s+v)^{j+1}}\,dv
 \ll_j s^{-j}\log(2+Ts).
$$
For the last estimate substitute $v=sx$ and use integrability of $(1+x)^{-1-j}\log(2+x)$.

**Angular difference on the outer arc.** For $1/T\le s\le s_0$ and $s\le\theta\le\pi$,
$$
 |\Re(E_T(re^{i\theta})-E_T(r))|
 \ll \log(2+T\theta)\big(1+\log(1+\theta/s)\big). \tag{2}
$$
By symmetry this also applies to $|\theta|$. Subtract the integration-by-parts formulas with $B=H_s$ at angles $\theta$ and zero. On the part $|u|\le2\theta$, the bound for $\mathcal R(|u|)$ is $O(\log(2+T\theta))$. The absolute integrals of both $H_s'(u+\theta)$ and $H_s'(u)$ on this arc are $O(1+\log(1+\theta/s))$. To see this, split the translated arc at multiples of $\pi$, then integrate $(s+v)^{-1}$ over intervals of length at most $4\theta$ in the distance variable. There are at most a fixed number of intervals. On $|u|>2\theta$, all points $u+\lambda\theta$, $0\le\lambda\le1$, have circle distance at least $|u|/2$. The change of the derivative is $O(\theta/u^2)$. Its weighted contribution is
$$
 \ll\theta\int_{2\theta}^\pi\frac{\log(2+Tu)}{u^2}\,du
 \ll\log(2+T\theta),
$$
where the integral is omitted if $2\theta\ge\pi$. At the endpoints, the difference $H_s(\pi+\theta)-H_s(\pi)$ is $O(\theta)$ for $\theta\le\pi/4$. Then $\theta\log(2+T)\ll\log(2+T\theta)$ by concavity. For $\pi/4<\theta\le\pi$ the endpoint is $O(1+\log(1/s))$ before multiplication by $R(T)$. Both contributions satisfy (2).

**The modulus drop.** The decrease of the real part of $Tf$ around the circle is
$$
 T\sum_{n\ge1}A_ne^{-ns}(1-\cos n\theta).
$$
For $|\theta|\le s$ this is $\gg T\theta^2/s$, by retaining $n\le1/s$ and using $1-\cos(n\theta)\gg n^2\theta^2$. For $s\le|\theta|\le1$ retain $n\le1/|\theta|$ to obtain $\gg T|\theta|$. On $1\le|\theta|\le\pi$ the first term alone gives the same bound. The real part of $E_T(re^{i\theta})$ is even, so its change on the inner arc is $O(\theta^2s^{-2}\log(2+Ts))$ by (1). On the outer arc (2) gives $O(\log^2(2+T|\theta|))$ because $s\ge1/T$.

There is therefore an absolute $K_0$ such that whenever $K_0/T\le s\le s_0$,
$$
 \frac{|P_T(re^{i\theta})|}{P_T(r)}
 \le\begin{cases}
 \exp(-cT\theta^2/s),&|\theta|\le s,\\
 \exp(-cT|\theta|),&s\le|\theta|\le\pi.
 \end{cases} \tag{3}
$$
Indeed $\log(2+y)/y$ and $\log^2(2+y)/y$ tend to zero. The denominator is positive because every quadratic factor at real $0<r<1$ is positive. Also, uniformly in the same range, $b(r)=D^2L_T(r)\asymp T/s>0$ and
$$
 \sup_{|\theta|\le s}|D^3L_T(re^{i\theta})|\ll T/s^2.
$$
For the curvature use $T\sum n^2A_ne^{-ns}\asymp T/s$ and (1) at $j=2$. The third derivative follows from $T\sum n^3A_ne^{-ns}\ll T/s^2$ and (1) at $j=3$. Increasing $K_0$ gives constants independent of any later choice $K\ge K_0$.

## 3. Which coefficients does the saddle integral cover?

On the remaining radii $0<r\le e^{-s_0}$ write $a(r)=DL_T(r)$. Absolute convergence and the coefficient bounds of section 1 yield $a(r)\asymp Tr$, $b(r)\asymp Tr>0$ and $\sup_{|\theta|\le\pi}|D^3L_T(re^{i\theta})|\ll Tr$ for sufficiently large $T$. For example the error in each derivative is bounded by $C\log T\sum n^j r^n\ll_j r\log T$, uniformly here. The main modulus drop is at least $cTr(1-\cos\theta)$ from the $n=1$ term. Since $1-\cos(n\theta)\le n^2(1-\cos\theta)$, the error in this drop is $O(r\log T(1-\cos\theta))$. The modulus ratio is therefore at most $\exp(-cTr\theta^2)$ on the whole circle.

Fix $K\ge K_0$, enlarged below. Since $a(0)=0$ and $a'(r)=b(r)/r>0$ throughout $0<r\le e^{-K/T}$, every integer $0<k\le a(e^{-K/T})$ has a unique saddle radius $a(r)=k$. Cauchy's formula expresses $p_k$ as the positive number $P_T(r)r^{-k}/(2\pi)$ times
$$
 I=\int_{-\pi}^{\pi}\frac{P_T(re^{i\theta})}{P_T(r)}e^{-ik\theta}\,d\theta.
$$
This integral is real. For $K/T\le s\le s_0$, Taylor's formula on $|\theta|\le s$ gives
$$
 L_T(re^{i\theta})-L_T(r)-ik\theta
     =-b(r)\theta^2/2+O(T|\theta|^3/s^2).
$$
Put $b=b(r)$. Choose an absolute $A\ge1$ for the arc $|\theta|\le A/\sqrt b$. Then choose $K$ sufficiently large in terms of $A$. Since $b\asymp T/s$, the arc is inside $|\theta|\le s$ and the complex Taylor error is at most $CA^3/\sqrt{Ts}\le 1/10$. Its contribution to $\Re I$ is at least $c_*/\sqrt b$ for an absolute $c_*>0$, by integrating the Gaussian on $|\theta|\le1/\sqrt b$ and using positivity of the real part on the entire arc. By (3), the absolute contribution on $A/\sqrt b<|\theta|\le s$ is at most
$$
 \frac{C}{\sqrt b}\int_{cA}^{\infty}e^{-v^2}\,dv .
$$
On $s<|\theta|\le\pi$ it is at most $Ce^{-cTs}/T$, which is at most $(C/\sqrt b)e^{-cTs}/\sqrt{Ts}$. Choose $A$ first to make the Gaussian tail less than $c_*/(4\sqrt b)$, then $K$ to control the Taylor error and the outer integral. Thus $I>0$ simultaneously at every saddle in this range.

When $r\le e^{-s_0}$, the same arc argument uses $b\asymp Tr$, a cubic error $O(Tr|\theta|^3)$ and the full-circle Gaussian bound proved above. For $k\ge T^{1/4}$, the saddle equation gives $Tr\asymp k\to\infty$ uniformly. Thus the Taylor error on $|\theta|\le A/\sqrt b$ is $O(A^3/\sqrt{Tr})$, tending to zero. Lower degrees have already been covered directly in section 1.

Finally,
$$
 TDf(e^{-s})=\frac{T}{2\pi}\log(1/s)+O(T),\qquad 0<s\le s_0,
$$
by $nA_n=1/(2\pi n)+O(n^{-2})$ and $\sum e^{-ns}/n=-\log(1-e^{-s})$. At $s=K/T$, (1) with $j=1$ adds an error $O_K(T)$. The zero count gives $a(e^{-K/T})=T\log T/(2\pi)+O_K(T)=m+O_K(T)$. Choose a fixed $C_0$ large enough that $a(e^{-K/T})\ge m-C_0T$ for large $T$. We have proved $p_k>0$ through $k\le m-C_0T$. Reciprocal symmetry gives $p_{2m-k}=p_k$ and covers the other edge. This step used only the zero count.

## 4. The central band by Fourier inversion

Let $h=2\pi/T$. Define the following function on one fundamental interval, extending by zero outside $[-T/2,T/2]$ solely for the convergence argument:
$$
 Q_T(u)=e^{-imhu}\frac{P_T(e^{ihu})}{P_T(1)}
 =\prod_{0<\gamma\le T}\left(1-\frac{\sin^2(\pi u/T)}{\sin^2(\pi\gamma/(2T))}\right).
$$
For each fixed $u$, the product tends to $\prod_{\gamma>0}(1-4u^2/\gamma^2)$. Indeed $\sin(\pi\gamma/(2T))\ge\gamma/T$ for $\gamma\le T$, so the sum of absolute deviations from one in the tail $\gamma>G$ is at most $\pi^2u^2\sum_{\gamma>G}\gamma^{-2}$ uniformly in $T$. The finite initial factors converge individually. Under RH the limit is $F(u)=\Xi(2u)/\Xi(0)$, by the paired Hadamard product. Here $\Xi(z)=\xi(1/2+iz)$ is even and entire of order one, and $\Xi(0)>0$. Pairing positive and negative zeros leaves an exponential of degree at most one, whose linear coefficient vanishes by evenness and whose constant is fixed by $F(0)=1$. This identification is where RH enters. [Lagarias and Montague, §1](https://arxiv.org/pdf/1106.4348)

To obtain an integrable majorant, use $|1-e^{iv}|^2\le r^{-1}|1-re^{iv}|^2$ in each of the $2m$ linear factors. For $r=e^{-K/T}$ this gives $|P_T(e^{i\theta})|\le r^{-m}|P_T(re^{i\theta})|$, and
$$
 \frac{r^{-m}P_T(r)}{P_T(1)}
  =\prod_{\gamma\le T}
  \left(1+\frac{\sinh^2(K/(2T))}{\sin^2(\pi\gamma/(2T))}\right)
  \le \exp\left(C K^2\sum_{\gamma>0}\gamma^{-2}\right).
$$
Here and below the product uses positive ordinates. With fixed $K$ and sufficiently large $T$ the constant is independent of $T$. Applying (3) at $\theta=hu$ bounds $|Q_T(u)|$ by $C_K e^{-c'|u|}$ when $|u|\ge K/(2\pi)$ in the fundamental interval, and by $C_K$ on its remaining compact part. The extended functions therefore converge to $F$ in $L^1(\mathbb R)$.

In the transform convention $\widehat\phi(u)=\int e^{iut}\phi(t)\,dt$, the inverse transform of $F$ is $\phi(t)=\Phi(t/2)/(2\Xi(0))$. Riemann's kernel $\Phi$ is even. For $v\ge0$ it has the locally uniformly convergent series
$$
 \Phi(v)=\sum_{n\ge1}2\pi n^2e^{5v/2}
     (2\pi n^2e^{2v}-3)e^{-\pi n^2 e^{2v}} .
$$
Every summand is positive. Thus $\phi$ is continuous and strictly positive on the whole real line. [Lagarias and Montague, (1.1), (1.2) and Lemma 3.1](https://arxiv.org/pdf/1106.4348)

For every integer $|j|\le m$, Fourier coefficient inversion and the substitution $\theta=hu$ give
$$
 \frac{p_{m+j}}{hP_T(1)}=\frac1{2\pi}\int_{\mathbb R}Q_T(u)e^{-ihju}\,du,\qquad
 \left|\frac{p_{m+j}}{hP_T(1)}-\phi(hj)\right|
 \le\frac1{2\pi}\|Q_T-F\|_1 .
$$
The bound tends to zero uniformly in $j$. For $|j|\le C_0T$, $hj\in[-2\pi C_0,2\pi C_0]$. On this fixed compact interval the minimum of $\phi$ is a strictly positive number. Taking $T$ large enough makes the right-hand side smaller than half that minimum. This proves positivity on the whole central band and completes the proof.

The constants are chosen in the order $s_0$, the absolute constants in (1)–(3), $A$, $K$, $C_0$, and finally $T_0$. None depends on the coefficient index or on the running height. Fourier inversion used identities for the signed coefficients throughout, so its use of the positive limiting density assumes no positivity of the finite coefficients.

**Relation to the saddle-point literature.** Hayman established classical sufficient conditions for asymptotic coefficient formulae. De Angelis treats large powers of a fixed function. Here the polynomial and its zero set both change with $T$. Sections 2–3 supply the required uniform estimates for this family. [Hayman (1956)](https://doi.org/10.1515/crll.1956.196.67) [De Angelis](https://vdeangel.xula.edu/Publications/asy.pdf)
