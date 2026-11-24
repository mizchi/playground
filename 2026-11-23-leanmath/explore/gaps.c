// gaps.c — maximal prime gaps up to N, compared with the explicit RH bound of
// Carneiro-Milinovich-Soundararajan (2019): RH => p_{n+1} - p_n < (22/25) sqrt(p_n) log p_n for p_n > 3.
// Prints the record (maximal) gaps: p,gap,gap/(sqrt(p) log p)
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <math.h>
#define SEG (1u << 22)
int main(int argc, char **argv) {
  uint64_t N = argc > 1 ? strtoull(argv[1], 0, 10) : 1000000000ULL;
  uint32_t B = (uint32_t)sqrt((double)N) + 2;
  uint8_t *c = calloc(B + 1, 1); uint32_t *bp = malloc(sizeof(uint32_t) * B); uint32_t nb = 0;
  for (uint32_t i = 2; i <= B; i++) if (!c[i]) { bp[nb++] = i; for (uint64_t j = (uint64_t)i * i; j <= B; j += i) c[j] = 1; }
  uint8_t *s = malloc(SEG);
  uint64_t prev = 0, rec = 0; double maxratio = 0; uint64_t maxratio_p = 0;
  printf("p,gap,ratio\n");
  for (uint64_t L = 2; L <= N; L += SEG) {
    uint64_t R = L + SEG - 1; if (R > N) R = N;
    memset(s, 1, R - L + 1);
    for (uint32_t k = 0; k < nb; k++) {
      uint64_t q = bp[k]; if (q * q > R) break;
      uint64_t st = (L + q - 1) / q * q; if (st < q * q) st = q * q;
      for (uint64_t x = st; x <= R; x += q) s[x - L] = 0;
    }
    for (uint64_t x = L; x <= R; x++) if (s[x - L]) {
      if (prev) {
        uint64_t g = x - prev;
        double r = g / (sqrt((double)prev) * log((double)prev));
        if (prev > 3 && r > maxratio) { maxratio = r; maxratio_p = prev; }
        if (g > rec) { rec = g; printf("%llu,%llu,%.6f\n", (unsigned long long)prev, (unsigned long long)g, r); }
      }
      prev = x;
    }
  }
  fprintf(stderr, "max ratio over p>3: %.6f at p=%llu (bound 0.88)\n", maxratio, (unsigned long long)maxratio_p);
  return 0;
}
