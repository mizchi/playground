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

## cccc の評価

`cccc-eval/README.md` にまとめた。要点:

- SonarSource の仕様どおりに数えた (11 ケース中 10 ケースが一致。残る 1 ケース、クロージャを親と別に採点する点は README に明記された意図的な仕様)。
- TypeScript コンパイラ (約 3.4 万関数) を 0.22 秒で処理した。ループの中で毎回呼んでもコストはない。
- 三項演算子や短絡評価に詰め込むハックには強い。
- 関数分割やクロージャへの退避で「関数ごとの最大値」は簡単に下がる。算術で分岐を消すと 0 になる。
- 分岐を減らしたのか散らしただけかは、cognitive 合計と cyclomatic 合計を並べると区別できる (run1: cognitive 71 → 28、cyclomatic 48 → 50)。

### 次に試すなら

- 棄却理由 (judge のコメント) を次の iteration のプロンプトに渡す。特に「この関数はこのままでよい」を伝えて、無理な 100 点狙いを止める
- スコアに cognitive 合計と関数数の増加を入れ、関数ごとの閾値だけでは満点にならないようにする
- 閾値 8 だと 1 回で飽和するので、より大きい実コードで試す
- 複数回実行してばらつきを見る
- similarity の検出漏れと誤検出のトレードオフを定量化する (閾値ごとに検出数を見る)
