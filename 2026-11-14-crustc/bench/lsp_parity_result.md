#### synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | crustc の前回との差 |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | +13.4 点 |
| definition (行き先の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) | +0.0 点 |
| hover (名前を含む) | 100% | 100% | +0.0 点 |
| references (数の一致) | 100% | 100% | +0.0 点 |
| documentHighlight (数の一致) | 100% | 100% | +0.0 点 |
| rename (書き換えの一致) | 同じ 100% (断ったのも同じ 0/0) | 同じ 100% (断ったのも同じ 0/0) | +0.0 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 95%) | +0.0 点 |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 11) | 100% (候補 3) | +0.0 点 |
| completion `na\|me` (同) | 97% (候補 25) | 91% (候補 145) | +0.0 点 |
| 一致の平均 | 99.7% | 99.1% | +1.3 点 |
| 時間 p50/p90 (ms) | defi 0.5/1.0, hove 0.5/1.1, comp 0.9/1.1, refe 0.7/10.1, rena 1.0/11.4, sign 0.5/0.7, docu 0.8/10.6 | defi 0.3/0.6, hove 0.4/1.0, comp 2.6/5.4, refe 0.7/4.6, rena 0.8/3.4, sign 0.2/0.4, docu 0.7/1.1 | defi -0.2, hove -0.1, comp -0.2, refe -2.8, rena -2.4, sign -0.0, docu -2.6 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 13 ms (515 ms) / 17 MB | 2689 ms (3592 ms) / 868 MB | +3 ms / +1 MB |

#### fixture (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | crustc の前回との差 |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | +33.3 点 |
| definition (行き先の一致) | 同じ 100% (答えた 90%) | 同じ 100% (答えた 90%) | +7.1 点 |
| hover (名前を含む) | 88% | 88% | +0.0 点 |
| references (数の一致) | 100% | 100% | +38.5 点 |
| documentHighlight (数の一致) | 100% | 100% | +20.0 点 |
| rename (書き換えの一致) | 同じ 96% (断ったのも同じ 14/14) | 同じ 100% (断ったのも同じ 14/14) | +33.7 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 50%) | 同じ 100% (答えた 45%) |  |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 28) | 100% (候補 18) | +0.0 点 |
| completion `na\|me` (同) | 90% (候補 3) | 69% (候補 76) | +3.4 点 |
| 一致の平均 | 97.3% | 95.6% | +15.4 点 |
| 時間 p50/p90 (ms) | defi 0.5/0.8, hove 0.5/1.4, comp 1.1/2.1, refe 0.9/1.7, rena 0.7/1.6, sign 0.3/0.7, docu 0.7/1.4 | defi 0.4/1.1, hove 0.6/2.9, comp 4.2/11.6, refe 0.9/5.6, rena 0.4/0.6, sign 0.2/0.5, docu 0.5/1.4 | defi -0.0, hove +0.1, comp +0.3, refe -0.6, rena -0.3, sign -0.1, docu -0.8 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 7 ms (509 ms) / 17 MB | 332 ms (3495 ms) / 833 MB | -1 ms / -1 MB |

#### crustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | crustc の前回との差 |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | +0.9 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | +27.1 点 |
| definition (行き先の一致) | 同じ 100% (答えた 78%) | 同じ 100% (答えた 78%) | +0.0 点 |
| hover (名前を含む) | 88% | 88% | +5.0 点 |
| references (数の一致) | 97% | 100% | +5.6 点 |
| documentHighlight (数の一致) | 100% | 100% | +0.0 点 |
| rename (書き換えの一致) | 同じ 94% (断ったのも同じ 23/23) | 同じ 100% (断ったのも同じ 23/23) | +0.0 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 95%) | 同じ 100% (答えた 95%) | +0.0 点 |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 48) | 100% (候補 26) | +0.0 点 |
| completion `na\|me` (同) | 84% (候補 7) | 68% (候補 85) | +4.0 点 |
| 一致の平均 | 96.3% | 95.5% | +4.3 点 |
| 時間 p50/p90 (ms) | defi 0.8/1.1, hove 0.8/1.1, comp 1.2/2.7, refe 5.4/459.1, rena 0.9/6.8, sign 1.0/2.0, docu 1.2/10.4 | defi 0.7/2.2, hove 0.9/1.9, comp 5.6/14.0, refe 22.4/932.6, rena 0.8/110.4, sign 0.5/0.9, docu 1.7/3.3 | defi -0.1, hove -0.1, comp +0.0, refe -10.0, rena -0.4, sign -0.2, docu -13.7 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 139 ms (541 ms) / 53 MB | 4932 ms (6090 ms) / 2368 MB | +55 ms / +8 MB |

#### ide (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | crustc の前回との差 |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | +0.7 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | +38.5 点 |
| definition (行き先の一致) | 同じ 97% (答えた 90%) | 同じ 100% (答えた 92%) | +2.9 点 |
| hover (名前を含む) | 92% | 90% | +5.0 点 |
| references (数の一致) | 100% | 100% | +5.0 点 |
| documentHighlight (数の一致) | 100% | 100% | +2.8 点 |
| rename (書き換えの一致) | 同じ 100% (断ったのも同じ 15/15) | 同じ 100% (断ったのも同じ 15/15) | +20.0 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 95% (答えた 90%) | 同じ 100% (答えた 95%) | +10.5 点 |
| completion `x.\|` (書いてある名前を含む) | 75% (候補 81) | 95% (候補 56) | -10.0 点 |
| completion `na\|me` (同) | 100% (候補 10) | 90% (候補 120) | +8.3 点 |
| 一致の平均 | 96.0% | 97.5% | +8.4 点 |
| 時間 p50/p90 (ms) | defi 0.8/1.4, hove 0.8/1.2, comp 1.5/2.6, refe 3.6/19.8, rena 1.9/15.7, sign 1.7/8.2, docu 1.8/11.1 | defi 0.8/4.0, hove 0.7/1.6, comp 6.3/11.8, refe 2.3/553.0, rena 0.9/7.8, sign 0.2/0.4, docu 1.8/5.6 | defi -0.3, hove -0.2, comp +0.2, refe -9.4, rena -0.5, sign +0.6, docu -14.3 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 170 ms (522 ms) / 53 MB | 5391 ms (5792 ms) / 1874 MB | +75 ms / +18 MB |
