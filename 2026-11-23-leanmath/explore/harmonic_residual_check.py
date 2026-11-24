"""Compare the exact three-harmonic curve with an independent time-grid LP.

Run with "uv run --script explore/harmonic_residual_check.py".
The LP is a floating-point cross-check. The continuous proof is in Lean.
"""
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["scipy==1.15.3"]
# ///
import argparse
from fractions import Fraction
from math import cos, pi

import numpy as np
from scipy.optimize import linprog


def predicted(radius):
    l = cos(radius)
    if radius >= 3 * pi / 4:
        return 0.0
    if l >= 0:
        return max(l, (2*l+1)/(2*l+3), 4*l*(1+l)/(4*l*(1+l)+2))
    return (1-2*l*l)/(3-2*l-4*l*l)


def grid_minimax(times):
    # Reflection averages away each sine moment while preserving each cosine
    # moment. A probability on these nodes represents symmetric pairs ±t.
    points = len(times)
    modes = np.cos(np.outer([1, 2, 3], times))
    inequalities = np.column_stack((np.vstack((modes, -modes)), -np.ones(6)))
    normalized = np.append(np.ones(points), 0)[None, :]
    objective = np.append(np.zeros(points), 1)
    result = linprog(objective, A_ub=inequalities, b_ub=np.zeros(6),
                     A_eq=normalized, b_eq=[1], bounds=(0, None), method="highs")
    if not result.success:
        raise RuntimeError(result.message)
    return result.fun


def check_clock_grids():
    coarse = np.array([-0.5, 0.5, 1.0])
    assert abs(grid_minimax(np.arccos(coarse)) - 1/5) < 1e-9
    # Outside the proven bracket, the extrapolated formula gives 1/2.
    # On these nodes 6*T3 - T2 >= 5, and the listed masses attain 5/7.
    outside = [Fraction(-1, 2), Fraction(-1, 4), Fraction(1)]
    weights = [0, Fraction(32, 35), Fraction(3, 35)]
    t2 = lambda z: 2*z**2-1
    t3 = lambda z: 4*z**3-3*z
    assert all(6*t3(z)-t2(z) >= 5 for z in outside)
    assert [sum(w*f(z) for w, z in zip(weights, outside)) for f in (lambda z: z, t2, t3)] == [
        Fraction(-1, 7), Fraction(-5, 7), Fraction(5, 7)]
    rng = np.random.default_rng(314159)
    errors = []
    for _ in range(96):
        a, b = rng.uniform(0.5, 0.75), rng.uniform(0.75, 1.0)
        # Fill only outside the gap and include all three required nodes.
        nodes = np.r_[-0.5, a, b, rng.uniform(-0.5, a, 23), rng.uniform(b, 1, 11)]
        expected = 1/6 + 8*(0.75-a)*(b-0.75)/(3*(3+4*a*b))
        errors.append(abs(grid_minimax(np.arccos(nodes)) - expected))
    for a, b in [(0.75, 1.0), (0.5, 0.75)]:
        assert abs(grid_minimax(np.arccos([-0.5, a, b])) - 1/6) < 1e-9
    print(f"Five ticks: 1/5. On 96 irregular clocks, max |LP - grid formula| = {max(errors):.3g}")
    assert max(errors) < 1e-7


def check(plot_path):
    inner, outer = Fraction(3, 4), -Fraction(1, 2)
    moments = [Fraction(8, 15)*fn(inner) + Fraction(7, 15)*fn(outer)
               for fn in (lambda z: z, lambda z: 2*z*z-1, lambda z: 4*z**3-3*z)]
    assert moments == [Fraction(1, 6), -Fraction(1, 6), Fraction(1, 6)]
    print("Exact centered moments at radius 2π/3:", *map(str, moments))
    radii = np.unique(np.r_[np.linspace(0, pi, 97), pi/4, 2*pi/5, 2*pi/3, 3*pi/4])
    gaps = [grid_minimax(np.linspace(0, r, 513)) - predicted(r) for r in radii]
    print(f"{len(radii)} radii, 513 nodes: LP minus formula in "
          f"[{min(gaps):.3g}, {max(gaps):.3g}]")
    assert min(gaps) > -1e-7 and max(gaps) < 2e-5

    check_clock_grids()

    if plot_path:
        import matplotlib.pyplot as plt

        fig, ax = plt.subplots(figsize=(7.2, 3.8), layout="constrained")
        widths = np.linspace(0, 0.9, 901)
        ax.plot(widths, [predicted(pi*w) for w in widths], color="#164961", lw=2.5)
        for w, val, label, offset in [(0.5, 1/3, "1/3", (6, 10)),
                                      (2/3, 1/6, "1/6", (-16, -19)),
                                      (0.75, 0, "0", (4, 10))]:
            ax.scatter(w, val, c="#b54d20", zorder=3)
            ax.annotate(label, (w, val), xytext=offset, textcoords="offset points")
        ax.set(xlim=(0, 0.9), ylim=(-0.04, 1.04),
               xlabel="Available interval width / fundamental period",
               ylabel="Minimum worst-mode residual amplitude")
        ax.set_xticks([0, 0.25, 0.5, 2/3, 0.75], ["0", "1/4", "1/2", "2/3", "3/4"])
        ax.grid(axis="y", alpha=0.22)
        fig.savefig(plot_path, dpi=175, facecolor="white")
        plt.close(fig)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plot", metavar="PNG", help="also draw the curve (requires matplotlib)")
    check(parser.parse_args().plot)
