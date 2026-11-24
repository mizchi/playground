"""Reciprocity-corrected model for the bottleneck distribution of G(p) = max_l n_l(p).

Bottleneck := smallest l with n_l(p) = G(p).  For odd l0:
  P(B = l0) = sum_i P(n_2 = q_i) sum_{j>i} w(l0,i) l0^-(j-1) (1 - 1/l0)
              * prod_{odd l < l0} (1 - w(l,i) l^-(j-1)) * prod_{l > l0} (1 - w(l,i) l^-j)
with w(l,i) = c/(l-1) = P(l | p-1 | n_2 = q_i):  c = 2 if l = 1 mod 4 and l < q_i (l is then
forced to be a QR), c = 0 if l = 1 mod 4 and l = q_i, c = 1 otherwise.
Usage: model_bottleneck.py [gp outputs...]  (compares with their ARG lines)
"""
import sys
from collections import defaultdict

def primes(n):
    s = bytearray([1]) * (n + 1); s[0] = s[1] = 0
    for i in range(2, int(n ** .5) + 1):
        if s[i]: s[i*i::i] = bytearray(len(s[i*i::i]))
    return [i for i in range(n + 1) if s[i]]

PR = primes(200000)
Q = PR[:40]
ODD = PR[1:]

NAIVE = "--naive" in sys.argv

def w(l, i):
    if NAIVE: return 1.0 / (l - 1)
    qi = Q[i - 1]
    if l % 4 == 1 and l < qi: return 2.0 / (l - 1)
    if l % 4 == 1 and l == qi: return 0.0
    return 1.0 / (l - 1)

def prod_except(l0, i, j):
    out = 1.0
    for l in ODD:
        if l == l0: continue
        e = j - 1 if l < l0 else j
        t = w(l, i) * l ** -e
        if l > l0 and t < 1e-17: break
        out *= 1 - t
    return out

def p_bottleneck(l0, I=30):
    tot = 0.0
    for i in range(1, I):
        for j in range(i + 1, I + 1):
            base = w(l0, i) * l0 ** -(j - 1) * (1 - 1 / l0)
            if base < 1e-16: break
            tot += 2.0 ** -i * base * prod_except(l0, i, j)
    return tot

obs = defaultdict(int)
for path in [a for a in sys.argv[1:] if a != "--naive"]:
    for line in open(path):
        t = line.split()
        if t[0] == "ARG": obs[t[1]] += int(t[2])
N = sum(obs.values())
print("| bottleneck ℓ | model | observed | (obs − model)/σ |")
print("|---|---|---|---|")
total = 0.0
for l0 in [3, 5, 7, 11, 13, 17, 19, 23]:
    m = p_bottleneck(l0); total += m
    if N:
        o = obs[str(l0)] / N
        sig = (m * (1 - m) / N) ** .5
        print(f"| {l0} | {m:.6f} | {o:.6f} | {(o - m) / sig:+.1f} |")
    else:
        print(f"| {l0} | {m:.6f} | | |")
print(f"\nsum over l0 <= 23: {total:.6f}  (N = {N})")
