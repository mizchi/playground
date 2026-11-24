# The shortest positive command shaper for three measured modes

## Which plants have a certified optimal shaper?

### Takeaway

Consider a scalar command driving three undamped linear modes, with angular frequencies $\omega_1(1,a,b)$. For **every real pair** $1.40\le a\le1.55$, $1.68\le b\le1.85$, there is a unique minimum-duration positive, unit-gain shaper up to translation. It uses four impulses. Across the entire rectangle its normalized duration $\omega_1D$ lies between 6.4227 and 6.9315, and it saves **at least 4.33%** of the added delay compared with convolving three single-mode zero-vibration shapers. These are computer-assisted bounds over a continuum of frequency pairs. [Verifier](../../explore/three_mode_shaper.py)

### Cited Findings

- Pre-existing command shaping theory convolves a motion command with a distribution of delayed copies; the zeros of its frequency response suppress the corresponding undamped residual oscillations. Filter duration is the added command delay. Three-mode, four-impulse simultaneous shapers already appear in engineering research. [Discrete time-optimal command shaping](https://www.sciencedirect.com/science/article/abs/pii/S0005109807001264) [Fast and robust control of systems with multiple flexible modes, Fig. 5](https://doi.org/10.1109/TMECH.2005.856217)
- Kozma and Oravecz's Theorem 3 solves the dual zero-free-interval problem for symmetric integer arithmetic progressions with a restriction on their step. At the ratio $1:1.4:1.8$, their result for frequencies $5,7,9$ independently gives $15\pi/7=6.7319842576924140824\ldots$. This value falls inside the interval returned by the verifier. [Theorem 3](https://arxiv.org/pdf/math/0601021)

### Inferences

A causal positive shaper is a probability measure $\mu$ on $[0,D]$. Its response is $H(\omega)=\int e^{i\omega t}d\mu(t)$; exact cancellation requires $H(\omega_j)=0$. Optimizing over all such measures permits arbitrary impulse counts, nonuniform timing, and continuous densities. In units $u=\omega_1 t$, put $C(u)=(\cos u,\cos au,\cos bu)$. Solve $wC(x)+(1-w)C(y)=0$ for $0<x<y$, $0<w<1$. The optimal causal times in seconds and corresponding weights are
$$
(0,y-x,y+x,2y)/\omega_1,\qquad
(1-w,w,w,1-w)/2.
$$
Each plant has its own $x,y,w$. A single compactly supported unit-gain shaper cannot vanish on an entire frequency interval, since its Fourier transform is entire and equals one at zero. RH plays no role in the control result. The mathematical connection is the same positive-measure moment problem and the primal and dual certificates first tested against three zeta ordinates. [Three-frequency verifier](../../explore/three_zero_width.py) [Control verifier](../../explore/three_mode_shaper.py)

### Gaps

Positive unit gain preserves the range of a bounded input. Slew limits, signed controls, damping-ratio constraints, discrete timing, and suppression of additional modes define different optimization problems. The certified ratio box is a sufficient region; the boundary at which its four-impulse optimizer changes form has not been located.

## How does the design perform on measured frequencies?

### Takeaway

Yang et al. measured modes $(2,1),(3,1),(1,4)$ of a silicon nitride membrane at **517, 747, and 927 kHz**. Using those published frequencies in the undamped small-signal design gives four command copies at **0, 0.682107480, 1.371771199, 2.053878679 microseconds** with weights **0.153715285156, 0.346284714844, 0.346284714844, 0.153715285156**. This is a prediction at three selected nominal frequencies; no experiment with the filter on their device was performed. [Yang et al., Supplementary Material, Table I](https://arxiv.org/pdf/1902.01270) [Control verifier](../../explore/three_mode_shaper.py)

### Cited Findings

- The measured frequencies in Table I are distinct from the paper's theoretical frequency table. The authors report temperature dependence and use different interferometry setups. Their main experiment investigates strong driving outside the linear regime. The ideal shaper above targets the three selected linear resonances in the supplementary characterization. [Yang et al.](https://doi.org/10.1103/PhysRevLett.122.154301)
- An independently reproducible model example is an ideal fixed-edge rectangular membrane. At aspect ratio $4:3$ the lowest three mode frequencies are proportional to $5,\sqrt{52},\sqrt{73}$ by the rectangular membrane eigenfrequency formula. [Daniel Russell, Penn State Acoustics](https://www.acs.psu.edu/drussell/Demos/rect-membrane/rect-mem.html)

### Inferences

The dimensionless duration for the measured triplet is $6.6718334751542354222\ldots$. Three single-mode shapers, with individual durations $\pi/\omega_j$, have combined duration 2.175836357 microseconds. The optimum saves 5.60509% against this baseline. A stronger baseline combines an optimal two-mode shaper with a single-mode shaper. All frequency pairs in the rectangle have ratio below 3, so their exact minimum is $4\pi/(\omega_i+\omega_j)$. Taking the best of the three pairings gives 6.673480141 in normalized units. The four-impulse optimum saves **0.02467%** against that baseline, about **0.507 nanoseconds**. On the arithmetic progression $1:1.4:1.8$ the best pair-plus-single construction already reaches the optimum. [Two-mode derivation](../RH%20帰結の先行研究照合/最小幅.md) [Control verifier with both benchmarks](../../explore/three_mode_shaper.py)

A 10 ns timing-grid example changes the four nominal times to 0, 0.68, 1.37, 2.05 microseconds. Evaluating $|\sum_j w_j e^{2\pi i f t_j}|$ with the displayed weights gives approximately 0.000709, 0.003329, 0.001665 in ascending frequency order. These are calculated dimensionless residual gains, illustrating the effect of rounding before deployment. [Frequency inputs](https://arxiv.org/pdf/1902.01270) [Full-precision design](../../explore/three_mode_shaper.py)

### Gaps

The physical significance of a 0.507 ns advantage over the best pair-plus-single design depends on the available timing resolution, damping, and drift. The published triplet is a selection of modes from the membrane spectrum. The theorem gives no attenuation guarantee at the remaining resonances or in the nonlinear regime.

## How can another team verify and use the result?

### Takeaway

The verifier includes both the construction and a lower bound over all positive measures. The default run covers the parameter rectangle by 1,264 closed subrectangles; each successful test treats every parameter value in its subrectangle. A single-frequency-triplet run outputs a rigorously enclosed optimum and rounded causal implementation times. [Verifier](../../explore/three_mode_shaper.py)

### Cited Findings

- The three-frequency Fourier problem admits a dual interpretation via real trigonometric polynomials of prescribed spectrum. The lower bound here is verified over full time intervals, with parameter intervals included in every inequality. [Kozma–Oravecz, §1](https://arxiv.org/pdf/math/0601021) [Interval implementation](../../explore/three_mode_shaper.py)

### Inferences

A floating-point Newton iteration only suggests a trial box and a rational preconditioner $R$. For each parameter cell, interval arithmetic proves that $z\mapsto z-RF(z)$ maps that box into its interior and has derivative of maximum row-sum norm less than one. This supplies a true $x,y,w$ for every plant in the cell. For the matching lower bound, form $g(t)=(C'(x)\times C(x))\cdot C(t)$. Then $g(x)=g'(x)=0$, and $g(y)=0$ by the moment equations. The verifier uses interval Taylor bounds to prove $g''>0$ around $x$, $g'<0$ up to $y$, and $g>0$ on the remaining two intervals. Hence $g$ is nonnegative on $[-y,y]$ and its only zeros there are $\pm x,\pm y$. Any feasible centered measure integrates $g$ to zero and must be supported on those four points. If its width is smaller than $2y$, only $\pm x$ remain, contradicting $\cos x>0$. The first two sine equations have nonsingular matrix and force equal weights within each pair; the first cosine equation fixes the total inner weight. This proves uniqueness among arbitrary measures, including asymmetric ones. [Verifier and certificate arithmetic](../../explore/three_mode_shaper.py)

Run from the experiment directory with standard Python 3 and assertions enabled:

```sh
python3 explore/three_mode_shaper.py
python3 explore/three_mode_shaper.py --hz 517000 747000 927000
python3 explore/three_mode_shaper.py --design 1.5 1.8
python3 explore/three_mode_shaper.py --membrane
python3 explore/three_zero_width.py
```

The default run reports 1,264 certificates, enclosing all durations between 6.42271457340132… and 6.93142019597784… after outward rounding, and a minimum saving of 433 basis points against independent single-mode convolution. The $1:1.5:1.8$ design gives normalized duration 6.57571245573896…, savings of 5.80986% against three single filters and 0.10136% against the best two-plus-one. The $4:3$ membrane gives 6.81158076225123…, 4.84457%, and 0.08525%, respectively. The final command reproduces the earlier three-zeta-zero certificate unchanged. [Reusable control code](../../explore/three_mode_shaper.py) [Original verifier](../../explore/three_zero_width.py)

### Gaps

The area coverage and certificate code have been executed with Python integer arithmetic; no Lean kernel verification or hardware test has been performed. Simultaneous four-impulse designs and the general zero-free-interval optimization have prior art. This search did not establish publication priority for a parameter-uniform, continuous-time global optimality certificate on this rectangle.
