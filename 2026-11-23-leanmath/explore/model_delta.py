"""Naive independence model for delta := density of primes p with G(p) > n_2(p).

Model: the residue symbols of the small primes q_1 = 2, q_2 = 3, ... behave independently;
P(n_2 = q_i) = 2^-i, P(l | p-1) = 1/(l-1), P(q_1..q_i all l-th powers | l | p-1) = l^-i.
G(p) > n_2(p) = q_i  iff  some odd prime l | p-1 has q_1..q_i all l-th power residues.
"""
def primes(n):
    s = bytearray([1]) * (n + 1); s[0] = s[1] = 0
    for i in range(2, int(n ** .5) + 1):
        if s[i]: s[i*i::i] = bytearray(len(s[i*i::i]))
    return [i for i in range(n + 1) if s[i]]
L = [l for l in primes(10 ** 6) if l > 2]
delta = 0.0
for i in range(1, 60):
    prod = 1.0
    for l in L:
        t = 1.0 / ((l - 1) * l ** i)
        if t < 1e-18: break
        prod *= 1 - t
    delta += 2.0 ** -i * (1 - prod)
print(f"naive model delta = {delta:.6f}")

# Model 2: quadratic-reciprocity entanglement.  If l | p-1 then (l/p) = (p/l)(-1)^{(l-1)(p-1)/4}
# = (-1)^{(l-1)/2 * (p-1)/2}; for l = 1 mod 4 this forces (l/p) = +1.  So conditional on
# n_2 = q_i, the event A_l = {l | p-1, q_1..q_i are l-th powers} has its probability doubled
# when l < q_i, l = 1 mod 4, and is impossible when l = q_i, l = 1 mod 4.
Q = primes(400)
delta2 = 0.0
for i in range(1, len(Q) + 1):
    qi = Q[i - 1]
    prod = 1.0
    for l in L:
        t = 1.0 / ((l - 1) * l ** i)
        if t < 1e-18: break
        if l % 4 == 1 and l < qi: t *= 2
        elif l % 4 == 1 and l == qi: t = 0
        prod *= 1 - t
    delta2 += 2.0 ** -i * (1 - prod)
print(f"reciprocity-corrected model delta = {delta2:.6f}")
