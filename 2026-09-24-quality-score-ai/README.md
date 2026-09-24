# 2026-09-24-quality-score-ai

コード品質の定量指標をツールで取って採点し、その採点を報酬にして AI (Claude) にリファクタさせる。
このループを自動化できるかを検証した。

- [moznion/cccc](https://github.com/moznion/cccc): 関数ごとの Cognitive / Cyclomatic Complexity を測る
- [mizchi/similarity](https://github.com/mizchi/similarity) (`similarity-ts`): AST ベースで似ている関数を検出する
- `claude -p`: ヘッドレスの Claude Code でリファクタとレビューを行う

## 仕組み

```
fixture をコピー → 採点 ─┬→ claude -p にレポートを渡してリファクタ
                        │         ↓
                        │     再採点 + ゲート
                        │         ↓
                        └── 採択 (commit) / 棄却 (reset)   × N 回
                                  ↓
                          --judge: 差分を別プロンプトで定性レビュー
```

### スコア (`src/score.ts`)

0〜100 点。重みと閾値はとりあえず決めたもので、`CONFIG` で変えられる。

| 要素 | 定義 |
| --- | --- |
| complexity | `penalty = Σ max(0, cognitive-8) + 0.5·Σ max(0, cyclomatic-8)`、`100·exp(-penalty/20)` |
| duplication | similarity-ts の各グループについて `類似度 × (合計行数 − 最大メンバーの行数)` を足し、`100·(1 − 重複行 / LOC)` |
| total | テストが通れば `0.5·complexity + 0.5·duplication`、通らなければ 0 |

### ゲート (`src/loop.ts`)

- テストファイルのハッシュが変わっていたら棄却する (テストを書き換えて通す行為を防ぐ)。
- テストが通らなければ棄却する。
- `--accept score` (デフォルト): スコアが上がっていなければ棄却する。
- `--accept judge`: スコアが下がる、または別プロンプトの judge が差分を `better` と判定しなければ棄却する。
  - `--judges N` で judge を N 人並列に走らせ、過半数が `better` なら採択する。
- `--best-of N`: 1 iteration で N 個の候補を並列に生成する (各候補は現在の採択状態の clone 上で作業する)。
  - 各候補にゲートと judge をかける。「judge の better 票が多い → スコアが高い → cognitive 合計が小さい」の順で最良の候補を選ぶ。
  - 最良の候補も過半数の better を得ていなければ、その iteration は全棄却になる。
- `--feedback`: 全候補が棄却された iteration について、全候補の棄却理由と judge のコメントを次の iteration のリファクタプロンプトに追加する (直近 2 iteration 分)。あわせて「読みやすい関数は点数が低くてもそのままでよい、変更なしも可」と指示する。
- 採択時だけ commit し、棄却時は `git reset --hard` で戻す。

## 動かし方

`cccc` と `similarity-ts` が PATH に必要。

```sh
cargo +1.98.1 install --locked cccc-cli        # rustc >= 1.96 が必要
cargo install similarity-ts                     # または mizchi/similarity から --path で入れる

node src/score.ts fixtures/order-service        # 採点のみ (--json で JSON 出力)
node src/loop.ts --iterations 3 --model sonnet --judge                 # run1: 点数が上がれば採用
node src/loop.ts --iterations 4 --model sonnet --judge --accept judge  # run2: 点数が下がらず judge が better なら採用
node src/loop.ts --iterations 4 --judge --accept judge --judges 3      # run3: judge 3 人を並列で走らせ、過半数が better なら採用
node src/loop.ts --iterations 4 --judge --accept judge --judges 3 --feedback  # run4: 棄却理由を次のプロンプトに渡す
node src/loop.ts --iterations 3 --judge --accept judge --judges 3 --best-of 3 # run5: 候補を 3 つ並列に作って最良を採る
node src/loop.ts --iterations 3 --judge --accept judge --judges 3 --best-of 3 --feedback  # run6: run5 + 全棄却時のフィードバック
```

実行結果は `runs/<timestamp>/` に出る (gitignore 済み)。代表的な結果は `results/` に置いた。

## 題材

`fixtures/order-service/`: わざと汚く書いた注文計算モジュールとテスト。

- `calculateShipping`, `validateOrder` は深いネストの if-else
- `calculateMemberDiscount` / `calculateGoldDiscount` はほぼ同じコード
- `report.ts` の `summarizeBy{Category,Country,Tier}` は 3 つともほぼ同じコード

## 結果 (run1: sonnet, 3 iterations)

| iteration | score | 結果 |
| --- | --- | --- |
| baseline | 51.6 (complexity 10.5 / duplication 92.6) | |
| 1 | 100 | 採択: order.ts を関数分割し、テーブル駆動にした |
| 2 | 100 | 棄却: report.ts の 3 重複を正しく共通化したが、スコアが上がらない |
| 3 | 100 | 棄却: 変更なし |

詳細は `results/run1/` (`refactor.diff`, `judge.md`, `claude-*.log`, `summary.json`) を参照。

### わかったこと

1. **ループ自体は自動化できる。** 採点 → レポート → `claude -p` → テストで検証 → 採択 / 棄却 の流れは、100 行程度のスクリプトで回った。1 回で 51.6 → 100 点になり、テストも通ったままだった。
2. **指標の検出漏れが、そのまま改善の検出漏れになる。** similarity-ts のデフォルト設定 (`--threshold 0.8`) は `report.ts` の 3 つの重複を拾わなかった。
   - `--no-size-penalty -m 1` にすると拾えるが、今度は `calculateShipping` と `validateOrder` のような無関係な関数まで 90% 前後で似ていると判定する誤検出が出る。
   - 小さい関数に対しては、閾値とペナルティの調整が難しい。
3. **スコアが頭打ちになると、正しい改善も棄却される。** iteration 2 で AI は報告にない重複を自分で見つけて直したが、スコアは 100 のままなので捨てられた。
   - 「スコアが上がった」を唯一の採択条件にすると、指標が見ていない改善を取りこぼす。
   - 「スコアが下がらず、テストも通り、judge が better と判定」などの複合条件の方がよさそう。
4. **指標の改善は、可読性の改善を保証しない。** `--judge` の判定は `same` だった。
   - 良い点: バリデーションの分割と、`calculateTotal` の段階分けは明確な改善と評価された。
   - 悪い点: 割引・送料のテーブル駆動化は「重複指標を下げるための過剰な抽象化」と指摘された。`Infinity` の番兵値や `!` による non-null アサーションも挙げられた。
   - ただし judge 自体も主観的で、テーブル駆動化は妥当と見る人もいるはず。
5. **採点のゲートは必要。** テストファイルのハッシュ検査と、テスト失敗時にスコアを 0 にする仕組みがないと、テストや機能を削ってスコアを上げる方向に流れうる。今回の run では、そういった挙動は起きなかった。

## 結果 (run2: sonnet, 4 iterations, `--accept judge`, 2 並列)

run1 の問題 3 を受けて、採択条件を「スコアが下がらない、かつ judge が better と判定」に変えた。

| iteration | run2a | run2b |
| --- | --- | --- |
| 1 | 100 / same → 棄却 | 100 / same → 棄却 |
| 2 | 100 / better → **採択** | 100 / same → 棄却 |
| 3 | 変更なし | 100 / better → **採択** |
| 4 | 変更なし | 100 / better → **採択** (report.ts の 3 重複を共通化) |

各 iteration 後の cognitive 合計 / 最大 (ベースラインは 89 / 28):

- run2a: 45/6 (棄却), 48/6 (採択)
- run2b: 46/6 (棄却), 46/6 (棄却), 43/6 (採択), 29/5 (採択)

詳細は `results/run2a/`, `results/run2b/` を参照。

### run2 でわかったこと

6. **judge を採択条件に入れると、スコアが頭打ちでも正しい改善が採択される。** run2b の iteration 4 では、run1 で棄却された `report.ts` の共通化が 100 → 100 のまま採択された (judge は「本物の重複除去で、ハックではない」と判定)。
7. **judge は厳しいが、判定がぶれる。** judge の判定 6 回のうち 3 回が `same` で、棄却された。
   - 棄却された差分と採択された差分は、どちらも「割引率・送料のテーブル化 + validate の分割」で、中身はほぼ同じ。
   - それでも、ある回はテーブル化を「ハック臭い」と言い、別の回は「本物の改善」と言う。
   - 1 回の LLM 判定を唯一のゲートにするのは不安定。多数決や、判定理由に基づく採点基準の固定が必要。
8. **棄却するたびにベースラインからやり直すので効率が悪い。** 棄却された試行の judge のコメントを次の試行のプロンプトに渡せば、同じ指摘を繰り返さずに済むはず (未実装)。
9. **AI は報告されていない問題を見つけることもあれば、見つけないこともある。** `report.ts` の重複は、run2b では見つけたが、run2a では見つけなかった (iteration 3, 4 は「変更なし」)。指標の検出漏れを AI が補う保証はない。

## 結果 (run3: `--accept judge --judges 3`, 2 並列)

run2 の問題 7 (judge の判定がぶれる) を受けて、judge を 3 人にして多数決にした。

| iteration | run3c | run3d |
| --- | --- | --- |
| 1 | 100 / worse, worse, same → 棄却 | 100 / better, better, same → **採択** |
| 2 | 95.2 / same, better, better → **採択** | 100 / same, same, worse → 棄却 |
| 3 | 100 / worse, worse, worse → 棄却 | 100 / same, same, same → 棄却 |
| 4 | 100 / worse, same, worse → 棄却 | 100 / better, better, better → **採択** (report.ts の共通化) |
| 最終 | **95.2** | 100 |

詳細は `results/run3c/`, `results/run3d/` を参照。

### run3 でわかったこと

10. **多数決にすると、judge が指標に拒否権を持つようになる。** run3c は 95.2 で止まった。
    - 残った `baseShippingCost` は cognitive 10 (閾値 8 を超える)。
    - iteration 3, 4 で AI はこれを「テーブル + `find` + `Infinity` 番兵 + `!`」に書き換えて 100 点にしたが、judge は 3 人とも `worse` と判定した。理由は「平坦な if の並びの方が読みやすく、`!` は隠れた不変条件に依存している」。
    - スコアだけで判定した run1 と、judge 1 人の run2b では、これと同じパターンが採択されていた。

    ```ts
    // 採択されたコード (cognitive 10、スコアは減点される)
    function baseShippingCost(weight: number, country: string): number {
      const domestic = country === "JP";
      if (weight < 1) return domestic ? 300 : 1500;
      if (weight < 5) return domestic ? 600 : 3000;
      if (weight < 20) return domestic ? 1200 : 6000;
      return domestic ? 3000 : 15000;
    }
    ```

11. **指標と judge が食い違う典型例は「平坦な分岐の並び」だった。** cognitive は `if` と三項演算子を 1 つずつ数えるので、ネストのない表のような分岐でも閾値を超える。これを無理に消そうとすると過剰な抽象化になる。
    - 「スコア 100」を目標にし続けると、この種の悪化した変更を AI に求め続けることになる。
    - 閾値を上げるか、judge が棄却した理由を次のプロンプトに渡して「この関数はこのままでよい」と伝える仕組みが必要。
12. **多数決でも、判定のぶれは残る。**
    - run3d の iteration 3 (same ×3) と iteration 4 (better ×3) は、どちらも `summarizeBy(orders, keyFor)` への共通化で、cognitive 合計も同じ 32。
    - 違いはコールバックの引数の設計 (使わない引数 `_order` があるかどうか) などの細部だけ。
    - 多数決は同じ差分に対するぶれを減らすが、差分ごとの細かい差には敏感なまま。これは正しい挙動とも言える。
13. **judge の人数分、コストが増える。** 1 iteration あたりの `claude -p` 呼び出しは 1 (リファクタ) + 3 (judge)。3 人は並列で実行したので、壁時計時間はほとんど増えなかった。

## 結果 (run4: `--accept judge --judges 3 --feedback`, 2 並列)

run3 の問題 11 (無理な 100 点狙い) を受けて、棄却理由を次のプロンプトに渡した。

| iteration | run4e | run4f |
| --- | --- | --- |
| 1 | 100 / better, same, same → 棄却 | 100 / better ×3 → **採択** |
| 2 | 100 / worse ×3 → 棄却 | 100 / better ×3 → **採択** (report.ts の共通化) |
| 3 | 92 / same ×3 → 棄却 | 変更なし |
| 4 | 68.7 / better ×3 → **採択** | 変更なし |
| 最終 | **68.7** (cognitive 合計 72 / 最大 21) | **100** (cognitive 合計 32 / 最大 5) |

詳細は `results/run4e/`, `results/run4f/` を参照 (`prompt-*.md` に、実際に渡したプロンプトがある)。

### run4 でわかったこと

14. **フィードバックは確かに効く。** run4e の iteration 4 のログで、AI は棄却理由を引用して方針を変えていた。
    - 「attempt 1 では 3 人とも、位置引数のヘルパー `tierDiscountRate(years, 0.1, 0.05, 0.02)` を有害と指摘した」
    - 「attempt 2 では、呼び出し元が 1 つしかないヘルパーへの分割は間接参照を増やすだけと言われた」
    - そのうえで、`round2` / `subtotalOf` / validate の分割という「3 人とも本物の改善と認めた部分」だけを残した。結果は全員一致の `better` で採択された。
15. **ただし効きすぎて保守的になった。** スコアは 100 → 100 → 92 → 68.7 と試行ごとに下がった。
    - `calculateShipping` (cognitive 21) と、割引関数の重複 (98%) には手を付けずに終わった。
    - run3c では、読みやすく cognitive 10 まで下げた `baseShippingCost` が採択されていた。つまり「悪い直し方」への指摘が「直さない」に一般化された。
    - 棄却理由を渡すなら、「何がダメだったか」だけでなく「どこまでは良かったか」(部分的に採択できた変更) も渡す必要がある。
16. **同じ設定でも、最初の 1 手で結果が大きく変わる。** run4f は 1 回目から全員一致の `better` で、フィードバックは一度も使われないまま 100 点 / cognitive 最大 5 に到達した。これは全 run の中で最も良い結果。run4e との差は、最初の試行の出来だけ。
    - 現状のループは「最初の 1 手の当たり外れ」に大きく依存している。
    - 1 iteration で複数の候補を並列に生成し、最も良いものを採る (best-of-N) と安定するはず。

## 結果 (run5: `--accept judge --judges 3 --best-of 3`, 2 並列, 3 iterations)

run4 の問題 16 (最初の 1 手の当たり外れ) を受けて、1 iteration で候補を 3 つ並列に作り、最良のものを採るようにした。フィードバックは使っていない。

| iteration | run5g の候補 (スコア / 票) | run5h の候補 (スコア / 票) |
| --- | --- | --- |
| 1 | #1 100 worse,same,same / #2 100 same,same,better / **#3 100 better,same,better → 採択** | #1 same,worse,worse / #2 same,same,better / #3 same,better,same → 全棄却 |
| 2 | #1 変更なし / #2 変更なし / **#3 100 better,worse,better → 採択** (report.ts) | #1 better,same,better / #2 better,same,same / **#3 better ×3 → 採択** |
| 3 | 全候補が変更なし | **#1 better ×3 → 採択** (report.ts) / #2 better ×3 / #3 better,same,better |
| 最終 | 100 (cognitive 合計 38 / 最大 6) | 100 (cognitive 合計 32 / 最大 5) |

どちらも最終 judge は `better` だった。`report.ts` の重複も解消し、テーブル + `find` + `Infinity` 番兵のパターンは最終コードに残っていない。
詳細は `results/run5g/`, `results/run5h/` を参照。

コスト (1 run あたり):

| 設定 | リファクタ呼び出し | judge 呼び出し | 壁時計時間 |
| --- | --- | --- | --- |
| run3c (judge 3 人) | 4 | 12 | 4 分 48 秒 |
| run4e (judge 3 人 + feedback) | 4 | 12 | 8 分 10 秒 |
| run5g (best-of-3) | 9 | 12 | 3 分 32 秒 |
| run5h (best-of-3) | 9 | 27 | 5 分 34 秒 |

### run5 でわかったこと

17. **best-of-N で結果が安定した。** 2 run とも 100 点に到達し、最終 judge も `better` だった。
    - 同じ judge 3 人の条件でも、run3 は 95.2 / 100、run4 は 68.7 / 100 と結果が割れていた。
    - 判定を受けた候補 13 個のうち、過半数の better を得たのは 7 個 (約 54%)。候補 1 つあたりの合格率が 5 割なら、3 つのうち 1 つ以上が合格する確率は約 88%。
    - 実際、変更のあった 5 iteration のうち 4 回で合格候補が出た。
18. **候補を増やすと、指標の見落としも拾いやすくなる。** run5g の iteration 2 では、3 候補のうち 2 つが「もう直すところはない」として変更しなかった。残る 1 つだけが `report.ts` の重複 (similarity-ts が検出しないもの) を見つけて直した。1 候補だけだと、run2a のようにここで止まっていた可能性がある。
19. **並列化すれば、壁時計時間はほとんど増えない。** 呼び出し回数は 2〜3 倍になるが、並列なので時間は run3 と同程度だった。むしろ、棄却 → やり直しの直列ループが減るぶん短くなることもある (run4e の 8 分に対し、run5g は 3.5 分)。
20. **それでも全棄却は起こる。** run5h の iteration 1 は 3 候補とも過半数を取れなかった。best-of-N と棄却理由のフィードバックを組み合わせる余地がある。

## 結果 (run6: `--best-of 3 --feedback`, 2 並列, 3 iterations)

run5 に、全候補が棄却されたときだけ理由を次のプロンプトに渡すフィードバックを組み合わせた。

| iteration | run6i | run6j |
| --- | --- | --- |
| 1 | **#1 better,same,better → 採択** / #2 worse,worse,better / #3 worse,same,same | #1 better,same,same / #2 same,same,better / #3 better,same,same → 全棄却 |
| 2 | #1 変更なし / **#2 better,worse,better → 採択** (report.ts) / #3 変更なし | (フィードバックあり) #1 worse,same,same / **#2 95.2 better,better,same → 採択** / #3 same,better,same |
| 3 | 全候補が変更なし | (フィードバックあり) 3 候補とも 100 点を狙い、worse / worse / worse → 全棄却 |
| 最終 | 100 (最終 judge: same) | 95.2 (最終 judge: better) |

詳細は `results/run6i/`, `results/run6j/` を参照。

### run6 でわかったこと

21. **フィードバックはほとんど発動しない。** best-of-3 では全棄却が起きにくいため、run6i では一度も使われなかった (実質 run5 と同じ条件)。
22. **発動すると、run3c / run4e と同じ「保守的な着地」になった。** run6j はフィードバック後に 95.2 で採択された。
    - 残ったのは、平坦な if の並びの `baseShippingRate` (cognitive 10)。
    - iteration 3 では、3 候補ともこれを消して 100 点を狙い、ほぼ全員一致の `worse` で棄却された。
    - レポートに載っている関数に注意が集中するため、3 候補とも `report.ts` の重複には気付かなかった。
23. **各ステップの `better` が、全体の `better` になるとは限らない。** run6i は 2 ステップとも過半数の `better` で採択されたが、累積差分に対する最終 judge は `same` (細かく分割しすぎという指摘) だった。ただし、最終 judge は 1 人なので、これもぶれの範囲かもしれない。
24. **この題材・この規模では、best-of-N 単独とフィードバック併用の差ははっきりしない。**

## 全 run のまとめ

| run | 設定 | 最終スコア | cognitive 合計 / 最大 | report.ts の重複 | 最終 judge |
| --- | --- | --- | --- | --- | --- |
| baseline | - | 51.6 | 89 / 28 | 残る | - |
| run1 | スコアのみ | 100 | - | 棄却された | same |
| run2a | judge 1 人 | 100 | 48 / 6 | 残る | same |
| run2b | judge 1 人 | 100 | 29 / 5 | 解消 | same |
| run3c | judge 3 人 | 95.2 | 55 / 10 | 残る | better |
| run3d | judge 3 人 | 100 | 32 / 5 | 解消 | same |
| run4e | judge 3 人 + feedback | 68.7 | 72 / 21 | 残る | better |
| run4f | judge 3 人 + feedback | 100 | 32 / 5 | 解消 | better |
| run5g | best-of-3 | 100 | 38 / 6 | 解消 | better |
| run5h | best-of-3 | 100 | 32 / 5 | 解消 | better |
| run6i | best-of-3 + feedback | 100 | 35 / 5 | 解消 | same |
| run6j | best-of-3 + feedback | 95.2 | 57 / 10 | 残る | better |

(run1 の cognitive は、`cccc-eval/README.md` のとおり order.ts だけで 71 / 28 → 28 / 5。最終 judge は、各 run の最後に累積差分を 1 人で判定したもの。)

### 結論

- **採点 → AI リファクタ → ゲートのループ自体は、簡単に自動化できる。** 定量指標は、AI に「どこを直すべきか」を示すのに有効だった。
- **定量指標だけを報酬にすると、指標の穴 (関数分割・テーブル化・検出漏れ・100 点での頭打ち) にそのまま沿って最適化される。**
- **judge (LLM による定性評価) を採択ゲートに入れると、指標を悪用した変更を止められる。** ただし、1 人だとぶれるので、3 人の多数決が最低ライン。
- **最も安定したのは best-of-N。** 同じ judge でも、候補を増やすと「最初の 1 手の当たり外れ」が平均化される。並列にすれば時間もほとんど増えない。
- **棄却理由のフィードバックは効くが、保守的な方向に効きやすい。**「悪い直し方」の指摘が「直さない」に一般化され、95.2 や 68.7 で止まることがあった。
- **スコア 100 は、ゴールとして不適切。** 平坦な分岐のように「指標上は減点されるが、読みやすいコード」が存在する。スコアは改善箇所の候補リストとして使い、採否は judge が決める、という役割分担が良さそう。
- 今回の検証は 1 題材 (約 190 行)、モデル 1 種 (sonnet)、各設定 2 run の小さな実験。より大きな実コードで試す必要がある。

## cccc の評価

`cccc-eval/README.md` にまとめた。要点:

- SonarSource の仕様どおりに数えた (11 ケース中 10 ケースが一致。残る 1 ケース、クロージャを親と別に採点する点は README に明記された意図的な仕様)。
- TypeScript コンパイラ (約 3.4 万関数) を 0.22 秒で処理した。ループの中で毎回呼んでもコストはない。
- 三項演算子や短絡評価に詰め込むハックには強い。
- 関数分割やクロージャへの退避で「関数ごとの最大値」は簡単に下がる。算術で分岐を消すと 0 になる。
- 分岐を減らしたのか散らしただけかは、cognitive 合計と cyclomatic 合計を並べると区別できる (run1: cognitive 71 → 28、cyclomatic 48 → 50)。

### 次に試すなら

- 棄却された差分のうち judge が良いと言った部分だけを残す、部分採択の仕組みを入れる
- 平坦な分岐のように「読みやすいので残す」と judge が判断した関数を、次のレポートから除外する (無理な 100 点狙いを止める)
- iteration 数を増やして、保守的になった後に改善が再開するかを見る
- スコアに cognitive 合計と関数数の増加を入れ、関数ごとの閾値だけでは満点にならないようにする
- 閾値 8 だと 1 回で飽和するので、より大きい実コードで試す
- 複数回実行してばらつきを見る
- similarity の検出漏れと誤検出のトレードオフを定量化する (閾値ごとに検出数を見る)
