"""Certify the minimum support diameter for the first three zeta ordinates.

Run with python3 explore/three_zero_width.py.  All certificate arithmetic uses
integers or rational numbers.  The only external inputs are the three LMFDB
zero enclosures copied below (https://www.lmfdb.org/zeros/zeta/).
"""
from dataclasses import dataclass
from fractions import Fraction
from math import factorial

SCALE = 10**36


@dataclass(frozen=True)
class Interval:
    lo: int
    hi: int

    def __post_init__(self):
        assert self.lo <= self.hi

    @staticmethod
    def point(value):
        scaled = Fraction(value) * SCALE
        assert scaled.denominator == 1
        return Interval(scaled.numerator, scaled.numerator)

    def __add__(self, other):
        return Interval(self.lo + other.lo, self.hi + other.hi)

    def __neg__(self):
        return Interval(-self.hi, -self.lo)

    def __sub__(self, other):
        return self + (-other)

    def __mul__(self, other):
        p = [x*y for x in (self.lo, self.hi) for y in (other.lo, other.hi)]
        return Interval(min(p)//SCALE, -((-max(p))//SCALE))

    def over(self, n):
        assert n > 0
        return Interval(self.lo//n, -((-self.hi)//n))

    def mid(self):
        m = (self.lo + self.hi)//2
        return Interval(m, m)

    def magnitude(self):
        return max(abs(self.lo), abs(self.hi))

    def inflate(self, radius):
        return Interval(self.lo-radius, self.hi+radius)


Z = Interval.point(0)
ONE = Interval.point(1)


def trig(angle):
    """Taylor at the midpoint, extended using |sin'|, |cos'| <= 1."""
    t = angle.mid()
    if t.magnitude() > 6*SCALE:
        half_cos, half_sin = trig(t.over(2))
        cosine = half_cos*half_cos - half_sin*half_sin
        sine = half_cos*half_sin
        radius = max(t.lo-angle.lo, angle.hi-t.hi)
        return cosine.inflate(radius), (sine+sine).inflate(radius)
    assert t.magnitude() <= 6*SCALE
    sq = -(t*t)
    cosine = cterm = ONE
    sine = sterm = t
    for j in range(1, 36):
        cterm = (cterm*sq).over((2*j-1)*(2*j))
        sterm = (sterm*sq).over((2*j)*(2*j+1))
        cosine = cosine + cterm
        sine = sine + sterm
    # The Lagrange remainders after degrees 70 and 71 are each < 1/SCALE.
    assert 6**71*SCALE < factorial(71) and 6**72*SCALE < factorial(72)
    radius = max(t.lo-angle.lo, angle.hi-t.hi) + 1
    return cosine.inflate(radius), sine.inflate(radius)


def dot(a, b):
    return sum((x*y for x, y in zip(a, b)), Z)


def cross(a, b):
    return [a[(i+1)%3]*b[(i+2)%3]-a[(i+2)%3]*b[(i+1)%3] for i in range(3)]


# https://www.lmfdb.org/zeros/zeta/list?limit=3&N=1
# The published error is 2.5e-31. Widen to 1e-30, including printed rounding.
ORDINATES = (
    "14.1347251417346937904572519835625",
    "21.0220396387715549926284795938969",
    "25.0108575801456887632137909925628",
)
B = [Interval.point(v).inflate(10**6).over(2) for v in ORDINATES]


def curve(t, frequencies=B):
    pairs = [trig(b*t) for b in frequencies]
    return [c for c, s in pairs], [-b*s for b, (c, s) in zip(frequencies, pairs)]


def moment_and_jacobian(box, frequencies=B):
    x, y, w = box
    cx, dx = curve(x, frequencies)
    cy, dy = curve(y, frequencies)
    residual = [w*a+(ONE-w)*b for a, b in zip(cx, cy)]
    jacobian = [[w*da, (ONE-w)*db, a-b]
                for a, b, da, db in zip(cx, cy, dx, dy)]
    return residual, jacobian


def enclose_solution():
    center = [Interval.point(s) for s in (".158890692", ".469945213", ".694385833")]
    box = [p.inflate(SCALE//10**7) for p in center]
    # Fixed rational preconditioner, approximate inverse of the central Jacobian.
    rows = [["-.110247", ".080065", "-.137721"],
            ["-.212358", ".516629", "-.354152"],
            [".419424", ".109211", "-.333466"]]
    R = [[Interval.point(v) for v in row] for row in rows]
    for _ in range(4):
        center = [p.mid() for p in box]
        displacement = [p-c for p, c in zip(box, center)]
        residual, _ = moment_and_jacobian(center)
        _, jacobian = moment_and_jacobian(box)
        new = []
        for i, row in enumerate(R):
            derivative = [Interval.point(int(i == j))-dot(row, col)
                          for j, col in enumerate(zip(*jacobian))]
            assert sum(p.magnitude() for p in derivative) < SCALE//10000
            # Fixed-point map z -> z-R F(z), evaluated in mean value form.
            new.append(center[i]-dot(row, residual)+dot(derivative, displacement))
        assert all(p.lo < q.lo <= q.hi < p.hi for p, q in zip(box, new))
        box = new
    return box


def decimal(integer):
    sign = "-" if integer < 0 else ""
    whole, rest = divmod(abs(integer), SCALE)
    return f"{sign}{whole}.{rest:036d}"


def main():
    x, y, w = enclose_solution()
    assert 0 < w.lo <= w.hi < SCALE
    cx, dx = curve(x)
    # Perpendicular to the exact C(x) and C'(x). Pick sign positive at 0.
    coeff = cross(dx, cx)
    assert cx[0].lo > 0
    cy, dy = curve(y)
    # The sine moment equations have independent columns for x and y.
    # Divide the two derivative rows by -b_j to get the sine rows.
    determinant = dx[0]*dy[1] - dx[1]*dy[0]
    assert determinant.hi < 0

    def eval_dual(t, order):
        c, derivative = curve(t)
        if order == 0:
            return dot(coeff, c)
        if order == 1:
            return dot(coeff, derivative)
        return dot(coeff, [-(b*b)*v for b, v in zip(B, c)])

    assert 120*SCALE//1000 < x.lo <= x.hi < 200*SCALE//1000
    assert 440*SCALE//1000 < y.lo <= y.hi < 470*SCALE//1000
    for start, stop, order, sign in [(0, 120, 0, 1), (120, 200, 2, 1),
                                     (200, 440, 0, 1), (440, 470, 1, -1)]:
        lower = min((v.lo if sign == 1 else -v.hi) for v in (
            eval_dual(Interval(k*SCALE//1000, (k+1)*SCALE//1000), order)
            for k in range(start, stop)))
        assert lower > 0
        print(f"[{start}/1000,{stop}/1000] sign*derivative({order}) >= {decimal(lower)}")

    for name, value in [("x", x), ("y", y), ("w", w), ("minimum width", y+y)]:
        print(f"{name}: [{decimal(value.lo)}, {decimal(value.hi)}]")


if __name__ == "__main__":
    main()
