#### synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | crustc の前回との差 |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | +0.0 点 |
| definition (行き先の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) | +0.0 点 |
| hover (名前を含む) | 100% | 100% | +0.0 点 |
| references (数の一致) | 100% | 100% | +0.0 点 |
| documentHighlight (数の一致) | 100% | 100% | +0.0 点 |
| rename (書き換えの一致) | 同じ 100% (断ったのも同じ 0/0) | 同じ 100% (断ったのも同じ 0/0) | +0.0 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 95%) | +0.0 点 |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 10) | 100% (候補 3) | +0.0 点 |
| completion `na\|me` (同) | 97% (候補 25) | 91% (候補 145) | +0.0 点 |
| 一致の平均 | 99.7% | 99.1% | +0.0 点 |
| 時間 p50/p90 (ms) | defi 0.6/1.2, hove 0.6/1.2, comp 1.1/1.5, refe 0.8/11.1, rena 0.9/13.4, sign 0.7/0.9, docu 0.8/11.5 | defi 0.4/0.8, hove 0.6/0.9, comp 3.3/5.4, refe 1.0/8.1, rena 0.8/4.0, sign 0.2/0.3, docu 0.8/1.4 | defi +0.1, hove +0.1, comp +0.2, refe +0.1, rena -0.1, sign +0.2, docu +0.0 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 9 ms (511 ms) / 18 MB | 2498 ms (3552 ms) / 872 MB | -4 ms / +1 MB |

#### fixture (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | crustc の前回との差 |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | +0.0 点 |
| definition (行き先の一致) | 同じ 100% (答えた 90%) | 同じ 100% (答えた 90%) | +0.0 点 |
| hover (名前を含む) | 88% | 88% | +0.0 点 |
| references (数の一致) | 100% | 100% | +0.0 点 |
| documentHighlight (数の一致) | 100% | 100% | +0.0 点 |
| rename (書き換えの一致) | 同じ 96% (断ったのも同じ 14/14) | 同じ 100% (断ったのも同じ 14/14) | +0.0 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 50%) | 同じ 100% (答えた 45%) | +0.0 点 |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 26) | 100% (候補 18) | +0.0 点 |
| completion `na\|me` (同) | 90% (候補 3) | 69% (候補 76) | +0.0 点 |
| 一致の平均 | 97.3% | 95.6% | +0.0 点 |
| 時間 p50/p90 (ms) | defi 0.5/1.1, hove 0.5/1.3, comp 0.9/2.5, refe 0.9/1.8, rena 0.8/1.6, sign 0.5/0.9, docu 0.8/1.7 | defi 0.6/1.4, hove 0.7/3.2, comp 3.9/11.3, refe 1.2/5.4, rena 0.5/0.9, sign 0.2/0.5, docu 0.6/1.7 | defi +0.0, hove -0.0, comp -0.2, refe -0.0, rena +0.1, sign +0.1, docu +0.1 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 11 ms (513 ms) / 18 MB | 329 ms (3594 ms) / 718 MB | +4 ms / +1 MB |

#### crustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | crustc の前回との差 |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | +0.0 点 |
| definition (行き先の一致) | 同じ 100% (答えた 78%) | 同じ 100% (答えた 78%) | +0.0 点 |
| hover (名前を含む) | 88% | 88% | +0.0 点 |
| references (数の一致) | 97% | 100% | +0.0 点 |
| documentHighlight (数の一致) | 100% | 100% | +0.0 点 |
| rename (書き換えの一致) | 同じ 94% (断ったのも同じ 23/23) | 同じ 100% (断ったのも同じ 23/23) | +0.0 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 95%) | 同じ 100% (答えた 95%) | +0.0 点 |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 33) | 100% (候補 26) | +0.0 点 |
| completion `na\|me` (同) | 84% (候補 7) | 68% (候補 85) | +0.0 点 |
| 一致の平均 | 96.3% | 95.5% | +0.0 点 |
| 時間 p50/p90 (ms) | defi 0.9/1.2, hove 0.9/1.2, comp 1.4/2.9, refe 5.4/497.4, rena 1.0/8.0, sign 1.1/2.6, docu 1.4/13.5 | defi 0.8/1.3, hove 0.8/1.8, comp 5.4/13.4, refe 27.9/869.4, rena 0.9/94.1, sign 0.4/0.8, docu 1.8/3.6 | defi +0.1, hove +0.1, comp +0.2, refe +0.0, rena +0.1, sign +0.1, docu +0.3 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 124 ms (525 ms) / 55 MB | 4835 ms (5939 ms) / 2389 MB | -16 ms / +2 MB |

#### ide (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | crustc の前回との差 |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | +0.0 点 |
| definition (行き先の一致) | 同じ 100% (答えた 85%) | 同じ 100% (答えた 85%) | +2.7 点 |
| hover (名前を含む) | 98% | 92% | +5.0 点 |
| references (数の一致) | 95% | 100% | -5.0 点 |
| documentHighlight (数の一致) | 97% | 100% | -2.9 点 |
| rename (書き換えの一致) | 同じ 100% (断ったのも同じ 15/15) | 同じ 100% (断ったのも同じ 15/15) | +0.0 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 95% (答えた 90%) | 同じ 100% (答えた 95%) | +0.0 点 |
| completion `x.\|` (書いてある名前を含む) | 90% (候補 100) | 95% (候補 88) | +15.0 点 |
| completion `na\|me` (同) | 78% (候補 14) | 65% (候補 64) | -21.7 点 |
| 一致の平均 | 95.3% | 95.3% | -0.7 点 |
| 時間 p50/p90 (ms) | defi 1.1/1.8, hove 1.0/1.6, comp 1.5/3.2, refe 6.1/656.8, rena 1.9/93.0, sign 1.3/7.2, docu 2.8/89.5 | defi 0.8/3.5, hove 0.8/3.5, comp 6.8/19.0, refe 8.6/465.3, rena 1.3/10.7, sign 0.4/0.7, docu 2.5/9.7 | defi +0.3, hove +0.2, comp +0.1, refe +2.5, rena -0.0, sign -0.4, docu +1.1 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 141 ms (543 ms) / 56 MB | 5016 ms (5417 ms) / 1949 MB | -29 ms / +2 MB |
