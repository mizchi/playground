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
| P5 IME | `src/backend/canvas.ts` | 隠し textarea に OS の IME を任せる。preedit は表示だけで木に入れず、確定文字列を `replaceSelectedText` にする |
| タイル | `src/shell.ts` `frame` | 2 枚の横分割。境界をドラッグできる。最小幅を下回ると「はみ出し」を出す |

右のパネルでは、次のことができる。

- agent として提案する (ゴースト表示 → 承認 / 却下)
- template を貼って適用・pin・reject する (LLM の出力の代わり)
- agent から見える意味ツリーと監査ログを見る

ブラウザのコンソールからは `agent.getTree()` と `agent.propose(intent, steps)` が使える。どちらも MCP の tool にそのまま写せる形にしてある。

## 動かし方

```sh
pnpm install
pnpm dev          # http://127.0.0.1:8123
pnpm test         # 単体テスト (node --test、型ストリップで .ts をそのまま実行)
pnpm typecheck
pnpm build && pnpm e2e   # Chromium で通し確認。--shots で screenshots/ に保存
```

## 結果

- 単体テスト 10 件と E2E 11 件が通る (Node 22、Playwright の Chromium)
- E2E の IME は、CDP の `Input.imeSetComposition` で実際の composition イベントを起こして確かめた。preedit は木に入らず、確定した `卵` だけが値になる
- LLM の template の代わりに「カード」template を適用すると、件数が変わっても (3 件 → 5 件) 同じキャッシュに当たる。pin すると再読み込み後も残る
- 操作を落とす template は、既定レンダラに戻り、理由がパネルに出る
- agent の直接操作 (承認なし) は拒否される。sensitive な提案は自動承認されない。Settings アプリは、承認済みでも agent からの全削除を自分で拒否する

## 簡略化していること

- textbox のキャレットは常に末尾にある (選択範囲がない)
- 文字はすべて 1 行で、折り返さない。幅は Canvas の `measureText` で測る (veriss と統合するときは、shell 側の font metrics に移す。[VERISS.md](../VERISS.md))
- 空の list は、件数のある list と別のシグネチャになる
- LLM はまだ繋いでいない。template はパネルに貼る
