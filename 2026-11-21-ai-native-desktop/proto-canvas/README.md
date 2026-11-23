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
| P4b 描画 (Odin + wgpu、**Odin の主な backend**) | `backend-odin-gpu/gpu.odin`, `src/backend/odin-gpu.ts` | 同じ display list を GPU のインスタンス描画 (draw call 1 回) で描く。ブラウザでは WebGPU、ネイティブでは wgpu-native ([../DECISIONS.md](../DECISIONS.md)) |
| P4b 描画 (Odin、ソフトウェア) | `backend-odin/raster.odin`, `src/backend/odin.ts` | 同じ display list を、Odin (js_wasm32) のソフトウェア描画で描く。文字は stb_truetype。GPU 版の比較の基準と、GPU が使えないときの代わり |
| a11y | `src/backend/aria.ts` | 意味ツリーを ARIA 付きの見えない DOM に写し、widget の位置に重ねる。スクリーンリーダーの操作は人の意味的な操作として shell に戻る。AI の提案と拒否は `aria-live` で読み上げる |
| P5 IME | `src/backend/canvas.ts` | 隠し textarea に OS の IME を任せる。preedit は表示だけで木に入れず、確定文字列を `replaceSelectedText` にする |
| veriss への lowering | `src/lower.ts` | View Tree → HTML + CSS + 文字の契約 ([VERISS.md](../VERISS.md)) |
| wasm コンポーネント | `component/`, `src/apps/component-app.ts` | Rust の Todo を WIT のコンポーネントにしたもの。import は `host.emit` だけ (std の WASI はホストが絞る) |
| 既存アプリ | `legacy/`, `src/legacy/` | GTK のアプリを AT-SPI で読み、意味ツリーに写して同じ権限モデルで操作する |
| MCP | `mcp/server.ts`, `src/mcp-link.ts` | AI 向けの口。読む・提案する・待つだけで、承認の tool は無い |
| プラットフォーム | `src/platform/`, `src/apps/manifests.ts`, `src/apps/launcher.ts` | アプリの manifest と registry、能力の確認付きのシステムのサービス (storage / clipboard / 通知 / アプリの起動 / 擬似端末)、許可の要求 (人だけが答える)、セッションの保存と復元 ([../PLATFORM.md](../PLATFORM.md)) |
| アプリの例: Terminal | `src/apps/terminal/`, `pty/server.ts` | libghostty-vt (ghostty-web の wasm) の端末。画面は意味ツリーの `log`。内蔵シェル、または `?pty=` でホストの bash |
| アプリのパッケージ | `src/platform/packages.ts`, `src/platform/worker-runner.ts`, `packages/` | `app.json` + `.wasm`。glue はホストが jco の `generate` で作る (パッケージの JS は動かさない)。能力 = import。コンポーネントは Worker で動き、応答しなければ止める |
| アプリの例: Calendar | `src/apps/calendar.ts` | 月の表を ARIA の table で出す。列は等幅の行 (`equal`) で揃える。予定は storage、今日の予定を通知 |
| テーマ | `src/theme.ts`, `www/index.html` | ライト / ダーク (システムに従う・明示)。lowering はダークを `prefers-color-scheme: dark` の色の宣言として出し、veriss が両方のコントラストを検査する |
| ワークスペース | `src/platform/services.ts`, `src/apps/agent-desk.ts`, `src/platform/platform.ts` | 人と agent のワークスペース (Alt+1 / Alt+2)。agent は自分のワークスペースで直接操作し、保存は重ねた層に溜まる。持ち込みは人が承認する |
| タイル | `src/tiling.ts`, `src/anim.ts`, `src/shell.ts` | 分割ツリー。窓のタイトルバーを掴んで運ぶと分割・入れ替えができる (下記)。境界はドラッグで動かせる。最小幅を下回ると「はみ出し」を出す |

右のパネルでは、次のことができる。

- テーマを選ぶ (システムに従う / ライト / ダーク)
- アプリを開く / 窓を閉じる (Alt+L でランチャー。タイトルバーの × でも閉じる)
- ワークスペースを切り替える (人 / agent)。agent の持ち込みの要求に答える
- アプリの許可の要求に答える (クリップボードの読み取り・通知・アプリの起動・ホストのシェル)。通知を見る
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
pnpm fonts && pnpm build && node e2e/font-shell.cjs   # ?font= で shell の計測を mizchi/font にしたアプリで、証明と描画の計測が一致するか
pnpm perf                # shell の 1 フレームと veriss の検証時間
pnpm build && pnpm e2e:mcp   # MCP サーバの通し確認 (MCP クライアント → サーバ → ブラウザの shell)
pnpm build:odin          # Odin の描画 backend を wasm にする (odin を PATH に置く)。?backend=odin&font=fonts/ipaexg.ttf で使う
pnpm fonts && pnpm build:odin && pnpm build && node e2e/odin.cjs   # Odin backend の確認 (Canvas 版との画素の差、文字幅、操作、描画時間)
pnpm build:odin-gpu      # Odin + wgpu の描画 backend を wasm にする。?backend=odin-gpu&font=fonts/ipaexg.ttf で使う (WebGPU が要る)
pnpm fonts && pnpm build:odin-gpu && pnpm build && node e2e/odin-gpu.cjs   # Odin + wgpu の確認 (GPU から読み戻した画素の比較など)
pnpm build:component     # Rust の Todo を WIT のコンポーネントにする (wasm32-wasip2 のターゲットが必要。component/todo.wasm はコミット済み)
pnpm atspi-bridge        # 既存の GTK アプリを AT-SPI で取り込む (sway / at-spi2-core / python3-pyatspi / gtk-3-examples が必要)
pnpm build && node e2e/perf-browser.cjs   # Chromium での 1 フレーム
pnpm build && pnpm e2e:terminal   # Terminal (libghostty-vt) の通し確認。ランチャー、キーボードと IME、色、許可、agent の提案、セッションの復元、ホストの bash
pnpm build && pnpm e2e:calendar   # Calendar の通し確認 (列の揃い、予定の追加、再読み込み、表のミラー)
pnpm build && pnpm e2e:theme      # テーマの切り替え (OS の設定、明示、保存、Canvas の画素)
pnpm build && pnpm e2e:packages   # パッケージ (WIT のコンポーネント) をパネルから入れ、開き、保存する
pnpm build && pnpm e2e:workspace  # agent のワークスペースで編集し、人がパネルで持ち込みを承認する
pnpm pty                 # ホストの擬似端末の橋。表示された URL を ?pty= に渡すと、Terminal の「ホストのシェルにつなぐ」が使える
```

### AI (MCP クライアント) からつなぐ

```sh
pnpm dev    # http://127.0.0.1:8123/?mcp=ws://127.0.0.1:8130 を開く
```

MCP クライアントには、`node mcp/server.ts` を stdio のサーバとして登録する。tool は `list_apps` / `list_installed` / `get_tree` / `get_layout` / `propose` / `wait_proposal` と、agent のワークスペースの `workspace_get_tree` / `workspace_launch` / `workspace_act` / `propose_merge` / `wait_merge` の 11 個。窓を開く・閉じるのも `propose` の layout の手順 (`launch` / `close`) で出す。承認の tool は無いので、提案は画面のパネルで人が承認する。

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

E2E は Playwright と Chromium を使う。この環境には入っているので package.json には入れていない (別の版を入れると、入っている Chromium を見つけられない)。無い環境では `pnpm add -D playwright && pnpm exec playwright install chromium` で入れる。

Node 24 以上が必要 (`.node-version`)。veriss がそれを要求する。この環境では `/opt/node24/bin` を PATH の先頭に置いて実行した。

## 結果

- 単体テスト 126 件 (うち veriss の検証 12 件、sway への写像 4 件)、E2E 12 件、レイアウト操作の E2E 14 件、Terminal の E2E 14 件、Calendar の E2E 6 件、テーマの E2E 6 件、パッケージの E2E 9 件、ワークスペースの E2E 4 件が通る (Node 24、Playwright の Chromium)
- E2E の IME は、CDP の `Input.imeSetComposition` で実際の composition イベントを起こして確かめた。preedit は木に入らず、確定した `卵` だけが値になる
- LLM の template の代わりに「カード」template を適用すると、件数が変わっても (3 件 → 5 件) 同じキャッシュに当たる。pin すると再読み込み後も残る
- 操作を落とす template は、既定レンダラに戻り、理由がパネルに出る
- agent の直接操作 (承認なし) は拒否される。sensitive な提案は自動承認されない。Settings アプリは、承認済みでも agent からの全削除を自分で拒否する

## 簡略化していること

- textbox のキャレットは常に末尾にある (選択範囲がない)
- 文字はすべて 1 行で、折り返さない。幅は Canvas の `measureText` で測る (veriss と統合するときは、shell 側の font metrics に移す。[VERISS.md](../VERISS.md))
- 空の list は、件数のある list と別のシグネチャになる
- LLM はまだ繋いでいない。template はパネルに貼る
