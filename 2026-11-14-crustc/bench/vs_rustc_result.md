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
| crustc | 53 | 7 | 8 | 47.6 | 44.1 | 245 | 7.20x | 一致 |
| crustc -O | 60 | 8 | 8 | 39.0 | 26.9 | 77 | 2.26x | 一致 |
| rustc -Cincremental | 158 | 106 | 121 | 2671.3 | 378.4 | 746 | 21.99x | 一致 |
| rustc -O | 234 | - | - | 0.0 | 361.0 | 34 | 1.00x | 一致 |

### nbody

| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| crustc | 37 | 5 | 8 | 37.2 | 40.9 | 442 | 7.12x | 一致 |
| crustc -O | 47 | 5 | 6 | 29.3 | 23.3 | 92 | 1.48x | 一致 |
| rustc -Cincremental | 149 | 103 | 106 | 2226.5 | 365.9 | 1231 | 19.85x | 一致 |
| rustc -O | 177 | - | - | 0.0 | 363.9 | 62 | 1.00x | 一致 |

### sieve

| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| crustc | 28 | 4 | 5 | 21.0 | 21.5 | 348 | 3.26x | 一致 |
| crustc -O | 32 | 6 | 4 | 17.5 | 15.0 | 252 | 2.36x | 一致 |
| rustc -Cincremental | 142 | 103 | 98 | 1475.6 | 340.4 | 432 | 4.04x | 一致 |
| rustc -O | 163 | - | - | 0.0 | 335.0 | 107 | 1.00x | 一致 |

### sort

| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| crustc | 40 | 5 | 6 | 35.2 | 31.1 | 725 | 3.95x | 一致 |
| crustc -O | 46 | 6 | 6 | 28.3 | 19.7 | 550 | 3.00x | 一致 |
| rustc -Cincremental | 203 | 112 | 128 | 3102.2 | 384.9 | 2041 | 11.10x | 一致 |
| rustc -O | 317 | - | - | 0.0 | 348.0 | 184 | 1.00x | 一致 |

### strings

| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| crustc | 42 | 12 | 7 | 41.5 | 33.5 | 259 | 7.44x | 一致 |
| crustc -O | 50 | 6 | 6 | 34.0 | 22.6 | 141 | 4.05x | 一致 |
| rustc -Cincremental | 191 | 106 | 101 | 2198.4 | 357.3 | 122 | 3.49x | 一致 |
| rustc -O | 200 | - | - | 0.0 | 339.8 | 35 | 1.00x | 一致 |
