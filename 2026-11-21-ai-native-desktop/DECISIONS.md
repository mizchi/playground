# 決めたこと

この実験で決めた方針を、日付・判断・理由の順に残す。検証の結果は [VALIDATION.md](./VALIDATION.md) にある。

## 2026-11-22: Odin の描画 backend は wgpu を主に使う

**判断**: Odin で描画 backend を作るときは、`vendor:wgpu` (GPU) を主に使う。

- 対象: `proto-canvas/backend-odin-gpu/` (`?backend=odin-gpu`)
- 環境ごとの使い分け: ブラウザでは WebGPU、ネイティブ (mac の単体アプリ、Linux) では wgpu-native を使う

ソフトウェア描画の版 (`proto-canvas/backend-odin/`、`?backend=odin`) は、主には使わない。次の用途に限って残す。

- GPU 版の見た目を比べる基準として使う。同じ符号付き距離の式で描くので、画素の差を比べられる
- WebGPU や GPU が使えない環境での代わりにする

**理由** (VALIDATION.md の 4a と 4a')

- **CPU の負担**: 1 フレームの CPU 時間は、ソフトウェア描画が 9.9ms、wgpu が 0.13ms だった (1300×900、デバイスピクセル比 1)
  - ソフトウェア描画は画素数に比例して重くなり、Retina では約 4 倍になって 16ms を超える
  - wgpu の CPU 側の時間は、描画命令の数で決まる (Todo・Settings・Notes の画面は、四角形 162 個を draw call 1 回で描く)
- **見た目と計測が同じ**
  - GPU から読み戻した画素は、Canvas 2D と 0.58% しか違わない
  - 文字幅は、veriss の計測 (mizchi/font) と差 0
- **一つのコードで両方に届く**: 同じ Odin のコードが、ブラウザ (WebGPU) と mac の単体アプリ (wgpu-native の Metal) の両方で動く。「GUI 層を抽象化し、mac でも単体のデスクトップアプリで動かす」という当初の方針に合う

**まだ確かめていないこと** ([human_checks.md](./human_checks.md) の H11)

- 実際の GPU のブラウザで、canvas に正しく表示されるか。ヘッドレスの環境では、オフスクリーンに描いて読み戻した画素しか確かめられていない
- mac のネイティブ (wgpu-native) で動くか。wgpu-native の配布物は、この cloud 環境からは取れなかった

**この判断に伴う課題**

- 表面の形式は、端末が推奨する形式 (この環境では `rgba8unorm`) を問い合わせて使う。今は BGRA8 で設定しているので、コピーが 1 回増える
- Odin の `wgpu.js` の 2 つの癖を前提にしている (どちらも回避済み)
  - 表面のテクスチャの状態を書き込まない
  - `BufferMapAsync` が、失敗のあとに成功も通知する
- IME と入力は、引き続き web-canvas backend (ブラウザ) が受け持つ。ネイティブでは、SDL3 などの窓の層から受ける必要がある
