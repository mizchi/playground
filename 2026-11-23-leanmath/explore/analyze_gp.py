"""Merge gp.c outputs (given in increasing range order) into global records and per-decade stats."""
import math, sys
from collections import defaultdict

rec = {"G": [], "n2": [], "n3": [], "nl": []}
best = defaultdict(int)
dec = {}
arg = defaultdict(int)
for path in sys.argv[1:]:
    local = []
    for line in open(path):
        t = line.split()
        if t[0] == "REC":
            local.append(t)
        elif t[0] == "DEC":
            k = int(t[1]); mg, pg, mn, pn, c, gt, gl = float(t[2]), int(t[3]), float(t[4]), int(t[5]), int(t[6]), int(t[7]), int(t[8])
            d = dec.setdefault(k, [0, 0, 0, 0, 0, 0, 0])
            if mg > d[0]: d[0], d[1] = mg, pg
            if mn > d[2]: d[2], d[3] = mn, pn
            d[4] += c; d[5] += gt; d[6] += gl
        elif t[0] == "ARG":
            arg[t[1]] += int(t[2])
    for t in local:  # local running maxima -> global records
        kind, p = t[1], int(t[2])
        val = int(t[3]) if kind == "G" else int(t[-1])
        if val > best[kind]:
            best[kind] = val
            rec[kind].append((p, *map(int, t[3:])))

def show(kind, head):
    print(head)
    for r in rec[kind]:
        p = r[0]
        v = r[1] if kind == "G" else r[-1]
        print("| " + " | ".join(map(str, r)) + f" | {v / math.log(p) ** 2:.3f} |")
    print()

show("G", "### Records of G(p)\n| p | G(p) | bottleneck ℓ | G/ln² p |\n|---|---|---|---|")
show("n2", "### Records of the least quadratic non-residue n₂(p)\n| p | n₂(p) | n₂/ln² p |\n|---|---|---|")
show("n3", "### Records of the least cubic non-residue n₃(p) (p ≡ 1 mod 3)\n| p | n₃(p) | n₃/ln² p |\n|---|---|---|")
show("nl", "### Records of n_ℓ(p) over ℓ ≥ 5\n| p | ℓ | n_ℓ(p) | n_ℓ/ln² p |\n|---|---|---|---|")
print("### Per decade\n| decade | #primes | max G/ln² p | at p | max n₂/ln² p | at p | share with G(p) > n₂(p) | #(G > ln² p) |")
print("|---|---|---|---|---|---|---|---|")
for k in sorted(dec):
    d = dec[k]
    print(f"| 10^{k} | {d[4]} | {d[0]:.3f} | {d[1]} | {d[2]:.3f} | {d[3]} | {d[5] / d[4]:.2%} | {d[6]} |")
print()
tot = sum(arg.values())
print("### Bottleneck subgroup (smallest ℓ with n_ℓ(p) = G(p)), all primes < 2³²")
print("| ℓ | count | share |\n|---|---|---|")
for l in sorted(arg, key=lambda x: (x == "big", int(x) if x != "big" else 0)):
    print(f"| {l} | {arg[l]} | {arg[l] / tot:.3%} |")
