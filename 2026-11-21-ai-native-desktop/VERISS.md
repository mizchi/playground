# veriss を組み込んだ安全なレイアウト (設計)

[mizchi/veriss](https://github.com/mizchi/veriss) は、CSS と box tree から、**viewport の幅の範囲全体にわたって**配置・アクセシビリティ・CSS の誤りを、描画せずに静的に検査するライブラリ (Z3 で確認する)。判定できないものは Unknown にし、成功には変えない。

このデスクトップでは次の 2 つが起きるので、相性がよい。

- **タイル型 WM は、窓をどんな幅にもする**: 「いくつかの幅で試した」では足りない。「この幅の範囲なら必ず崩れない」が欲しい
- **LLM が view を作る**: 生成物を人がレビューせずにキャッシュへ入れるには、機械的な保証が要る

V0 (lowering と 3 者照合) と V1 (既定の view の検証をテストに入れる) まで実装した。結果は下の「結果」にある。V2 以降はまだ設計。

## パイプラインの中の位置

```
LLM が template を出す
  → ① カタログの静的検査            (実装済み: catalog.ts validateTemplate)
  → ② 展開とカバレッジ検査          (実装済み: expand.ts)
  → ③ veriss に渡す形に lowering    (未実装: View Tree → box tree + CSS + boxes 契約)
  → ④ veriss: analyzeLayout + verifyLayoutReport (幅/高さの範囲 = タイルが取りうる範囲)
       verified → キャッシュに入れる (証明の記録を一緒に保存)
       violated → 反例 (幅の区間・witness・どの宣言が原因か) を LLM に返して作り直させる
       unknown  → 入れない。既定レンダラを使う
```

- 既定レンダラの template は、CI で一度だけ検証しておく (ビルド時のゲート)
- キャッシュの各エントリには、すでに `verification: unchecked | verified | violated | unknown` の欄がある (`cache.ts`)。今は常に `unchecked`

## lowering: View Tree → veriss の入力

View Tree には selector も cascade もない。ここが重要で、veriss の ADR-0014 / 0015 が Unknown と誤りの主な原因に挙げている S1 (selector と木の対応) と S2 (外から流れ込む値) が、**最初から起きない**。ADR-0015 の案 D (型付きの値) / 案 E (共有のレイアウト IR) に当たる。

| View Tree | veriss の入力 |
|---|---|
| `box` | `{tag:'div', classes:['v<n>']}` + `.v<n> { display:flex; flex-direction; gap; padding; box-sizing:border-box; flex-grow }`。1 ノード 1 クラスにする |
| `text` | `text: true` の box + `boxes['.v<n>']` に文字の契約。`/text/node` (mizchi/font) で font から幅の上界を出す |
| `widget` | 固定の寸法 (checkbox 18px など) を明示した CSS + ラベル文字の契約 |
| `list-of(...)` に畳んだ繰り返し | 0..N 件を列挙して検証する (veriss の構造の上限は 32)。N を超える件数は、list の容器に `overflowBlock: 'scroll'` の契約を付けて扱う |
| タイルが取りうる幅・高さ | `viewport: {min, max, height: {min, max}}` |

あわせて `integrity: true` (はみ出し・突き出し・文字の衝突・ターゲットの寸法) と `accessibility: true` (SC 2.5.8 のターゲット寸法、コントラスト、フォーカスの見え方など) を有効にする。色はトークンだけなので、コントラストはテーマごと (light / dark) に一度検証すれば済む。

## プロトコルへの追加 (必要になるもの)

- **P1 に文字列の長さの契約**: アプリが動的な文字列の上界を宣言する (例: `maxLength`、`charset`)。veriss の S3 (実行時にしか決まらない量) を契約で受けるため。宣言がない文字列は `nowrap` + clip で囲むか、Unknown にする
  - **実行時の強制**: 証明は宣言した上界の中でだけ成り立つ。shell は tree-update を受けたときに上界を確かめ、超えた値を含む subtree には検証済みの template を当てない (既定レンダラに戻し、その部分は `overflow: clip` で囲む。監査ログに「契約違反」として残す)。値を黙って切り詰めることはしない (アプリが言っていない文を作らない)
  - キャッシュのキーには上界 (`maxLength`・`charset`) を含める。上界が広がった更新は、狭い上界で検証した template に当たらない
- **P4b の `measure` を shell 側に移す**: 証明は font の計測値を前提にしている。実行時も**同じ font ファイルと同じ計測** (mizchi/font) を使わないと、証明が実際の画面と食い違う。backend は同じ font で字形を描くだけにする。証明の記録に font のハッシュを含める
- **実行時のレイアウトエンジンを CSS の flex と同じ意味にする**: 試作の `layout.ts` は簡易版。ADR-0015 の案 1 のとおり crater-layout (Taffy 由来の MoonBit、wasm component あり) に置き換え、veriss・crater・Chromium の 3 者で 1/64px 一致を照合する

## WM 側: 証明された範囲を守るタイル配置

- veriss の結果は「この幅・高さの範囲で崩れない」なので、窓は**検証した範囲** (`viewport` の幅と高さの min / max) の中でだけ、その template を使う。タイルの配置計算は、最小幅より狭い幅を割り当てない
  - 高さは窓の中身を縦にスクロールする容器にして検証する (`overflowBlock: 'scroll'`)。こうすると高さの下限は「スクロールできる」ことで満たされる
  - 範囲の外 (検証した最大より広い・高い窓、最小より低い窓) になったら、その窓では既定レンダラに戻すか、その範囲で検証し直す。範囲の外では証明に頼らない
  - 割り当てられない (窓が多すぎる) ときは、潰すのではなく、タブ化やスタック化に切り替える (i3 の tabbed のように)
- 幅の区間ごとに別の template を検証してキャッシュする手もある。タイルの幅で template を選べば、container query と同じことになる
- 試作の実行時バッジ (`252px 必要`) は、この仕組みが入れば出なくなるはず

## veriss をどこで動かすか

- veriss は JS (Node 24 以上、Z3 は wasm)。npm には出ていないので、`github:mizchi/veriss#<commit>&path:packages/veriss` で git 依存にしている (`proto-canvas/package.json`)
- ブラウザの試作: Web Worker で動かす (isle の playground が同じ構成)
- ネイティブ (Odin / mac): 別プロセスにして JSON Lines で呼ぶ。検証はキャッシュに入れるときだけで、描画の経路には乗らない。検証中は既定レンダラで表示し、プレビューに「検証中」と出す
- 証明の記録: `{status, viewport の範囲, 前提, template のハッシュ, font のハッシュ, veriss の commit, 実行時のレイアウトエンジンの識別子 (layout.ts の版、のちの crater-layout の版)}`。どれかが変わったら無効にする (V3 でエンジンを置き換えると、V2 の記録はすべて無効になる)

## 結果 (V0 / V1)

- `proto-canvas/src/lower.ts`: View Tree → HTML + CSS + 文字の契約。1 ノード 1 クラス
- `proto-canvas/e2e/veriss-diff.cjs`: 3 者照合。試作の窓 (Todo、Settings、カード template を当てた Todo) を lowering し、次の 3 つを幅 160〜700px で 1px ごとに比べる
  1. veriss の判定
  2. Chromium で配置した実測 (子が親の content box か viewport からはみ出すか)
  3. `layout.ts` の最小幅
- `proto-canvas/test/veriss.test.ts`: 既定の view について、`layout.ts` の最小幅以上の範囲で veriss と Z3 がともに verified、それ未満で violated になり、違反の上端が最小幅と一致することをテストする

| 窓 | veriss の最小幅 | Chromium の最小幅 | layout.ts の最小幅 | 見逃し / 過剰 |
|---|---|---|---|---|
| Todo (既定) | 252 | 252 | 252 | なし |
| Settings (既定) | 172 | 172 | 172 | なし |
| Todo (カード template) | 252 | 252 | 252 | なし |

px の固有幅を明示した lowering (`pxIntrinsic: true`) では、3 者が 1px 単位で一致した。明示しないと、どの窓も Unknown になる。原因は veriss の既知の制限 2 つで、どちらも Issue に報告した。

1. column の伸長しない item (`align-items: flex-start` など) は、文字の契約で max-content が決まっていても Unknown になる ([mizchi/veriss#6](https://github.com/mizchi/veriss/issues/6))
2. row の item で、中身のない箱に `min-width` を付けると `flex-basis: auto` が Unknown になる ([mizchi/veriss#7](https://github.com/mizchi/veriss/issues/7))

あわせて、`LayoutBoxContract` の型と README に、実行時が受け付ける `maxInlineSize` などが載っていないことも報告した ([mizchi/veriss#8](https://github.com/mizchi/veriss/issues/8))。最小の再現は `proto-canvas/e2e/veriss-repro.mjs` にある。

`layout.ts` は固有幅を決定的に出せるので、迂回の `pxIntrinsic` を使っても意味は変わらない。ただし、crater-layout に置き換えたあとや、LLM が任意の flex を書く場合には、迂回では済まない。

## 進め方

| 段階 | 内容 |
|---|---|
| V0 (済) | lowering (View Tree → box tree + CSS) を純粋な関数として作る。同じ CSS をブラウザの DOM で配置して、試作の `layout.ts` と差分照合する |
| V1 (済) | Node で veriss を動かし、既定の template をテストで検証する (Node 24) |
| V2 | ブラウザの Worker で、LLM の template をキャッシュに入れる前に検証する。violated の反例を LLM への再生成の指示に使う。**`verified` としてキャッシュに入れるのは、V0 の照合 (`layout.ts` とブラウザの DOM の一致) がその template で通ったときだけ** (実行時のエンジンが CSS の flex と違う意味なら、証明は実際の画面に当たらない)。通らなければ `unknown` のまま |
| V3 | 実行時のレイアウトを crater-layout (wasm) に置き換え、veriss・crater・Chromium の 3 者で照合する |
| V4 | WM が検証済みの範囲を守る (最小幅、タブ化、幅ごとの template) |

## 制約と未決

- 文字の折り返し: veriss は、動的な文字列 (文字種の範囲) を 1 行 (`nowrap`) だけで扱う。折り返すのは固定の文字列だけ。ラベルは 1 行 + 長さの契約にするのが現実的
- view で使う flex の機能は、veriss が証明できる部分集合 (row/column、wrap、grow/shrink、gap、align) に留める。外れたものは Unknown になり、キャッシュに入らない
- IME の preedit は木に入らないので、textbox は `overflow: clip/scroll` の契約で囲む必要がある
