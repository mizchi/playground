# proto-canvas

[PROTOCOL.md](../PROTOCOL.md) のパイプラインを、ブラウザの Canvas で最小限に通した試作。

![提案のプレビュー](docs/proposal-preview.png)

## 何があるか

| 層 | ファイル | 内容 |
|---|---|---|
| P1 意味ツリー | `src/tree.ts`, `src/apps/*.ts` | Todo と Settings の 2 つのアプリ。view を知らず、ARIA 語彙のツリーを出すだけ |
| P2 意味的な入力 | `src/shell.ts` `dispatch` | 人間のクリックやキー入力は、hit test でノードを特定して action にする。agent は承認済みの提案の範囲でしか通らない。すべて監査ログに残る |
| P3 template | `src/catalog.ts` | json-render 風のカタログ。静的検査 (語彙・値の範囲・自己再帰の禁止) |
| キャッシュ | `src/signature.ts`, `src/cache.ts` | 構造シグネチャ (値と件数に依存しない) → template。ephemeral / pinned / rejected |
| 展開 | `src/expand.ts` | 決定的に展開する。操作できるノードの抜け・重複、role との不一致があれば、その template を捨てて既定レンダラに戻す |
| レイアウト | `src/layout.ts` | portable な flex の部分集合の簡易版 (後で crater-layout に置き換え、veriss で証明する) |
| P4b 描画 | `src/render.ts`, `src/backend/canvas.ts` | display list → Canvas 2D |
| a11y | `src/backend/aria.ts` | 意味ツリーを ARIA 付きの見えない DOM に写し、widget の位置に重ねる。スクリーンリーダーの操作は人の意味的な操作として shell に戻る。AI の提案と拒否は `aria-live` で読み上げる |
| P5 IME | `src/backend/canvas.ts` | 隠し textarea に OS の IME を任せる。preedit は表示だけで木に入れず、確定文字列を `replaceSelectedText` にする |
| veriss への lowering | `src/lower.ts` | View Tree → HTML + CSS + 文字の契約 ([VERISS.md](../VERISS.md)) |
| タイル | `src/tiling.ts`, `src/anim.ts`, `src/shell.ts` | 分割ツリー。窓のタイトルバーを掴んで運ぶと分割・入れ替えができる (下記)。境界はドラッグで動かせる。最小幅を下回ると「はみ出し」を出す |

右のパネルでは、次のことができる。

- agent として提案する (ゴースト表示 → 承認 / 却下)
- template を貼って適用・pin・reject する (LLM の出力の代わり)
- agent から見える意味ツリーと監査ログを見る

ブラウザのコンソールからは `agent.getTree()` と `agent.propose(intent, steps)` が使える。どちらも MCP の tool にそのまま写せる形にしてある。

## レイアウトの操作

![ドロップ先のプレビュー](docs/dnd-preview.png)

窓のタイトルバー (左の ⠿) かタブを掴んで運ぶ。VS Code・Windows のスナップ・タイル型 WM の操作を組み合わせた。

| 落とす場所 / 操作 | 結果 |
|---|---|
| 別のペーンの端 (各辺から 28% の帯) | そのペーンをその向きに 2 分割し、運んだ窓を入れる |
| 別のペーンの中央 | 2 つの窓を入れ替える |
| 別のペーンのタイトルバー | タブにまとめる。タブを押すと切り替わり、タブを掴むとその窓だけを運べる (端へ運べばグループから出る) |
| ペーンの境界 (中心線から 12px) | 間に差し込む。両側が 2/3 に縮み、差し込んだ窓が 1/3 を取る |
| ワークスペースの外周 (28px の帯) | 全体をその向きに 2 分割し、端に置く |
| ドラッグ中に下に現れる受け口 | 最小化してドックに入れる。ドックの札を押すと、札の位置から一番大きいペーンを分けて戻る |
| タイトルバーのダブルクリック / Alt+Enter | ズーム (全体に広げる)。もう一度、または Esc で戻る |
| 境界のダブルクリック / Alt+E | 均等にする |
| Alt+方向キー | 向きの隣の窓と入れ替える (AeroSpace の move) |
| Alt+M | 最小化 |
| Escape | ドラッグを取り消す |
| ホイール | 窓の中身をスクロールする (Tab で移動すると、フォーカスした要素を見える位置まで送る) |

- **プレビュー**: ドロップ先は、青い半透明の矩形と「右に分割」などのラベルで示す。
  - プレビューは、ポインタの位置から広がって現れる。候補が変わると、少し弾むばねで次の候補へ滑る
  - パネルの「ライブプレビュー」をオンにすると、離す前からほかの窓が実際に場所を空けて動く。境界への差し込みは、常にこの表示になる
- **アニメーション**
  - 離すと、運んだ窓はプレビューの位置から、ほかの窓は元の位置から、臨界減衰のばねで新しい位置へ動く。中身は各フレームでその時点の幅に合わせて配置し直す
  - 最小化した窓は、ドックの札へ縮みながら消える
- **安全性**: 操作した結果、どれかの窓が自分の最小幅を下回る場合は、プレビューが赤くなる。理由が出て、離しても何も変わらない。最小幅は veriss で証明している下限と同じ値 ([VERISS.md](../VERISS.md))。最後の 1 枚は最小化できない
- **agent**: agent は人のドラッグを真似ない。`{kind: 'layout', op: {type: 'move' | 'zoom' | 'minimize' | 'restore' | 'equalize', ...}}` を提案する。
  - 提案すると、実行後の配置が破線で示される。承認すると、人の操作と同じアニメーションで動く
  - 最小幅を下回る提案は、提案の時点で拒否する。レイアウトの操作も監査ログに残る
  - パネルの「例: レイアウト」で試せる
- アニメーションは時刻を外から渡して進めるので、単体テストで決定的に確かめられる (`test/tiling.test.ts`、`test/layout-patterns.test.ts`)

## 動かし方

```sh
pnpm install
pnpm dev          # http://127.0.0.1:8123
pnpm test         # 単体テスト (node --test、型ストリップで .ts をそのまま実行)
pnpm typecheck
pnpm build && pnpm e2e   # Chromium で通し確認。--shots で screenshots/ に保存
pnpm build && pnpm e2e:a11y  # スクリーンリーダー向けのミラー DOM (アクセシビリティツリー、操作、読み上げ、axe-core)
pnpm build && pnpm e2e:dnd   # レイアウトの操作 14 項目。--video で screenshots/dnd.webm と dnd.gif を保存
pnpm build && pnpm veriss-diff   # veriss・Chromium・layout.ts の 3 者照合 (../VERISS.md)
pnpm veriss-repro        # veriss に報告した Unknown の最小再現
pnpm sway-bridge         # LayoutOp を実際の sway に写して比べる (下記)
pnpm font-diff           # 実際のフォント (IPAex ゴシック) で、veriss の計測・Chromium・layout.ts の幅を比べる
pnpm perf                # shell の 1 フレームと veriss の検証時間
pnpm build && node e2e/perf-browser.cjs   # Chromium での 1 フレーム
```

### sway への写像を試す

```sh
apt-get install -y --no-install-recommends sway foot fonts-dejavu-core
mkdir -p /tmp/sway/run && chmod 700 /tmp/sway/run
printf 'output HEADLESS-1 resolution 1400x800\ndefault_border none\ngaps inner 0\n' > /tmp/sway/config
XDG_RUNTIME_DIR=/tmp/sway/run WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman sway -c /tmp/sway/config &
export XDG_RUNTIME_DIR=/tmp/sway/run WAYLAND_DISPLAY=wayland-1 SWAYSOCK=$(ls /tmp/sway/run/sway-ipc.*.sock)
for a in todo settings notes; do foot --app-id=$a sh -c 'sleep 100000' & done
pnpm sway-bridge
```

結果は [VALIDATION.md](../VALIDATION.md) の 5a。

Node 24 以上が必要 (`.node-version`)。veriss がそれを要求する。この環境では `/opt/node24/bin` を PATH の先頭に置いて実行した。

## 結果

- 単体テスト 48 件 (うち veriss の検証 4 件、sway への写像 4 件)、E2E 11 件、レイアウト操作の E2E 14 件が通る (Node 24、Playwright の Chromium)
- E2E の IME は、CDP の `Input.imeSetComposition` で実際の composition イベントを起こして確かめた。preedit は木に入らず、確定した `卵` だけが値になる
- LLM の template の代わりに「カード」template を適用すると、件数が変わっても (3 件 → 5 件) 同じキャッシュに当たる。pin すると再読み込み後も残る
- 操作を落とす template は、既定レンダラに戻り、理由がパネルに出る
- agent の直接操作 (承認なし) は拒否される。sensitive な提案は自動承認されない。Settings アプリは、承認済みでも agent からの全削除を自分で拒否する

## 簡略化していること

- textbox のキャレットは常に末尾にある (選択範囲がない)
- 文字はすべて 1 行で、折り返さない。幅は Canvas の `measureText` で測る (veriss と統合するときは、shell 側の font metrics に移す。[VERISS.md](../VERISS.md))
- 空の list は、件数のある list と別のシグネチャになる
- LLM はまだ繋いでいない。template はパネルに貼る
