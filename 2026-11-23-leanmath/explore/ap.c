// ap.c — P(q) := max over gcd(a,q)=1 of the least prime p = a (mod q), for 3 <= q <= Q.
// GRH => P(q) <= (phi(q) log q)^2 for q >= 4 (Lamzouri-Li-Soundararajan 2015, explicit).
// Prints CSV: q,phi,P,worst_a,P_over_q2,P_over_phi_log2
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <math.h>
static uint32_t gcd(uint32_t a, uint32_t b) { while (b) { uint32_t t = a % b; a = b; b = t; } return a; }
int main(int argc, char **argv) {
  uint32_t Q = argc > 1 ? atoi(argv[1]) : 10000;
  uint32_t X = argc > 2 ? atoi(argv[2]) : 200000000;
  uint8_t *c = calloc(X + 1, 1);
  for (uint64_t i = 2; i * i <= X; i++) if (!c[i]) for (uint64_t j = i * i; j <= X; j += i) c[j] = 1;
  uint32_t np = 0; for (uint32_t i = 2; i <= X; i++) if (!c[i]) np++;
  uint32_t *pr = malloc(sizeof(uint32_t) * np); np = 0;
  for (uint32_t i = 2; i <= X; i++) if (!c[i]) pr[np++] = i;
  free(c);
  uint8_t *seen = malloc(Q + 1);
  printf("q,phi,P,worst_a,P_over_q2,P_over_phi_log2\n");
  for (uint32_t q = 3; q <= Q; q++) {
    memset(seen, 0, q);
    uint32_t phi = 0; for (uint32_t a = 1; a < q; a++) if (gcd(a, q) == 1) phi++;
    uint32_t got = 0, P = 0, wa = 0;
    for (uint32_t k = 0; k < np && got < phi; k++) {
      uint32_t r = pr[k] % q;
      if (q % pr[k] == 0 || seen[r]) continue;
      seen[r] = 1; got++; P = pr[k]; wa = r;
    }
    if (got < phi) { fprintf(stderr, "sieve too small at q=%u\n", q); return 1; }
    double lq = log((double)q), b = phi * lq;
    printf("%u,%u,%u,%u,%.6f,%.6f\n", q, phi, P, wa, (double)P / ((double)q * q), P / (b * b));
  }
  return 0;
}
