// gp.c — for every prime p in [lo, hi] (hi < 2^32): decompose G(p) by maximal subgroup.
//
// (Z/pZ)^* is cyclic, so its maximal subgroups are the l-th powers for primes l | p-1, and
//   G(p) = max_{l | p-1} n_l(p),   n_l(p) := least l-th power non-residue mod p.
// n_2 = least quadratic non-residue (deterministic sqrt mod p), n_3 = least cubic
// non-residue (deterministic cube roots), etc.  GRH (Bach) => G(p) <= 2 ln^2 p.
//
// Output (stdout, machine-readable lines):
//   REC G  p G l          new record of G(p) (l = which maximal subgroup was last escaped)
//   REC n2 p n2           new record of least QNR
//   REC n3 p n3           new record of least cubic non-residue (p = 1 mod 3)
//   REC nl p l nl         new record of n_l for l >= 5
//   DEC k  maxG/ln2 p  maxn2/ln2 p  primes  cntGgtN2  cntGgtLn2
//   ARG l count           how often l is the bottleneck (smallest l with n_l(p) = G(p))
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <math.h>

static inline uint64_t mulmod(uint64_t a, uint64_t b, uint64_t m) { return a * b % m; }
static uint64_t powmod(uint64_t a, uint64_t e, uint64_t m) {
  uint64_t r = 1; a %= m;
  while (e) { if (e & 1) r = mulmod(r, a, m); a = mulmod(a, a, m); e >>= 1; }
  return r;
}
static int jacobi(uint64_t a, uint64_t n) { // n odd
  int t = 1; a %= n;
  while (a) {
    while (!(a & 1)) { a >>= 1; uint64_t r = n & 7; if (r == 3 || r == 5) t = -t; }
    uint64_t tmp = a; a = n; n = tmp;
    if ((a & 3) == 3 && (n & 3) == 3) t = -t;
    a %= n;
  }
  return n == 1 ? t : 0;
}

#define MAXF 12
#define SEG (1u << 20)
#define NSMALL 2000

int main(int argc, char **argv) {
  uint64_t lo = argc > 1 ? strtoull(argv[1], 0, 10) : 3;
  uint64_t hi = argc > 2 ? strtoull(argv[2], 0, 10) : 100000000ULL;
  if (lo < 3) lo = 3;
  uint32_t B = (uint32_t)sqrt((double)hi) + 2;
  uint32_t SM = B > 20000 ? B : 20000;
  uint8_t *comp = calloc(SM + 1, 1);
  uint32_t *bp = malloc(sizeof(uint32_t) * (SM + 1)); uint32_t nbp = 0;
  for (uint32_t i = 2; i <= SM; i++) if (!comp[i]) { bp[nbp++] = i; for (uint64_t j = (uint64_t)i * i; j <= SM; j += i) comp[j] = 1; }
  uint32_t nsieve = 0; while (nsieve < nbp && bp[nsieve] <= B) nsieve++;

  uint64_t *rem = malloc(sizeof(uint64_t) * SEG);
  uint32_t *fac = malloc(sizeof(uint32_t) * SEG * MAXF);
  uint8_t *nf = malloc(SEG);
  uint8_t *isp = malloc(SEG);

  uint32_t recG = 0, recN2 = 0, recN3 = 0, recNl = 0;
  static uint64_t argcount[64]; uint64_t arg_big = 0;
  int curdec = -1; double dmaxG = 0, dmaxN2 = 0; uint64_t dpG = 0, dpN2 = 0, dcnt = 0, dGgtN2 = 0, dGgtL = 0;

  for (uint64_t L = lo; L <= hi; L += SEG) {           // segment of p values [L, L+SEG)
    uint64_t R = L + SEG - 1; if (R > hi) R = hi;
    uint32_t len = (uint32_t)(R - L + 1);
    memset(isp, 1, len); memset(nf, 0, len);
    for (uint32_t i = 0; i < len; i++) rem[i] = L + i - 1;   // m = p - 1
    for (uint32_t k = 0; k < nsieve; k++) {
      uint64_t q = bp[k];
      // primality of p
      uint64_t s = (L + q - 1) / q * q; if (s < q * q) s = q * q;
      for (uint64_t x = s; x <= R; x += q) isp[x - L] = 0;
      // factor m = p-1 in [L-1, R-1]
      uint64_t m0 = L - 1;
      uint64_t t = (m0 + q - 1) / q * q;
      for (uint64_t x = t; x <= R - 1; x += q) {
        uint32_t i = (uint32_t)(x - m0);
        fac[i * MAXF + nf[i]++] = (uint32_t)q;
        uint64_t r = rem[i]; do r /= q; while (r % q == 0); rem[i] = r;
      }
    }
    for (uint32_t i = 0; i < len; i++) {
      uint64_t p = L + i;
      if (!isp[i] || p < 3) continue;
      uint32_t ls[MAXF + 1]; int nl = 0;
      for (int j = 0; j < nf[i]; j++) ls[nl++] = fac[i * MAXF + j];
      if (rem[i] > 1) ls[nl++] = (uint32_t)rem[i];
      uint32_t G = 0, Gl = 0, n2 = 0;
      for (int j = 0; j < nl; j++) {
        uint32_t l = ls[j], nlv = 0;
        for (uint32_t k = 0;; k++) {
          uint32_t a = k < nbp ? bp[k] : 0; if (!a) { fprintf(stderr, "ran out\n"); return 1; }
          int nonres = (l == 2) ? (jacobi(a, p) == -1) : (powmod(a, (p - 1) / l, p) != 1);
          if (nonres) { nlv = a; break; }
        }
        if (l == 2) n2 = nlv;
        if (l == 3 && nlv > recN3) { recN3 = nlv; printf("REC n3 %llu %u\n", (unsigned long long)p, nlv); }
        if (l >= 5 && nlv > recNl) { recNl = nlv; printf("REC nl %llu %u %u\n", (unsigned long long)p, l, nlv); }
        if (nlv > G || (nlv == G && l < Gl)) { G = nlv; Gl = l; }   // ties -> smallest l
      }
      if (G > recG) { recG = G; printf("REC G %llu %u %u\n", (unsigned long long)p, G, Gl); }
      if (n2 > recN2) { recN2 = n2; printf("REC n2 %llu %u\n", (unsigned long long)p, n2); }
      if (Gl < 64) argcount[Gl]++; else arg_big++;
      int dec = (int)floor(log10((double)p));
      if (dec != curdec) {
        if (curdec >= 0) printf("DEC %d %.6f %llu %.6f %llu %llu %llu %llu\n", curdec, dmaxG, (unsigned long long)dpG, dmaxN2, (unsigned long long)dpN2, (unsigned long long)dcnt, (unsigned long long)dGgtN2, (unsigned long long)dGgtL);
        curdec = dec; dmaxG = dmaxN2 = 0; dcnt = dGgtN2 = dGgtL = 0;
      }
      double lp = log((double)p), l2 = lp * lp;
      dcnt++;
      if (G > n2) dGgtN2++;
      if (G > l2) dGgtL++;
      if (G / l2 > dmaxG) { dmaxG = G / l2; dpG = p; }
      if (n2 / l2 > dmaxN2) { dmaxN2 = n2 / l2; dpN2 = p; }
    }
    fflush(stdout);
  }
  printf("DEC %d %.6f %llu %.6f %llu %llu %llu %llu\n", curdec, dmaxG, (unsigned long long)dpG, dmaxN2, (unsigned long long)dpN2, (unsigned long long)dcnt, (unsigned long long)dGgtN2, (unsigned long long)dGgtL);
  for (int l = 2; l < 64; l++) if (argcount[l]) printf("ARG %d %llu\n", l, (unsigned long long)argcount[l]);
  if (arg_big) printf("ARG big %llu\n", (unsigned long long)arg_big);
  return 0;
}
