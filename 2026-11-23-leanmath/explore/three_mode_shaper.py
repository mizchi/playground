"""Minimum-duration positive shapers for three undamped, scalar-driven modes."""
import argparse
from fractions import Fraction
from math import cos, isqrt, pi, sin

from three_zero_width import (
    Interval, ONE, SCALE, cross, curve, decimal, dot, moment_and_jacobian, trig,
)

point = Interval.point


def inverse(matrix):
    augmented = [row[:] + [int(i == j) for j in range(3)]
                 for i, row in enumerate(matrix)]
    for j in range(3):
        pivot = max(range(j, 3), key=lambda i: abs(augmented[i][j]))
        augmented[j], augmented[pivot] = augmented[pivot], augmented[j]
        value = augmented[j][j]
        augmented[j] = [entry/value for entry in augmented[j]]
        for i in range(3):
            if i != j:
                value = augmented[i][j]
                augmented[i] = [a-value*b for a, b in zip(augmented[i], augmented[j])]
    return [row[3:] for row in augmented]


def float_seed(frequencies):
    """Only choose a trial box and preconditioner; inclusion below proves existence."""
    b = [f.mid().lo/SCALE for f in frequencies]
    x, y, w = 1.1, 3.35, .70
    for _ in range(8):
        residual = [w*cos(f*x)+(1-w)*cos(f*y) for f in b]
        jacobian = [[-w*f*sin(f*x), -(1-w)*f*sin(f*y), cos(f*x)-cos(f*y)]
                    for f in b]
        preconditioner = inverse(jacobian)
        delta = [sum(a*r for a, r in zip(row, residual)) for row in preconditioner]
        x, y, w = [z-d for z, d in zip([x, y, w], delta)]
    return ([point(f"{v:.15f}") for v in (x, y, w)],
            [[point(f"{v:.12f}") for v in row] for row in preconditioner])


class Subdivide(Exception):
    """The current parameter enclosure is too wide to conclude."""


def solve(frequencies):
    center, preconditioner = float_seed(frequencies)
    x, y, w = center
    mid_frequencies = [f.mid() for f in frequencies]
    residual, _ = moment_and_jacobian(center, mid_frequencies)
    # Each component depends on just one frequency. A centered form preserves
    # cancellation between the two collocation nodes in its frequency derivative.
    for i, f in enumerate(frequencies):
        derivative = -(w*x*trig(f*x)[1]+(ONE-w)*y*trig(f*y)[1])
        residual[i] = residual[i] + derivative*(f-mid_frequencies[i])
    residual_step = [dot(row, residual) for row in preconditioner]
    radii = [2*r.magnitude()+SCALE//10**14 for r in residual_step]
    box = [v.inflate(r) for v, r in zip(center, radii)]
    _, jacobian = moment_and_jacobian(box, frequencies)
    delta = [v-c for v, c in zip(box, center)]
    image = []
    for i, row in enumerate(preconditioner):
        derivative = [point(int(i == j))-dot(row, col) for j, col in enumerate(zip(*jacobian))]
        if sum(v.magnitude() for v in derivative) >= SCALE:
            raise Subdivide
        image.append(center[i]-residual_step[i]+dot(derivative, delta))
    if any(not (p.lo < v.lo <= v.hi < p.hi) for p, v in zip(box, image)):
        raise Subdivide
    return image


def sign_on_interval(fun, lo, hi, sign):
    """An adaptively subdivided cover, including both endpoints."""
    todo = [(lo, hi, 0)]
    while todo:
        a, b, depth = todo.pop()
        bound = fun(Interval(a, b))
        if (bound.lo if sign > 0 else -bound.hi) > 0:
            continue
        if depth == 12:
            raise Subdivide
        mid = (a+b)//2
        todo.extend(((a, mid, depth+1), (mid, b, depth+1)))


def certify(a, b):
    frequencies = [ONE, a, b]
    x, y, w = solve(frequencies)
    cx, dx = curve(x, frequencies)
    _, dy = curve(y, frequencies)
    if not (0 < x.lo < y.lo and 0 < w.lo <= w.hi < SCALE and cx[0].lo > 0
            and (dx[0]*dy[1]-dx[1]*dy[0]).hi < 0):
        raise Subdivide
    coeff = cross(dx, cx)
    squared = [f*f for f in frequencies]
    moment_bounds = []
    for order in range(3):
        powers = list(coeff)
        for _ in range(order+2):
            powers = [v*f for v, f in zip(powers, frequencies)]
        bound = sum(v.magnitude() for v in powers)
        moment_bounds.append(Interval(-bound, bound))

    def dual(t, order):
        center = t.mid()
        delta = t-center
        c, d = curve(center, frequencies)
        derivatives = [c, d, [-f*v for f, v in zip(squared, c)],
                       [-f*v for f, v in zip(squared, d)]]
        return (dot(coeff, derivatives[order]) + dot(coeff, derivatives[order+1])*delta
                + (moment_bounds[order]*delta*delta).over(2))

    # f'(0)=f'(x)=f(x)=f(y)=0 analytically. Strict curvature around x
    # and strict decrease near y let us cover both contact points.
    left_turn = x.lo - point(".42").lo
    right_turn = x.hi + point(".50").lo
    right_tail = y.lo - point(".34").lo
    if not (0 < left_turn < x.lo <= x.hi < right_turn < right_tail < y.lo):
        raise Subdivide
    for lo, hi, order, sign in [
        (0, left_turn, 0, 1), (left_turn, right_turn, 2, 1),
        (right_turn, right_tail, 0, 1), (right_tail, y.hi, 1, -1),
    ]:
        sign_on_interval(lambda t: dual(t, order), lo, hi, sign)
    return x, y, w


def check_rectangle():
    # Both endpoints are included. Each successful leaf proves all real pairs in it.
    initial = (Interval(point("1.40").lo, point("1.55").hi),
               Interval(point("1.68").lo, point("1.85").hi))
    pending = [initial]
    count = 0
    min_width, max_width = 10*SCALE, 0
    min_saving = Fraction(1)
    while pending:
        a, b = pending.pop()
        try:
            _, y, _ = certify(a, b)
        except Subdivide:
            ratios = [a, b]
            i = int(b.hi-b.lo > a.hi-a.lo)
            p = ratios[i]
            assert p.hi-p.lo > SCALE//100000, "unresolved parameter box"
            midpoint = (p.lo+p.hi)//2
            for half in [Interval(p.lo, midpoint), Interval(midpoint, p.hi)]:
                ratios[i] = half
                pending.append(tuple(ratios))
            continue
        count += 1
        min_width = min(min_width, 2*y.lo)
        max_width = max(max_width, 2*y.hi)
        pi_lower = Fraction("3.14159265358979323846")
        convolution_lower = pi_lower*(1+Fraction(SCALE, a.hi)+Fraction(SCALE, b.hi))
        min_saving = min(min_saving, 1-Fraction(2*y.hi, SCALE)/convolution_lower)
    print("Certified ratio rectangle [1.40,1.55] x [1.68,1.85]")
    print("Certificates:", count)
    print("Enclosure of normalized durations:", decimal(min_width), decimal(max_width))
    print("Minimum saving vs three independent shapers (basis points):",
          (10000*min_saving).__floor__())


def square_root_over_five(n):
    root = isqrt(n*SCALE*SCALE)
    return Interval(root, root+1).over(5)


def show_design(a, b, first_hz=None):
    x, y, w = certify(a, b)
    print("Normalized duration interval:", decimal(2*y.lo), decimal(2*y.hi))
    for name, value in [("x", x), ("y", y), ("inner total weight", w)]:
        print(name, decimal(value.lo), decimal(value.hi))
    # Convenience values are rounded nearest, outside the certificate.
    mid = lambda v: v.mid().lo/SCALE
    tx, ty, weight = map(mid, (x, y, w))
    print("Normalized causal times:", [round(v, 12) for v in (0, ty-tx, ty+tx, 2*ty)])
    print("Impulse weights:", [round(v/2, 12) for v in (1-weight, weight, weight, 1-weight)])
    convolution = pi*(1+1/mid(a)+1/mid(b))
    print(f"Convolution duration {convolution:.9f}, delay reduction {100*(1-2*ty/convolution):.5f}%")
    ratios = [1, mid(a), mid(b)]
    pair_then_single = min(pi/ratios[j] + 4*pi/(sum(ratios)-ratios[j]) for j in range(3))
    print(f"Best optimal pair then single {pair_then_single:.9f}, delay reduction {100*(1-2*ty/pair_then_single):.5f}%")
    if first_hz:
        microseconds = 1e6/(2*pi*float(first_hz))
        print("Causal times (microseconds):", [round(v*microseconds, 9) for v in (0, ty-tx, ty+tx, 2*ty)])
        print(f"Convolution duration (microseconds): {convolution*microseconds:.9f}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--design", nargs=2, metavar=("RATIO2", "RATIO3"))
    group.add_argument("--hz", nargs=3, metavar=("F1", "F2", "F3"),
                       help="three ascending frequencies in hertz")
    group.add_argument("--membrane", action="store_true", help="First three modes of a 4:3 rectangular membrane")
    args = parser.parse_args()
    if args.membrane:
        show_design(square_root_over_five(52), square_root_over_five(73))
    elif args.design or args.hz:
        first_hz = None
        if args.hz:
            first_hz, second, third = map(Fraction, args.hz)
            if first_hz <= 0:
                parser.error("the lowest target frequency must be positive")
            a, b = second/first_hz, third/first_hz
        else:
            a, b = map(Fraction, args.design)
        if not (Fraction("1.40") <= a <= Fraction("1.55") and Fraction("1.68") <= b <= Fraction("1.85")):
            parser.error("use frequency ratios inside the certified rectangle")
        def enclose(q):
            t = q*SCALE
            return Interval(t.numerator//t.denominator, -((-t.numerator)//t.denominator))
        show_design(enclose(a), enclose(b), first_hz)
    else:
        check_rectangle()


if __name__ == "__main__":
    main()
