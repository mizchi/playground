# 翻訳 (設計メモ。まだ実装しない)

あとで翻訳を組み込むときに、どこに入れるかと、何に気をつけるかを残しておく。

## どこに入れるか

a11y ツリーが正で、view はその射影である ([PROTOCOL.md](./PROTOCOL.md))。翻訳も、view と同じ「射影の一段」として扱う。

```
app ──(原文の label)──▶ Semantic Tree ──▶ 翻訳 (locale ごと) ──▶ 展開 (template の bind: label) ──▶ レイアウト・描画
                                │
                                └──▶ agent には原文 (と、必要なら翻訳) を渡す
```

- アプリは原文の label を出すだけで、翻訳を知らない
- shell が locale に応じて label を差し替える。差し替える場所は、`expand.ts` が `{bind: 'label'}` を解決するところ
- 構造シグネチャは label を含まない ([PROTOCOL.md](./PROTOCOL.md))。だから、翻訳しても template のキャッシュはそのまま当たる

## 翻訳の出どころ

優先順に、次の 3 つ。

1. アプリが同梱するメッセージカタログ
2. 翻訳キャッシュ。キーは `(原文, locale, role)`。役割で訳し分けるため、role も含める (ボタンの「保存」と見出しの「保存」など)
3. LLM。template と同じく、ephemeral → 人が確かめて pinned、の流れにする

## 気をつけること

- **送らないもの**
  - sensitive なノードの value は、LLM に送らない
  - 人が入力した値 (textbox の value) も翻訳しない。翻訳するのは label と、アプリ固定の文字列だけ
- **レイアウトの安全性 (veriss)**: 訳すと文字列の長さが変わる。すると文字の契約 (`maxInlineSize`) が変わり、窓の最小幅も変わる。
  - 証明は locale ごとに持つ。キャッシュのキーと証明の記録に locale を含める
  - タイル配置の検証 (最小幅を下回る分割の拒否) も、locale ごとの最小幅で行う。例: ドイツ語では窓が広く要る
  - veriss の CJK の行分割は言語に依存する (`text.lang`)。翻訳後の lang を、文字の契約に渡す
- **書字方向**: veriss は `horizontal-tb` / `ltr` だけを扱う。RTL (アラビア語など) の配置は Unknown になる。対応範囲の外として明記する
- **人が見る文**: agent の intent や提案の説明も、人がプレビューで読む文なので翻訳の対象になる。監査ログには原文を残す
- **入力**: IME の入力は翻訳と関係ない (P5 はそのまま)

## 段階

| 段階 | 内容 |
|---|---|
| T0 | メッセージカタログと、`expand.ts` での label の差し替え |
| T1 | LLM による翻訳とキャッシュ (ephemeral / pinned) |
| T2 | locale ごとの veriss の検証と、locale ごとの最小幅によるタイル配置 |
