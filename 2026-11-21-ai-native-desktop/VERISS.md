# veriss を組み込んだ安全なレイアウト (設計)

[mizchi/veriss](https://github.com/mizchi/veriss) は、CSS と box tree から、**viewport の幅の範囲全体にわたって**配置・アクセシビリティ・CSS の誤りを、描画せずに静的に検査するライブラリ (Z3 で確認する)。判定できないものは Unknown にし、成功には変えない。

このデスクトップでは次の 2 つが起きるので、相性がよい。

- **タイル型 WM は、窓をどんな幅にもする**: 「いくつかの幅で試した」では足りない。「この幅の範囲なら必ず崩れない」が欲しい
- **LLM が view を作る**: 生成物を人がレビューせずにキャッシュへ入れるには、機械的な保証が要る

以下はまだ設計で、実装していない。試作 (`proto-canvas/`) は、ここで静的に防ぎたい「はみ出し」を実行時に検出して表示している (タイルの境界を狭めると赤いバッジが出る)。

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
- **P4b の `measure` を shell 側に移す**: 証明は font の計測値を前提にしている。実行時も**同じ font ファイルと同じ計測** (mizchi/font) を使わないと、証明が実際の画面と食い違う。backend は同じ font で字形を描くだけにする。証明の記録に font のハッシュを含める
- **実行時のレイアウトエンジンを CSS の flex と同じ意味にする**: 試作の `layout.ts` は簡易版。ADR-0015 の案 1 のとおり crater-layout (Taffy 由来の MoonBit、wasm component あり) に置き換え、veriss・crater・Chromium の 3 者で 1/64px 一致を照合する

## WM 側: 証明された範囲を守るタイル配置

- veriss の結果は「この幅の範囲で崩れない」なので、窓は**検証済みの最小幅**を持つ。タイルの配置計算は、それより狭い幅を割り当てない
  - 割り当てられない (窓が多すぎる) ときは、潰すのではなく、タブ化やスタック化に切り替える (i3 の tabbed のように)
- 幅の区間ごとに別の template を検証してキャッシュする手もある。タイルの幅で template を選べば、container query と同じことになる
- 試作の実行時バッジ (`252px 必要`) は、この仕組みが入れば出なくなるはず

## veriss をどこで動かすか

- veriss は JS (Node 24 以上、Z3 は wasm)。npm には出ていないので、`github:mizchi/veriss#<commit>&path:packages/veriss` で git 依存にする
- ブラウザの試作: Web Worker で動かす (isle の playground が同じ構成)
- ネイティブ (Odin / mac): 別プロセスにして JSON Lines で呼ぶ。検証はキャッシュに入れるときだけで、描画の経路には乗らない。検証中は既定レンダラで表示し、プレビューに「検証中」と出す
- 証明の記録: `{status, viewport の範囲, 前提, template のハッシュ, font のハッシュ, veriss の commit}`。どれかが変わったら無効にする

## 進め方

| 段階 | 内容 |
|---|---|
| V0 | lowering (View Tree → box tree + CSS) を純粋な関数として作る。同じ CSS をブラウザの DOM で配置して、試作の `layout.ts` と差分照合する |
| V1 | Node で veriss を動かし、既定の template を CI で検証する (Node 24 が必要。この環境は 22) |
| V2 | ブラウザの Worker で、LLM の template をキャッシュに入れる前に検証する。violated の反例を LLM への再生成の指示に使う |
| V3 | 実行時のレイアウトを crater-layout (wasm) に置き換え、veriss・crater・Chromium の 3 者で照合する |
| V4 | WM が検証済みの範囲を守る (最小幅、タブ化、幅ごとの template) |

## 制約と未決

- 文字の折り返し: veriss は、動的な文字列 (文字種の範囲) を 1 行 (`nowrap`) だけで扱う。折り返すのは固定の文字列だけ。ラベルは 1 行 + 長さの契約にするのが現実的
- view で使う flex の機能は、veriss が証明できる部分集合 (row/column、wrap、grow/shrink、gap、align) に留める。外れたものは Unknown になり、キャッシュに入らない
- IME の preedit は木に入らないので、textbox は `overflow: clip/scroll` の契約で囲む必要がある
