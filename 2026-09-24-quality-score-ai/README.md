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
- スコアが上がっていなければ棄却する。
- 採択時だけ commit し、棄却時は `git reset --hard` で戻す。

## 動かし方

`cccc` と `similarity-ts` が PATH に必要。

```sh
cargo +1.98.1 install --locked cccc-cli        # rustc >= 1.96 が必要
cargo install similarity-ts                     # または mizchi/similarity から --path で入れる

node src/score.ts fixtures/order-service        # 採点のみ (--json で JSON 出力)
node src/loop.ts --iterations 3 --model sonnet --judge
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

### 次に試すなら

- 採択条件を「スコアが下がらない、かつ judge が better」に変える
- judge の判定 (better / same / worse) もスコアに組み込み、定量指標と定性評価のハイブリッドにする
- 閾値 8 だと 1 回で飽和するので、より大きい実コードで試す
- 複数回実行してばらつきを見る
- similarity の検出漏れと誤検出のトレードオフを定量化する (閾値ごとに検出数を見る)
