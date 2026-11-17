#### synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer |
|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 100% (種類も 97%) | 名前 100% (種類も 100%) |
| workspace/symbol (上位 20 の一致) | 87% | 100% |
| definition (行き先の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) |
| hover (名前を含む) | 100% | 100% |
| references (数の一致) | 100% | 100% |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 11) | 100% (候補 3) |
| completion `na\|me` (同) | 97% (候補 25) | 91% (候補 145) |
| 時間の中央値 (ms) | defi 0.7, hove 0.7, comp 1.1, refe 3.8 | defi 0.3, hove 0.3, comp 3.4, refe 0.6 |

#### crustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer |
|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 99% (種類も 97%); 足りない extern "C" | 名前 100% (種類も 100%) |
| workspace/symbol (上位 20 の一致) | 73% | 100% |
| definition (行き先の一致) | 同じ 100% (答えた 78%) | 同じ 100% (答えた 78%) |
| hover (名前を含む) | 82% | 88% |
| references (数の一致) | 78% | 100% |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 48) | 100% (候補 26) |
| completion `na\|me` (同) | 80% (候補 7) | 68% (候補 85) |
| 時間の中央値 (ms) | defi 0.9, hove 0.9, comp 1.2, refe 16.3 | defi 0.9, hove 0.9, comp 5.7, refe 23.9 |
