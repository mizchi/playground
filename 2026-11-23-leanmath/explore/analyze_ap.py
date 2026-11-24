"""Summarize ap.csv: worst-case least prime in an arithmetic progression vs q^2 and the GRH bound."""
import csv, math, sys
rows = list(csv.DictReader(open(sys.argv[1])))
print("| q range | max P(q)/q² | at q | max P(q)/(φ(q) ln q)² | at q | max P(q)/(φ(q) ln² q) | at q |")
print("|---|---|---|---|---|---|---|")
lo = 3
for hi in (100, 1000, 10000, 20000):
    seg = [r for r in rows if lo <= int(r["q"]) <= hi]
    a = max(seg, key=lambda r: float(r["P_over_q2"]))
    b = max(seg, key=lambda r: float(r["P_over_phi_log2"]))
    c = max(seg, key=lambda r: int(r["P"]) / (int(r["phi"]) * math.log(int(r["q"])) ** 2))
    cval = int(c["P"]) / (int(c["phi"]) * math.log(int(c["q"])) ** 2)
    print(f"| {lo}–{hi} | {float(a['P_over_q2']):.4f} | {a['q']} | {float(b['P_over_phi_log2']):.5f} | {b['q']} | {cval:.3f} | {c['q']} |")
    lo = hi + 1
viol = [r["q"] for r in rows if int(r["q"]) >= 4 and float(r["P_over_phi_log2"]) > 1]
print()
print("q >= 4 with P(q) > (φ(q) ln q)²:", viol or "none")
big = [r for r in rows if int(r["P"]) > int(r["q"]) ** 2]
print("q with P(q) > q²:", [r["q"] for r in big] or "none")
