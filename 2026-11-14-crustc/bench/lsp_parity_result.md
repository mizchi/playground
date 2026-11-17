#### synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 100% (種類も 97%) | 名前 100% (種類も 100%) | 名前 100% (種類も 95%); 余分 #0 |
| workspace/symbol (上位 20 の一致) | 87% | 100% | 76% |
| definition (行き先の一致) | 同じ 0% (答えた 0%) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) |
| hover (名前を含む) | 0% | 100% | 100% |
| references (数の一致) | 0% | 100% | 100% |
| completion `x.\|` (書いてある名前を含む) | 0% (候補 0) | 100% (候補 3) | 100% (候補 5) |
| completion `na\|me` (同) | 0% (候補 0) | 91% (候補 145) | 56% (候補 5) |
| 時間の中央値 (ms) | defi 0.1, hove 0.1, comp 0.1, refe 0.1 | defi 0.5, hove 0.6, comp 4.1, refe 1.0 | defi 7.7, hove 8.4, comp 234.5, refe 20.3 |

#### crustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 99% (種類も 97%); 足りない extern "C" | 名前 100% (種類も 100%) | 名前 90% (種類も 82%); 足りない c_close, c_exit, c_fork, c_mkdir |
| workspace/symbol (上位 20 の一致) | 61% | 100% | 35% |
| definition (行き先の一致) | 同じ 0% (答えた 0%) | 同じ 100% (答えた 78%) | 同じ 87% (答えた 68%) |
| hover (名前を含む) | 0% | 88% | 72% |
| references (数の一致) | 0% | 100% | 42% |
| completion `x.\|` (書いてある名前を含む) | 0% (候補 0) | 100% (候補 26) | 90% (候補 37) |
| completion `na\|me` (同) | 0% (候補 0) | 68% (候補 85) | 60% (候補 3) |
| 時間の中央値 (ms) | defi 0.1, hove 0.1, comp 0.1, refe 0.1 | defi 0.9, hove 1.0, comp 7.7, refe 31.4 | defi 14.4, hove 12.3, comp 257.3, refe 325.4 |
