crustc のリンカ: wild / rustc 1.97.0 (2d8144b78 2026-07-07)

### gen1000

| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| crustc | 118 | 18 | 28 | 104.9 | 520.9 | 12 | 4.97x | 一致 |
| crustc -O | 169 | 16 | 30 | 93.9 | 273.3 | 6 | 2.58x | 一致 |
| rustc -Cincremental | 827 | 233 | 417 | 22156.3 | 1043.4 | 9 | 3.78x | 一致 |
| rustc -O | 11448 | - | - | 0.0 | 382.4 | 2 | 1.00x | 一致 |

### fib

| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| crustc | 24 | 4 | 4 | 19.0 | 18.8 | 99 | 3.87x | 一致 |
| crustc -O | 34 | 4 | 5 | 16.0 | 13.8 | 73 | 2.85x | 一致 |
| rustc -Cincremental | 127 | 96 | 97 | 312.4 | 335.6 | 95 | 3.73x | 一致 |
| rustc -O | 111 | - | - | 0.0 | 334.6 | 25 | 1.00x | 一致 |

### hashmap

| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| crustc | 37 | 6 | 6 | 35.1 | 27.3 | 218 | 1.91x | 一致 |
| crustc -O | 41 | 6 | 6 | 29.3 | 18.9 | 176 | 1.55x | 一致 |
| rustc -Cincremental | 211 | 125 | 107 | 2632.2 | 373.7 | 732 | 6.43x | 一致 |
| rustc -O | 280 | - | - | 0.0 | 347.8 | 114 | 1.00x | 一致 |

### matmul

| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| crustc | 51 | 7 | 7 | 46.1 | 42.2 | 198 | 6.32x | 一致 |
| crustc -O | 56 | 7 | 7 | 37.9 | 26.3 | 53 | 1.69x | 一致 |
| rustc -Cincremental | 162 | 102 | 98 | 2671.3 | 378.4 | 692 | 22.13x | 一致 |
| rustc -O | 211 | - | - | 0.0 | 361.0 | 31 | 1.00x | 一致 |

### nbody

| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| crustc | 36 | 5 | 6 | 36.3 | 39.9 | 413 | 9.83x | 一致 |
| crustc -O | 46 | 5 | 5 | 28.4 | 22.6 | 69 | 1.65x | 一致 |
| rustc -Cincremental | 143 | 97 | 105 | 2226.5 | 365.9 | 1185 | 28.22x | 一致 |
| rustc -O | 165 | - | - | 0.0 | 363.9 | 42 | 1.00x | 一致 |

### sieve

| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| crustc | 28 | 4 | 4 | 20.4 | 20.6 | 247 | 2.68x | 一致 |
| crustc -O | 28 | 4 | 6 | 16.8 | 14.3 | 144 | 1.57x | 一致 |
| rustc -Cincremental | 119 | 92 | 90 | 1475.6 | 340.4 | 392 | 4.25x | 一致 |
| rustc -O | 122 | - | - | 0.0 | 335.0 | 92 | 1.00x | 一致 |

### sort

| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| crustc | 42 | 5 | 7 | 40.7 | 46.4 | 252 | 1.33x | 一致 |
| crustc -O | 51 | 6 | 8 | 32.3 | 30.0 | 195 | 1.03x | 一致 |
| rustc -Cincremental | 174 | 109 | 113 | 3102.2 | 384.9 | 1983 | 10.45x | 一致 |
| rustc -O | 281 | - | - | 0.0 | 348.0 | 190 | 1.00x | 一致 |

### strings

| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| crustc | 38 | 6 | 7 | 41.3 | 33.7 | 113 | 3.32x | 一致 |
| crustc -O | 52 | 6 | 6 | 33.7 | 22.3 | 73 | 2.17x | 一致 |
| rustc -Cincremental | 172 | 98 | 102 | 2198.4 | 357.3 | 121 | 3.57x | 一致 |
| rustc -O | 187 | - | - | 0.0 | 339.8 | 34 | 1.00x | 一致 |
