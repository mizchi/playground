# プロトコル案: a11y ツリーを正とする UI パイプライン

README の「宣言的 UI ツリー」を一歩進め、**アクセシビリティツリーを唯一の正 (model)** とし、見た目 (view) はそこからの射影とする。

## パイプライン

```
 app ──(P1 TreeUpdate)──▶ ① Semantic Tree (a11y, 正)
                              │
                              ▼
                         ② View 展開
                            ├ 既定レンダラ: role → widget の決定的マッピング
                            └ LLM: カタログ内の View Template を生成
                              │  検証 (下記の不変条件)
                              ▼
                         ③ Template Cache (構造シグネチャ → template)
                              │  好みのものを pin
                              ▼
                         ④ Expand: tree + template → View Tree → layout
                              │
                 ┌────────────┴─────────────┐
                 ▼                          ▼
       (P4a View Tree)              (P4b Display List)
       widget 系 backend              低レベル backend
       gpui, DOM                      canvas, wgpu, Odin+SDL3

 人間の入力: pointer/key → hit test / keymap ─┐
 AI の入力:  ─────────────────────────────────┴─▶ (P2 ActionRequest + actor) ─▶ app
 IME:        host ──(P5 text-input)──▶ shell ──▶ ReplaceSelectedText 等の action に変換
```

- **意味論的な入力**: 人間もクリックをそのまま送らない。hit test でノードを特定し、`click(node)` に変換してアプリに渡す。AI は最初から action を直接送る。経路が同じなので、権限・監査・プレビューを一か所で扱える。
- **AI は IME を通らない**: AI はテキストを `set-value` / `replace-selected-text` で直接入れる。IME は人間専用の経路でよい。

## 不変条件 (LLM を「スタイリスト」に留める)

1. template は既存のノード ID / role パターンしか参照できない。**action を新しく作れない**
2. actionable なノードはすべて view に出す。ユーザーが明示的に隠した場合は除く (カバレッジ検査)
3. フォーカス順と読み上げ順は、tree の順か、tree から導いた順にする
4. 権限は tree のノード/action 側に付く。view をどう生成しても権限は変わらない

→ LLM が変な UI を出しても、できることは増えない。最悪でも「見た目が悪い」で済む。

## キャッシュ

- キーは **構造シグネチャ**。`hash(app_id, catalog_version, normalize(subtree))` で、normalize は次のとおり:
  - role と action の集合、子のシグネチャを再帰的に使う
  - 値・ラベル文字列は**含めない** (データが変わってもヒットさせるため)
  - 同じシグネチャの子が繰り返す場合は `list-of(sig)` に畳む (件数が違ってもヒットさせるため)
- エントリの状態は 3 つ:
  - `ephemeral`: LLM が出したもの
  - `pinned`: ユーザーが「これが好み」と承認したもの
  - `rejected`: 再生成させたもの
- ヒットすれば LLM は呼ばない。④の展開は決定的に行う。
- **ヒットしても、展開のたびに不変条件を確かめ直す**。`list-of` に畳んだので、件数の違う木に同じ template が当たる。template は子の id を名指しせず、`each` / `children` で今の子をすべてたどる形しか書けない (catalog の語彙)。そのうえで展開時に「操作できるノードが view に 1 回ずつ現れる」(不変条件 2) を毎回検査し、破れたらその template を捨てて既定レンダラに戻す (`expand.ts`)

---

## 各プロトコル

**ワイヤ形式は最初は JSON Lines**。トランスポートはインプロセス呼び出し / `postMessage` / WebSocket / stdio のどれでもよい。理由は次のとおり:

- canvas(TS) / Rust (wgpu, gpui) / Odin の全試作で、同じ fixture とログを使い回せる。ログはそのまま再生テストになる
- LLM がそのまま読み書きできる
- WIT には型が固まってから写す (README の `wit/shell.wit` はその前段のスケッチ)

### 語彙: WAI-ARIA を基準にし、AccessKit は変換先にする

- **role と state は WAI-ARIA の名前を使う** (`button`, `checkbox`, `textbox`, `list`, `listitem` / `checked`, `expanded`, `disabled` ...)。ARIA は Web 標準で、LLM が最もよく知っている語彙なので、UI 生成やツリーの読解の精度が上がる
- **action は ARIA に定義がない**。そのため AccessKit の `Action` の名前を camelCase で借りる (`click`, `focus`, `blur`, `setValue`, `replaceSelectedText`, `setTextSelection`, `increment`, `decrement`, `expand`, `collapse`, `scrollIntoView`, `showContextMenu`)
- ARIA にない概念は、少数の拡張 role として明示する (`x-window`, `x-canvas` など)。拡張は `x-` 接頭辞で区別する
- ワイヤ形式は自前の単純な JSON にする。AccessKit は Rust backend の**外側に付けるアダプタ**としてだけ使い、`TreeUpdate` / `ActionRequest` に変換して OS の a11y (NSAccessibility / AT-SPI / UIA) に出す。外しても全体は動く

| ワイヤ (ARIA 基準) | AccessKit | 備考 |
|---|---|---|
| `button` | `Role::Button` | |
| `checkbox` + `checked: true\|false\|"mixed"` | `Role::CheckBox` + `Toggled` | |
| `switch` | `Role::Switch` + `Toggled` | |
| `textbox` + `value` | `Role::TextInput` (複数行は `MultilineTextInput`) | `multiline: true` で区別する |
| `list` / `listitem` | `Role::List` / `Role::ListItem` | |
| `tablist` / `tab` / `tabpanel` | `Role::TabList` / `Tab` / `TabPanel` | |
| `dialog` | `Role::Dialog` | |
| `x-window` | `Role::Window` | 拡張 |
| `x-canvas` | `Role::Canvas` | 拡張。ピクセル直描きの逃げ道 |
| `label` (accessible name) | `label` | ARIA の accname に相当 |
| `disabled` / `expanded` / `selected` | 同名のプロパティ | |

対応表は実装時に網羅する。ARIA の role で対応が曖昧なもの (`application` など) は使わない。

### P1: Semantic Tree (app → shell)

```json
{"t":"tree-update","app":"todo","focus":"n3",
 "nodes":[
  {"id":"root","role":"x-window","label":"Todo","children":["n1","n2"]},
  {"id":"n1","role":"list","label":"Tasks","children":["i1","i2"]},
  {"id":"i1","role":"checkbox","label":"牛乳を買う","checked":false,"actions":["click"]},
  {"id":"i2","role":"checkbox","label":"税金","checked":true,"actions":["click"],"sensitive":true},
  {"id":"n2","role":"textbox","label":"新しいタスク","value":"","actions":["focus","setValue","replaceSelectedText"]}
 ]}
```

- 差分で送る (送ったノードだけ置換)。`focus` は毎回送る (AccessKit と同じ規約)
- 独自拡張は `sensitive` だけにする。権限そのものはツリーに入れず、shell のポリシーに置く

### P2: ActionRequest (shell → app)

```json
{"t":"action","target":"i1","action":"click","by":"agent","proposal":"p42"}
{"t":"action","target":"n2","action":"replaceSelectedText","data":"卵","by":"human"}
```

- `by` (actor) と `proposal` (承認済み提案の ID) は、shell が付ける封筒 (envelope) である。アプリは `by` を見て拒否してもよい
- AI 側には MCP として `get_tree` / `propose(actions[])` / `wait(proposal)` を公開する

### P3: View Template (LLM / 既定レンダラ → shell)

json-render 風。カタログ (`stack`, `card`, `checkbox`, `textField`, `badge` ...) の範囲内で書かせる。

```json
{"t":"template","match":{"role":"list","children":{"role":"checkbox"}},
 "view":{"type":"card","title":{"bind":"label"},
         "body":{"type":"stack","axis":"vertical","gap":4,
                 "each":{"type":"checkbox","node":{"bind":"$child"}}}}}
```

- `bind` で参照できるのはノードのプロパティだけ。`node` で結びついた view 要素にだけ入力が流れる

### P4a: View Tree (shell → widget 系 backend)

template を展開し、レイアウト前の段階まで進めたツリー。**gpui や DOM は自分でレイアウトと描画をする**ので、この段階で渡す。各要素は元の a11y ノード ID を持ち、イベントは P2 に戻す。

### P4b: Display List (shell → 低レベル backend)

```json
{"t":"frame","ops":[
  ["rect",{"x":0,"y":0,"w":800,"h":600,"fill":"#1e1e2e"}],
  ["clip",{"x":16,"y":16,"w":400,"h":300}],
  ["text",{"x":24,"y":40,"str":"牛乳を買う","font":"ui","size":14,"fill":"#cdd6f4"}],
  ["pop"]]}
{"t":"measure","id":7,"str":"牛乳を買う","font":"ui","size":14}   // shell → backend
{"t":"measured","id":7,"w":70.5,"h":17}                             // backend → shell
```

- **テキストの shaping と描画は backend に委譲する** (IME と同じく最初は自前でやらない)。shell は `measure` で寸法だけ受け取ってレイアウトする
- hit test 用に、shell は「矩形 → ノード ID」の表を持つ

### P5: Text Input / IME (host ↔ shell)

Wayland の `text-input-v3` を手本にした最小の形。

```json
{"t":"ime-enable","node":"n2","surrounding":"","cursor":0,"anchor":0,"caret":{"x":120,"y":300,"w":1,"h":18}}  // shell → host
{"t":"ime-preedit","text":"たまご","cursorBegin":9,"cursorEnd":9}   // host → shell (表示だけ、tree には入れない)
{"t":"ime-commit","text":"卵"}                                      // host → shell → P2 replaceSelectedText
{"t":"ime-disable"}
```

- **確定文字列だけを action にする**。preedit は view の一時状態で、tree にも AI にも見せない
- **未対応: キャレットの前後を消す操作**。`text-input-v3` の `delete_surrounding_text` に当たるものがまだ無い (IME が直前の文字を消して確定し直す場合)。足すときは `{"t":"ime-delete-surrounding","before":n,"after":m}` (文字数は Unicode のコードポイント) とし、同じ確定の組の中では「消す → commit」の順に当てる。P2 では `setValue` (消したあとの値) に写す
- `caret` を送るのは、変換候補ウィンドウを OS が正しい位置に出すため

---

## backend ごとの IME 委譲と a11y

| backend | 窓 / 描画 | IME の委譲先 | テキスト | OS a11y へ出すには |
|---|---|---|---|---|
| **Canvas (TS)** | `<canvas>` 2D | 隠し `<textarea>` を caret 位置に置く (全ブラウザで動く)。EditContext は Chromium 限定の拡張 | `measureText` / `fillText` | ARIA 付きの不可視 DOM を並べる (Flutter web 方式) |
| **Odin + SDL3** | SDL3 renderer か自前ラスタ | SDL の text input (`SDL_StartTextInput`, `TEXT_EDITING` / `TEXT_INPUT` イベント, `SDL_SetTextInputArea`) | SDL3_ttf か stb_truetype | accesskit-c を FFI で (後回し) |
| **wgpu** | winit + wgpu | winit の `Ime` イベント (`Preedit` / `Commit`) と `set_ime_cursor_area` | cosmic-text / glyphon / parley | `accesskit_winit` |
| **gpui** | gpui に任せる (P4a で渡す) | gpui の InputHandler | gpui 内蔵 | AccessKit 統合 (fork では実装済み。上流の状況は要確認) |

## 試作の順番

1. **Canvas (TS)**: P1–P5 を最短で通し、LLM での template 生成とキャッシュを回す。本題の UX 検証はここでやる
2. **Odin + SDL3**: 同じ JSON ログを再生して、プロトコルが backend 非依存かを確かめる。mac ネイティブで IME が委譲できることも確認する
3. **wgpu (winit + accesskit_winit)**: OS a11y への露出を確認する
4. **gpui**: widget 系 backend (P4a) として比較する

## veriss 統合で変わる予定の点

詳細は [VERISS.md](./VERISS.md)。

- P1: 動的な文字列に長さの契約 (`maxLength` など) を足す。veriss が文字の幅の上界を出すのに使う
- P4b: `measure` を backend から shell に移す。証明と実行時で同じ font・同じ計測値を使うため
- P1 の最初の更新は `root` を持つ。不要になったノードは `remove` で消す (試作の `src/protocol.ts` に合わせた)
