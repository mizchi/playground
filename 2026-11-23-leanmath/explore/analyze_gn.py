"""Summarize gn.csv: records of G(n)/ln^2 n by decade, composite vs prime, W(n) vs G(n)."""
import csv, math, sys
from collections import defaultdict

path = sys.argv[1]
rows = list(csv.DictReader(open(path)))
dec = defaultdict(lambda: {"all": (0, 0), "prime": (0, 0), "comp": (0, 0), "W": (0, 0)})
recG = []
best = 0
ratio_W_over_G = defaultdict(int)
viol_bach = []
cnt_gt1 = 0
maxW = (0, 0)
for r in rows:
    n, phi, G, W = int(r["n"]), int(r["phi"]), int(r["G"]), int(r["W"])
    L2 = math.log(n) ** 2
    rat = G / L2
    if G > 2 * L2:
        viol_bach.append(n)
    if n > 100 and G > L2:
        cnt_gt1 += 1
    d = int(math.log10(n))
    kind = "prime" if phi == n - 1 else "comp"
    for k in ("all", kind):
        if rat > dec[d][k][0]:
            dec[d][k] = (rat, n)
    if W:
        if W / L2 > dec[d]["W"][0]:
            dec[d]["W"] = (W / L2, n)
        if W > maxW[0]:
            maxW = (W, n)
    if G > best:
        best = G
        recG.append((n, phi, G))

print("Bach bound G(n) <= 2 ln^2 n violated for n in [3, N]:", viol_bach or "none")
print("count of n>100 with G(n) > ln^2 n:", cnt_gt1)
print("largest least Miller witness W(n):", maxW)
print()
print("| decade | max G/ln² (all n) | at n | max over composite n | at n | max over prime n | at n | max W/ln² (odd comp.) | at n |")
print("|---|---|---|---|---|---|---|---|---|")
for d in sorted(dec):
    e = dec[d]
    print(f"| 10^{d} | {e['all'][0]:.3f} | {e['all'][1]} | {e['comp'][0]:.3f} | {e['comp'][1]} | {e['prime'][0]:.3f} | {e['prime'][1]} | {e['W'][0]:.3f} | {e['W'][1]} |")
print()
print("Record holders of G(n) (G(n) larger than for every smaller n):")
print("| n | factorization | φ(n) | G(n) | G/ln² n |")
print("|---|---|---|---|---|")
def fac(n):
    out, p = [], 2
    while p * p <= n:
        e = 0
        while n % p == 0:
            n //= p; e += 1
        if e:
            out.append(f"{p}^{e}" if e > 1 else str(p))
        p += 1
    if n > 1:
        out.append(str(n))
    return "·".join(out)
for n, phi, G in recG:
    print(f"| {n} | {fac(n)} | {phi} | {G} | {G / math.log(n) ** 2:.3f} |")

def omega(n):
    k, p = 0, 2
    while p * p <= n:
        if n % p == 0:
            k += 1
            while n % p == 0:
                n //= p
        p += 1
    return k + (n > 1)

print()
print("Odd n only (the case Miller's test needs) — record holders of G(n):")
print("| n | factorization | G(n) | G/ln² n | W(n) |")
print("|---|---|---|---|---|")
best = 0
for r in rows:
    n, G, W = int(r["n"]), int(r["G"]), int(r["W"])
    if n % 2 and G > best:
        best = G
        print(f"| {n} | {fac(n)} | {G} | {G / math.log(n) ** 2:.3f} | {W or '-'} |")

print()
print("By number of distinct prime factors ω(n), n ≥ 1000:")
print("| ω(n) | count | mean G(n) | max G(n) | max G/ln² | at n |")
print("|---|---|---|---|---|---|")
by = defaultdict(lambda: [0, 0, 0, 0.0, 0])
for r in rows:
    n, G = int(r["n"]), int(r["G"])
    if n < 1000:
        continue
    w = omega(n)
    e = by[w]
    e[0] += 1; e[1] += G
    if G > e[2]:
        e[2] = G
    rat = G / math.log(n) ** 2
    if rat > e[3]:
        e[3], e[4] = rat, n
for w in sorted(by):
    c, s, m, rat, n = by[w]
    print(f"| {w} | {c} | {s / c:.2f} | {m} | {rat:.3f} | {n} = {fac(n)} |")
