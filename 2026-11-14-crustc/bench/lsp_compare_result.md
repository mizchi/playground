| 対象 | サーバ | init | ready | docsym (数) | wsym (見つかった) | edit→docsym | edit→diag | references 1 回目 / 2 回目 (数) | peak RSS (子も) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| synth | crustc | 4.9 ms | 14.6 ms | 0.8 ms (37) | 0.9 ms (5/5) | 2.1 ms | 1.5 ms | 207 ms / 3.5 ms (29) | 16.9 MB (15.8 MB) |
| synth | rust-analyzer | 23.9 ms | 3398 ms | 1.4 ms (37) | 1.0 ms (5/5) | 19.7 ms | 299 ms | 84.0 ms / 3.2 ms (29) | 596 MB (865 MB) |
| synth | rust-glancer | 26.0 ms | 5812 ms | 1.9 ms (38) | 31.2 ms (5/5) | 4.7 ms | - | 380 ms / 14.6 ms (29) | 15.7 MB (888 MB) |
| crustc | crustc | 2.5 ms | 86.7 ms | 1.8 ms (123) | 1.2 ms (5/5) | 13.6 ms | 10.9 ms | 71.9 ms / 10.0 ms (36) | 30.1 MB (30.1 MB) |
| crustc | rust-analyzer | 25.7 ms | 6653 ms | 4.4 ms (124) | 1.1 ms (5/5) | 12.3 ms | - | 1698 ms / 15.1 ms (36) | 1571 MB (1633 MB) |
| crustc | rust-glancer | 106 ms | 11194 ms | 7.7 ms (113) | 265 ms (5/5) | 48.0 ms | - | 1702 ms / 122 ms (36) | 29.8 MB (1799 MB) |
| big | crustc | 2.4 ms | 152 ms | 0.8 ms (37) | 14.4 ms (5/5) | 3.7 ms | 3.1 ms | 47.6 ms / 16.6 ms (29) | 55.1 MB (55.1 MB) |
| big | rust-analyzer | 20.1 ms | 4293 ms | 5.7 ms (37) | 14.7 ms (5/5) | 5.2 ms | 4875 ms | 487 ms / 7.1 ms (29) | 825 MB (2038 MB) |
| big | rust-glancer | 35.5 ms | 6206 ms | 1.9 ms (38) | 108 ms (5/5) | 5.0 ms | - | 926 ms / 29.4 ms (29) | 22.0 MB (1033 MB) |
