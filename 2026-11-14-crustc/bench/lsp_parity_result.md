#### synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer | crustc の前回との差 |
|---|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | 名前 100% (種類も 95%); 余分 #0 | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | 76% | +0.0 点 |
| definition (行き先の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) | +0.0 点 |
| hover (名前を含む) | 100% | 100% | 100% | +0.0 点 |
| references (数の一致) | 100% | 100% | 100% | +0.0 点 |
| documentHighlight (数の一致) | 100% | 100% | 100% | +0.0 点 |
| rename (書き換えの一致) | 同じ 100% (断ったのも同じ 0/0) | 同じ 100% (断ったのも同じ 0/0) | 同じ 100% (断ったのも同じ 0/0) | +0.0 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 95%) | 同じ 0% (答えた 0%) | +0.0 点 |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 10) | 100% (候補 3) | 100% (候補 5) | +0.0 点 |
| completion `na\|me` (同) | 97% (候補 25) | 91% (候補 145) | 56% (候補 5) | +0.0 点 |
| 一致の平均 | 99.7% | 99.1% | 83.1% | +0.0 点 |
| 時間 p50/p90 (ms) | defi 0.4/0.7, hove 0.3/0.7, comp 0.8/0.9, refe 0.6/1.2, rena 0.5/1.4, sign 0.4/0.6, docu 0.5/1.0 | defi 0.4/0.8, hove 0.5/1.0, comp 3.4/6.4, refe 1.0/7.2, rena 1.1/4.0, sign 0.3/1.7, docu 0.9/1.5 | defi 6.0/13.1, hove 5.5/12.8, comp 213.3/240.6, refe 14.4/215.1, rena 15.4/201.6, sign 0.2/0.3, docu 9.4/20.0 | defi -0.2, hove -0.3, comp -0.3, refe -0.2, rena -0.4, sign -0.3, docu -0.3 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 10 ms (512 ms) / 19 MB | 2724 ms (3627 ms) / 866 MB | 6493 ms (6493 ms) / 889 MB | +1 ms / +1 MB |

#### fixture (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer | crustc の前回との差 |
|---|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | 名前 100% (種類も 82%); 余分 #0 | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | 48% | +0.0 点 |
| definition (行き先の一致) | 同じ 100% (答えた 90%) | 同じ 100% (答えた 90%) | 同じ 92% (答えた 85%) | +0.0 点 |
| hover (名前を含む) | 88% | 88% | 85% | +0.0 点 |
| references (数の一致) | 100% | 100% | 79% | +0.0 点 |
| documentHighlight (数の一致) | 100% | 100% | 79% | +0.0 点 |
| rename (書き換えの一致) | 同じ 96% (断ったのも同じ 14/14) | 同じ 100% (断ったのも同じ 14/14) | 同じ 85% (断ったのも同じ 14/14) | +0.0 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 50%) | 同じ 100% (答えた 45%) | 同じ 0% (答えた 0%) | +0.0 点 |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 26) | 100% (候補 18) | 95% (候補 24) | +0.0 点 |
| completion `na\|me` (同) | 90% (候補 3) | 69% (候補 76) | 52% (候補 18) | +0.0 点 |
| 一致の平均 | 97.3% | 95.6% | 71.5% | +0.0 点 |
| 時間 p50/p90 (ms) | defi 0.4/0.9, hove 0.3/0.9, comp 0.9/2.1, refe 0.5/1.4, rena 0.4/1.3, sign 0.2/0.5, docu 0.4/1.4 | defi 0.4/0.9, hove 0.5/4.9, comp 4.3/10.7, refe 0.9/5.6, rena 0.6/1.0, sign 0.2/0.7, docu 0.6/1.4 | defi 9.6/22.1, hove 8.8/209.7, comp 215.3/274.7, refe 19.4/240.0, rena 12.4/67.0, sign 0.2/0.3, docu 11.3/62.9 | defi -0.1, hove -0.2, comp -0.0, refe -0.4, rena -0.5, sign -0.3, docu -0.4 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 10 ms (513 ms) / 19 MB | 415 ms (3639 ms) / 852 MB | 5147 ms (5147 ms) / 881 MB | -0 ms / +1 MB |

#### crustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer | crustc の前回との差 |
|---|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | 名前 90% (種類も 82%); 足りない c_close, c_exit, c_fork, c_mkdir | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | 35% | +0.0 点 |
| definition (行き先の一致) | 同じ 100% (答えた 78%) | 同じ 100% (答えた 78%) | 同じ 87% (答えた 68%) | +0.0 点 |
| hover (名前を含む) | 88% | 88% | 72% | +0.0 点 |
| references (数の一致) | 97% | 100% | 39% | +0.0 点 |
| documentHighlight (数の一致) | 100% | 100% | 55% | +0.0 点 |
| rename (書き換えの一致) | 同じ 94% (断ったのも同じ 23/23) | 同じ 100% (断ったのも同じ 23/23) | 同じ 47% (断ったのも同じ 23/23) | +0.0 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 95%) | 同じ 100% (答えた 95%) | 同じ 0% (答えた 0%) | +0.0 点 |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 33) | 100% (候補 26) | 90% (候補 37) | +0.0 点 |
| completion `na\|me` (同) | 84% (候補 8) | 68% (候補 85) | 60% (候補 3) | +0.0 点 |
| 一致の平均 | 96.3% | 95.5% | 57.5% | +0.0 点 |
| 時間 p50/p90 (ms) | defi 0.9/1.6, hove 0.9/1.4, comp 1.7/3.2, refe 4.2/65.3, rena 0.8/5.2, sign 1.5/2.6, docu 1.1/2.8 | defi 0.8/3.0, hove 0.9/2.9, comp 5.9/22.3, refe 24.1/968.2, rena 0.8/111.7, sign 0.5/0.8, docu 1.8/3.7 | defi 11.3/53.7, hove 10.1/306.9, comp 227.3/270.6, refe 200.5/1177.2, rena 123.0/965.5, sign 0.2/0.3, docu 80.6/206.4 | defi -0.1, hove -0.0, comp +0.3, refe -1.2, rena -0.1, sign +0.4, docu -0.3 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 129 ms (531 ms) / 66 MB | 5594 ms (7353 ms) / 2406 MB | 14736 ms (14887 ms) / 579 MB | +6 ms / +11 MB |

#### ide (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer | rust-glancer | crustc の前回との差 |
|---|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | 名前 100% (種類も 98%); 余分 #0, #1 | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | 26% | +0.0 点 |
| definition (行き先の一致) | 同じ 100% (答えた 92%) | 同じ 100% (答えた 92%) | 同じ 97% (答えた 90%) | +0.0 点 |
| hover (名前を含む) | 98% | 95% | 90% | +0.0 点 |
| references (数の一致) | 98% | 100% | 40% | +2.5 点 |
| documentHighlight (数の一致) | 100% | 100% | 43% | +2.9 点 |
| rename (書き換えの一致) | 同じ 96% (断ったのも同じ 13/13) | 同じ 100% (断ったのも同じ 13/13) | 同じ 44% (断ったのも同じ 13/13) | -3.7 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 95%) | 同じ 100% (答えた 95%) | 同じ 0% (答えた 0%) | +5.3 点 |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 85) | 100% (候補 75) | 100% (候補 79) | +10.0 点 |
| completion `na\|me` (同) | 89% (候補 13) | 84% (候補 254) | 68% (候補 67) | +11.2 点 |
| 一致の平均 | 98.1% | 97.9% | 61.0% | +2.8 点 |
| 時間 p50/p90 (ms) | defi 1.2/4.0, hove 1.0/2.3, comp 1.7/4.0, refe 6.5/56.9, rena 3.6/13.6, sign 1.3/3.0, docu 4.4/11.7 | defi 1.0/5.9, hove 0.8/2.8, comp 9.8/19.7, refe 8.5/986.6, rena 1.6/16.4, sign 0.4/0.8, docu 3.6/14.2 | defi 16.3/63.9, hove 15.7/237.2, comp 258.1/313.3, refe 151.1/988.8, rena 174.5/963.8, sign 0.2/0.3, docu 131.8/291.6 | defi +0.1, hove -0.0, comp +0.2, refe +0.4, rena +1.8, sign +0.0, docu +1.6 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 145 ms (546 ms) / 67 MB | 5077 ms (5478 ms) / 1970 MB | 1099 ms (1501 ms) / 442 MB | +3 ms / +11 MB |
