"""Direct positive Gaussian sums over squarefree integers, using the standard library."""

from math import (
    ceil, comb, cos, e, erfc, exp, fsum, isqrt, log, log1p, pi, sqrt,
)

# First entries of Andrew Odlyzko's 1000-decimal-place table, rounded by Python
# to binary64. The final entry also bounds the range of permitted experiments.
ORDINATES = (
    14.13472514173469379045, 21.02203963877155499262, 25.01085758014568876321,
    30.42487612585951321031, 32.93506158773918969066, 37.58617815882567125721,
    40.91871901214749518739, 43.32707328091499951949, 48.00515088116715972794,
    49.77383247767230218191,
)


def notch(height):
    if height >= ORDINATES[-1]:
        raise ValueError("The verification needs additional zero ordinates.")
    coefficients = [1.0]
    for gamma in (g for g in ORDINATES if g <= height):
        linear = -2 * cos(pi * gamma / height)
        result = [0.0] * (len(coefficients) + 2)
        for i, c in enumerate(coefficients):
            result[i] += c
            result[i + 1] += linear * c
            result[i + 2] += c
        coefficients = result
    assert min(coefficients) > 0
    m = len(coefficients) // 2
    step = 2 * pi / height
    mass = fsum(coefficients)
    return [((j - m) * step, c / mass) for j, c in enumerate(coefficients)]


def arithmetic_check(x, zeta3):
    height = 16 * sqrt(log(x))
    atoms = notch(height)
    m = len(atoms) // 2
    radius = atoms[-1][0]
    n_max = ceil(x * exp(radius + 9))
    squarefree = bytearray([1]) * (n_max + 1)
    for d in range(2, isqrt(n_max) + 1):
        squarefree[d*d::d*d] = bytes(n_max // (d*d))

    def moment(a):
        return fsum(p * exp(a*t) for t, p in atoms)

    # Evaluate all translates as a positive polynomial in exp(h*log(n/x)).
    # The endpoint correction accounts for the grid starting at -radius.
    h = 2 * pi / height
    poly = [p * exp(-t/4 - t*t/2) for t, p in reversed(atoms)]
    denominator = sqrt(2*pi*e) * moment(3/4)

    def summand(n):
        v = log(n/x)
        y = exp(h*v)
        value = 0.0
        for c in poly:
            value = value*y + c
        return exp(-v*v/2 - radius*v) * value / denominator

    total = fsum(summand(n) for n in range(1, n_max + 1) if squarefree[n])
    main_term = 6*x/pi**2
    predicted = pi**2/(6*zeta3) * moment(5/4)/moment(3/4)
    truncation = x * erfc((log(n_max/(x*exp(radius))) - 1)/sqrt(2)) / 2
    print(f"{x:3d} {height:9.5f} {m:2d} {n_max:9d} {total:15.10f} "
          f"{x*(total-main_term):12.8f} {predicted:12.8f} "
          f"{total-main_term-predicted/x:+12.3e} {truncation:.2e}")


def main():
    zeta3 = 2.5 * fsum((-1)**(n-1) / (n**3 * comb(2*n, n)) for n in range(1, 40))
    correction = pi**2/3 + fsum(
        2/n * log1p(2/(2*n-1)) - 2/n**2 for n in range(1, 10000)
    )
    print(f"kappa = {(4+correction)/(2*pi):.10f}")
    print("  x         T  m         N       sum          x*(sum-main)    residue"
          "       remainder      truncation")
    for x in (12, 48, 192):
        arithmetic_check(x, zeta3)


if __name__ == "__main__":
    main()
