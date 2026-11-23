// gn.c — G(n) for every n <= N, plus the least Miller witness W(n) for odd composite n.
//
//   G(n) := least x such that { a <= x : gcd(a,n)=1 } generates (Z/nZ)^*.
//   W(n) := least a >= 2 that is a Miller (strong-probable-prime) witness for odd composite n.
//
// Bach (1990): GRH  =>  G(n) <= 2 (ln n)^2 for all n > 1.
// Unconditionally: W(n) <= G(n) (strong liars lie in a proper subgroup), so GRH => Miller's
// deterministic test with bases <= 2 ln^2 n is correct.  Here we only measure the real values.
//
// Subgroup closure: adding g to H costs O(|<H,g>|) and sizes at least double, so total
// work per n is O(phi(n)).
//
// usage: gn N [lo] [hi]   -> CSV on stdout: n,phi,G,W,G_over_ln2
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <math.h>

static uint64_t powmod(uint64_t a, uint64_t e, uint64_t m) {
  uint64_t r = 1 % m; a %= m;
  while (e) { if (e & 1) r = r * a % m; a = a * a % m; e >>= 1; }
  return r;
}
static uint64_t gcd(uint64_t a, uint64_t b) { while (b) { uint64_t t = a % b; a = b; b = t; } return a; }

// strong probable prime test to base a (n odd > 2)
static int is_sprp(uint64_t n, uint64_t a) {
  uint64_t d = n - 1; int s = 0;
  while (!(d & 1)) { d >>= 1; s++; }
  uint64_t x = powmod(a, d, n);
  if (x == 1 || x == n - 1) return 1;
  for (int i = 1; i < s; i++) { x = x * x % n; if (x == n - 1) return 1; }
  return 0;
}

int main(int argc, char **argv) {
  uint32_t N = argc > 1 ? (uint32_t)atol(argv[1]) : 100000;
  uint32_t lo = argc > 2 ? (uint32_t)atol(argv[2]) : 3;
  uint32_t hi = argc > 3 ? (uint32_t)atol(argv[3]) : N;
  uint32_t *phi = malloc(sizeof(uint32_t) * (N + 1));
  for (uint32_t i = 0; i <= N; i++) phi[i] = i;
  for (uint32_t p = 2; p <= N; p++) if (phi[p] == p) for (uint32_t k = p; k <= N; k += p) phi[k] -= phi[k] / p;
  uint8_t *inH = calloc(N + 1, 1);
  uint32_t *H = malloc(sizeof(uint32_t) * (N + 1));
  printf("n,phi,G,W,G_over_ln2\n");
  for (uint32_t n = lo; n <= hi; n++) {
    uint32_t h = 1; H[0] = 1; inH[1] = 1;
    uint32_t G = 1;
    for (uint32_t a = 2; h < phi[n]; a++) {
      uint32_t g = a % n;
      if (gcd(g, n) != 1 || inH[g]) continue;
      uint32_t h0 = h; uint64_t cur = g;
      while (!inH[cur]) {
        for (uint32_t i = 0; i < h0; i++) { uint32_t x = (uint32_t)((uint64_t)H[i] * cur % n); inH[x] = 1; H[h++] = x; }
        cur = cur * g % n;
      }
      G = a;
    }
    for (uint32_t i = 0; i < h; i++) inH[H[i]] = 0;
    uint32_t W = 0;
    if ((n & 1) && phi[n] != n - 1) {           // odd composite
      for (uint32_t a = 2;; a++) {
        if (gcd(a, n) != 1 || !is_sprp(n, a)) { W = a; break; }
      }
      if (W > G) { fprintf(stderr, "VIOLATION W>G at n=%u\n", n); return 1; }
    }
    double L = log((double)n);
    printf("%u,%u,%u,%u,%.6f\n", n, phi[n], G, W, G / (L * L));
  }
  return 0;
}
