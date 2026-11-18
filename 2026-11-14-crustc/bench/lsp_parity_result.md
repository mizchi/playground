#### synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer |
|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 100% (種類も 97%) | 名前 100% (種類も 100%) |
| workspace/symbol (上位 20 の一致) | 87% | 100% |
| definition (行き先の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) |
| hover (名前を含む) | 100% | 100% |
| references (数の一致) | 100% | 100% |
| documentHighlight (数の一致) | 100% | 100% |
| rename (書き換えの一致) | 同じ 100% (断ったのも同じ 0/0) | 同じ 100% (断ったのも同じ 0/0) |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 95%) |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 11) | 100% (候補 3) |
| completion `na\|me` (同) | 97% (候補 25) | 91% (候補 145) |
| 時間の中央値 (ms) | defi 0.7, hove 0.6, comp 1.1, refe 3.2, rena 3.6, sign 0.7 | defi 0.4, hove 0.6, comp 3.3, refe 0.8, rena 0.7, sign 0.3 |

#### crustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | crustc | rust-analyzer |
|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 99% (種類も 97%); 足りない extern "C" | 名前 100% (種類も 100%) |
| workspace/symbol (上位 20 の一致) | 73% | 100% |
| definition (行き先の一致) | 同じ 100% (答えた 78%) | 同じ 100% (答えた 78%) |
| hover (名前を含む) | 82% | 88% |
| references (数の一致) | 92% | 100% |
| documentHighlight (数の一致) | 100% | 100% |
| rename (書き換えの一致) | 同じ 94% (断ったのも同じ 23/23) | 同じ 100% (断ったのも同じ 23/23) |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 95%) | 同じ 100% (答えた 95%) |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 48) | 100% (候補 26) |
| completion `na\|me` (同) | 80% (候補 7) | 68% (候補 85) |
| 時間の中央値 (ms) | defi 1.0, hove 1.0, comp 1.3, refe 14.2, rena 1.3, sign 1.2 | defi 0.9, hove 0.9, comp 5.8, refe 25.0, rena 0.8, sign 0.5 |
