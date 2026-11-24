"""Check finite ladder clocks and certify a finite-tolerance irrational qutrit bound."""
from fractions import Fraction
from math import acos, cos, factorial, isqrt, pi, prod, sin, sqrt, tan

from three_zero_width import Interval, ONE, SCALE


def check_ladder(q):
    # Folding the integer coefficient lists modulo z^q - 1 tests all roots at once.
    weights = [sum(i + j == k for i in range(q) for j in range(q))
               for k in range(2*q - 1)]
    assert weights == list(range(1, q+1)) + list(range(q-1, 0, -1))
    for order in (0, 1):
        folded = [sum(n**order * weights[n] for n in range(k, len(weights), q))
                  for k in range(q)]
        assert len(set(folded)) == 1
    h = 2*pi/q
    if q > 2:
        # Test strict positivity away from the grid contacts, including near the edges.
        for k in range(q-1):
            for step in range(1, 100):
                t = (k + step/100)*h
                trig_poly = (2*sin(q*t/2)**2)/(cos(t)-cos(h))
                assert -trig_poly*(1 - t*tan(h/2)/(h*tan(t/2))) > 0
    return weights


def root(x):
    assert x.lo >= 0
    return Interval(isqrt(x.lo*SCALE), isqrt(x.hi*SCALE) + 1)


def reciprocal(x):
    assert x.lo > 0
    return Interval(SCALE**2//x.hi, -(-SCALE**2//x.lo))


def absolute(x):
    assert x.lo > 0 or x.hi < 0
    return x if x.lo > 0 else -x


def exponential(x):
    assert 0 <= x.lo <= x.hi <= 8*SCALE
    value = term = ONE
    for n in range(1, 91):
        term = (term*x).over(n)
        value = value + term
    # Beyond degree 90 the omitted terms have successive ratios at most 8/92.
    assert 2*8**91*SCALE < factorial(91)
    return Interval(value.lo, value.hi + 1)


def slit_rhs(gaps, radius, half_width):
    epsilon = Interval.point('0.01')
    nodes = []
    vertical = []
    for gap in gaps:
        height = root(radius*radius - gap*gap)
        w = gap*reciprocal(radius + height)
        nodes.extend((w, -w))
        vertical.extend((height, height))
    b = prod((absolute(w) for w in nodes), start=ONE)
    result = b*exponential(half_width*radius)
    for i, w in enumerate(nodes):
        derivative = reciprocal(ONE - w*w)
        for j, v in enumerate(nodes):
            if i != j:
                derivative = derivative*absolute(w-v)*reciprocal(ONE-v*w)
        result = result + b*epsilon*exponential(half_width*(radius-vertical[i]))*reciprocal(absolute(w)*derivative)
    return result


def output_trace_norm(x, y, populations):
    p, q, r = populations
    # Characteristic cubic of the pure-input output, with signs retained.
    a, b, c = x*sqrt(p*q), x*sqrt(q*r), y*sqrt(p*r)
    scale = sqrt((a*a + b*b + c*c)/3)
    if scale == 0:
        return 0
    angle = acos(min(1, max(-1, a*b*c/scale**3)))
    return sum(abs(2*scale*cos((angle + 2*k*pi)/3)) for k in range(3))


def check_global_qutrit(delta):
    v = 25*delta/(8 - 5*delta + sqrt(64 - 80*delta))
    m = (v-1)/2
    x = 2*v/(5+v)
    y = (1+v)*x - v
    u = 4/(5+v)
    witness = output_trace_norm(x, y, [u/2, 1-u, u/2])
    grid = max(output_trace_norm(x, y, [i/100, j/100, (100-i-j)/100])
               for i in range(101) for j in range(101-i))
    assert abs(witness-delta) < 1e-12 and delta-1e-6 < grid <= witness+1e-12
    print(f'equally spaced qutrit global diamond: width*Delta={2*acos(m):.9f}, '
          f'population grid={grid:.9f}, witness={witness:.9f}')


if __name__ == '__main__':
    for q in (2, 3, 4, 8, 16):
        weights = check_ladder(q)
        separate = 2*pi*sum(1/j for j in range(1, q))
        joint = 4*pi*(q-1)/q
        print(f'd={q:2}  width*Delta={joint:.9f}  separate/joint={separate/joint:.6f}  weights={weights}')
    drift = .01
    q = 8
    residual = max((sin(pi*j*drift)/(q*sin(pi*j*(1+drift)/q)))**2 for j in range(1, q))
    print(f'd=8 at 1% drift: max coherence {residual:.9f}, diamond upper {(pi*(q-1)*drift/2)**2:.9f}')
    p = Interval.point
    bound = slit_rhs([ONE, root(p(2)), ONE + root(p(2))], p('3.04'), p('2.53'))
    assert Fraction(bound.hi, SCALE) < Fraction(9997, 10000)
    print(f'irrational qutrit slit RHS in [{bound.lo}/{SCALE}, {bound.hi}/{SCALE}]')
    epsilon = Fraction(1, 100)
    m = (3*epsilon-1)/(2*(1-epsilon))
    at_zero = (epsilon-m)/(1-m)
    assert at_zero + (1-at_zero)*m == epsilon
    assert at_zero + (1-at_zero)*(2*m*m-1) == -epsilon
    print(f'equally spaced qutrit at 1%: width*Delta={2*acos(m):.9f}, mass at zero={at_zero}')
    check_global_qutrit(.01)
