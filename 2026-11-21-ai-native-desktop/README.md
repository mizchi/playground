# 2026-11-21-ai-native-desktop

AI と人間が同じ GUI 環境を安全に共同操作するための「AI ネイティブなデスクトップ」を考察する実験。

- **目的**: Linux デスクトップの構成要素を理解したうえで、Canvas / wasm / Odin で UI 体験のプロトタイプを作る
- **状態**: 設計メモ + Canvas の試作 ([`proto-canvas/`](./proto-canvas/README.md))。`wit/shell.wit` は未検証のスケッチ
- **プロトコル**: a11y ツリーを正とするパイプラインとワイヤ形式は [`PROTOCOL.md`](./PROTOCOL.md)。こちらが最新で、WIT はこれが固まってから追従させる
- **安全なレイアウト**: mizchi/veriss で、生成された view をタイルの幅の範囲全体で検証する設計は [`VERISS.md`](./VERISS.md)
- **翻訳** (未実装の設計メモ): [`TRANSLATION.md`](./TRANSLATION.md)
- **検証の計画と結果**: [`VALIDATION.md`](./VALIDATION.md)
- **方針**: GUI 層は抽象化し、ブラウザ Canvas と macOS 単体アプリで先に検証。Linux 実機コンポジタは最後

---

## 1. Linux デスクトップの構成 (下から順に)

```
┌───────────────────────────────────────────────────────────┐
│ アプリ (Firefox, Terminal...)  / ツールキット (GTK, Qt)      │
├───────────────────────────────────────────────────────────┤
│ デスクトップ周辺: D-Bus, xdg-desktop-portal, PipeWire,      │
│                 AT-SPI2 (アクセシビリティツリー)              │
├───────────────────────────────────────────────────────────┤
│ ディスプレイサーバ                                            │
│   X11:     Xorg + WM(i3等) + compositor(picom) が別プロセス   │
│   Wayland: compositor 1 つ (Hyprland, sway, GNOME の Mutter) │
├───────────────────────────────────────────────────────────┤
│ ユーザ空間の下回り: Mesa(GL/Vulkan), GBM/EGL, libinput,       │
│                    libseat/logind                         │
├───────────────────────────────────────────────────────────┤
│ カーネル: DRM/KMS (画面出力), evdev (入力), GPU ドライバ       │
└───────────────────────────────────────────────────────────┘
```

| 部品 | 役割 |
|---|---|
| **DRM/KMS** | カーネルの画面出力 API。解像度設定 (mode setting) と「このバッファをこの画面に出す」(page flip) を行う。コンポジタの最終出力先 |
| **evdev** | カーネルの入力デバイス API (`/dev/input/event*`) |
| **libinput** | evdev の生イベントを、ポインタ加速・タッチパッドのジェスチャなどに正規化するライブラリ |
| **libseat / logind** | 「seat」(画面+キーボード+マウスの一組) の管理。一般ユーザーのプロセスに DRM / 入力デバイスを開く権限を渡す。VT 切り替えも担う |
| **Mesa / GBM / EGL** | GPU の OpenGL/Vulkan 実装と、GPU バッファの確保・共有 (dmabuf) |
| **PipeWire** | 音声と映像ストリーム (画面共有) の配送 |
| **xdg-desktop-portal** | サンドボックスアプリが「スクショ」「画面共有」「ファイル選択」などをユーザーの承認付きで頼む D-Bus API |
| **AT-SPI2** | スクリーンリーダー向けの UI ツリー (D-Bus)。macOS の AX API に相当 |

### X11 (旧来)

- **X server (Xorg)** が画面と入力を所有し、アプリ (クライアント) はソケット越しに「描いて」と頼む。
- **ウィンドウマネージャ (WM)** は「他のウィンドウの位置を決める特権的なクライアント」にすぎない。i3, dwm, xmonad はこれ。合成 (透過・影) はさらに別プロセスの compositor。
- **セキュリティモデルがほぼない**: どのクライアントも他ウィンドウの画面を読め、キー入力を盗聴でき、XTEST 拡張で入力を注入できる (`xdotool`)。
  → AI に自動操作させるのは簡単だが、権限制御はできない。

### Wayland (現行)

- Wayland は**プロトコル**(XML で定義された、Unix ソケット上のオブジェクト指向メッセージ)。「Wayland サーバ」という単体のプログラムはない。
- **コンポジタ** = ディスプレイサーバ + WM + compositor が 1 プロセスにまとまったもの。Hyprland も sway も GNOME (Mutter) も KDE (KWin) も、それぞれが「Wayland の実装」。
- 描画の流れ: アプリは自分でバッファ (共有メモリまたは GPU の dmabuf) に描き、`wl_surface` に attach → commit する。コンポジタはそれらを合成して KMS に出す。コンポジタはアプリのバッファの中身を解釈しない。
- **隔離が前提**: アプリは他のウィンドウを見られず、入力を注入できず、自分の位置すら決められない。必要な機能は拡張プロトコルで個別に許可する:
  - `xdg-shell`: 通常のウィンドウ / `wlr-layer-shell`: バーや壁紙
  - `ext-foreign-toplevel-list`: ウィンドウ一覧の取得
  - `ext-image-copy-capture` / `wlr-screencopy`: 画面キャプチャ
  - `virtual-keyboard` / `wlr-virtual-pointer` / **libei**: 入力の注入
  - portal の RemoteDesktop / ScreenCast: ユーザーの承認付きで上記を行う
- **XWayland**: Wayland 上で X11 アプリを動かす互換レイヤ。
- コンポジタを作るためのライブラリ: **wlroots** (C, sway/river/labwc が使用)、**Smithay** (Rust, niri/COSMIC が使用)。

### Hyprland / sway / niri とは

いずれも **タイル型の Wayland コンポジタ**。Wayland では「タイル型 WM」はコンポジタそのものになる。

| | 特徴 | 外部から操作する IPC |
|---|---|---|
| **Hyprland** | C++ 製。アニメーションが豊富な動的タイル型。wlroots から独立し、低レイヤを自前ライブラリの aquamarine に置き換えた | `hyprctl` (Unix ソケット) + イベント用ソケット |
| **sway** | i3 互換。wlroots の参照実装的な存在 | `swaymsg` (i3 互換の JSON IPC) |
| **niri** | Rust/Smithay 製。横スクロール型のタイル | `niri msg` (JSON) |

**ポイント**: こうした IPC で「ウィンドウ一覧」「フォーカス移動」「ワークスペース移動」は AI から JSON で操作できる。だが**ウィンドウの中身 (ボタンなど) には触れない**。中身はピクセル + AT-SPI でしか見えない。

### macOS との対応

- **WindowServer** が唯一のコンポジタで、置き換えられない。
- **AeroSpace / yabai / Amethyst** は、Accessibility API (`AXUIElement`) で他アプリのウィンドウを外から動かす「後付け」のタイル管理。AeroSpace は i3 風で、仮想ワークスペースは「ウィンドウを画面外に隠す」ことで実現している。CLI があるので AI からも叩きやすい。
- 現状の computer use は「スクショ + 合成入力 (CGEvent)」。権限は「画面収録」と「アクセシビリティ」の全部許可か拒否かの二択で、細かく分けられない。

### QEMU などの仮想化

- **QEMU**: CPU/デバイスのエミュレータで、仮想マシンモニタでもある。**KVM** (Linux) や **HVF** (macOS) と組み合わせるとハードウェア仮想化で高速に動く。
- **virtio-gpu** (+ virgl / venus) でゲストに仮想 GPU を見せる。表示は SPICE / VNC / GTK / Cocoa ウィンドウ。
- Apple Silicon の mac では **UTM** (QEMU の GUI フロントエンド) で aarch64 Linux を動かすのが手軽。
- 自作コンポジタの開発での使い道: **DRM/KMS を直接触るコンポジタを、実機を壊さずに試す**ため。
- 実際の開発の順序は次のとおり:
  1. **ネスト実行**: コンポジタを既存のデスクトップ上の 1 ウィンドウとして動かす (wlroots の wayland/x11 backend、Smithay の winit backend)
  2. **headless**: 画面なしで動かす。CI や AI の自動テストに向く
  3. VM (QEMU) で DRM/KMS
  4. 実機

---

## 2. 問題の整理: なぜ今の GUI は AI 操作と噛み合わないか

1. **ピクセルが唯一の真実**: アプリは描いた後、意味情報を捨てる。a11y ツリーは後付けで、内容も不完全。
2. **操作の主体が区別されない**: 人間と AI が同じマウス/キーボードを奪い合う。アプリは誰が押したかを知らない。
3. **権限が粗い**: 「画面全体を読む」「全入力を注入する」の二択。「このウィンドウのこのボタンだけ」とは言えない。
4. **プレビューと取り消しの仕組みがない**: 操作は即時に副作用を起こす。人間が事前に確認する段階がない。

X11 は (1)〜(4) すべてが緩く、Wayland は (3) を閉じたが代替の意味 API がない。どちらも AI を前提にしていない。

---

## 3. 提案アーキテクチャ

```
          人間 (マウス/キーボード)          AI エージェント (MCP 等)
                    │                                │
                    ▼                                ▼
┌──────────────────────────────────────────────────────────────┐
│ Shell (自作)                                                 │
│  ・Input Router: 入力に actor (human / agent) を付ける          │
│  ・Policy Engine: capability 判定、承認待ち、監査ログ、undo     │
│  ・Layout Engine: タイルツリー (i3/AeroSpace 風)、ワークスペース │
│  ・UI Tree Store: 全アプリの宣言的 UI ツリー (= AI の観測対象)  │
│  ・Renderer: UI ツリー → ピクセル                               │
│  ・Preview: AI の提案操作をゴースト表示/シャドウ実行            │
├──────────────────────────────────────────────────────────────┤
│ Component Host (wasmtime / ブラウザでは jco)                   │
│   app.wasm  app.wasm  generated-ui (json-render 風)  ...      │
├──────────────────────────────────────────────────────────────┤
│ PAL (Platform Abstraction Layer)                             │
│   web-canvas │ macOS (SDL3/Metal) │ linux-nested │ linux-drm  │
└──────────────────────────────────────────────────────────────┘
```

### 設計原則

- **UI は宣言的なツリーで渡す**: アプリはピクセルではなく UI ツリー (の差分) を shell に送る。shell は同じツリーを**人間向けに描画し、AI にも公開する**。AI 用と人間用で表現が分かれない。ピクセル描画は `canvas` ノードという逃げ道として残す。
- **actor を第一級にする**: すべての入力・アクションに「人間か AI か」を付ける。アプリも shell も、それを見て判断できる。
- **capability = WIT の import**: コンポーネントは import したものしか呼べない。アプリの権限はこれで決まる。AI への権限は「(アプリ, ノード, アクション種別) × 期間/回数」の単位で付与する。
- **propose → preview → approve → commit**: AI の操作はまず提案として出させる。人間はゴーストのオーバーレイか、AI 専用ワークスペースでのシャドウ実行で結果を確認してから確定する。危険度の低いものはポリシーで自動承認する。
- **AI 専用ワークスペース**: AI は人間のフォーカスを奪わず、自分のワークスペースで作業する。人間はそれをサムネイルや PiP で眺める。AeroSpace / Hyprland のワークスペース概念を AI と人間で共有するイメージ。
- **生成 UI も同じ土俵に載せる**: json-render のように、AI は「カタログ (許可されたコンポーネントのスキーマ)」の範囲内で JSON の UI を出す。それを同じ UI ツリー形式に流し込めば、生成 UI にも人間のアプリと同じ権限・操作 API・監査が効く。

### 既存 Linux アプリの扱い

Firefox などを動かすには Wayland プロトコルのサーバ実装 (= 本物のコンポジタ) が必要で、これは大仕事になる。そこで既存アプリは「レガシー枠」とし、ピクセル + AT-SPI で扱う。新しいアプリは wasm コンポーネントとして作る。二層構えにするのが現実的。

---

## 4. PAL (GUI 層の抽象)

最小インタフェース:

```
create_window(w, h) / present(rgba_buffer | gpu_frame) / poll_events() -> []Event
clipboard_get/set / now() / request_frame()
```

- 最初は**ソフトウェアレンダラ**(RGBA バッファに CPU で描く) にする。全バックエンドが「バッファを blit するだけ」で済み、移植が楽。GPU 対応は後で足す。
- バックエンド:
  - `web-canvas`: wasm から `ImageData` を `putImageData`。後で WebGPU
  - `macos`: Odin の `vendor:sdl3` が最短。本格的にやるなら Cocoa + Metal (`vendor:darwin`)
  - `linux-nested`: SDL3 か Wayland クライアントとして
  - `linux-drm`: 将来、自分がコンポジタになる段階

---

## 5. wasm / WASI / WIT

- **core wasm** は数値しかやり取りできない。**Component Model** が文字列・リスト・record・variant・resource といった型付きのインタフェースを足す。
- **WIT** はそのインタフェース定義言語。`world` は「import するもの + export するもの」の組で、コンポーネントの契約になる。import していない機能は呼べないので、**構造的に capability-based security になる**。
- **WASI** は WIT で定義された標準インタフェース群 (filesystem, clocks, sockets, http...)。
  - 0.2 (2024): component model ベース
  - 0.3 (2026-06 ratified): `stream<T>` / `future<T>` / `async func` が component model にネイティブで入った。`wasi:io` は廃止。Wasmtime 46 からデフォルトで有効
- 自作 GUI は **独自 package (`gui:shell`) の world** として定義する (`wit/shell.wit`)。filesystem などは標準の WASI を選んで import させる (例: preopen したディレクトリだけ)。
- 参考: **wasi-gfx** (`wasi:webgpu`, `wasi:surface`, `wasi:frame-buffer`) は WASI に GPU/画面を持ち込む提案。canvas ノード部分の参考になる。
- ホスト:
  - **wasmtime**: Rust API が最も充実している。C API にも component の API が入ったが、resource / async がどこまで使えるかは `crates/c-api/include/wasmtime/component/` のヘッダで要確認
  - **jco**: ブラウザ / Node 向けに component を JS + core wasm に変換する。同じコンポーネントをブラウザでも動かせる

---

## 6. Odin の位置づけ

- 得意なこと: ネイティブのホスト、レンダラ、レイアウトエンジン。`vendor` に SDL2/SDL3、raylib、wgpu、Metal などのバインディングがある。ターゲットに `js_wasm32` / `wasi_wasm32` / `freestanding_wasm32` がある。
- 苦手なこと: **Component Model のゲスト側**。`wit-bindgen` は Odin に対応していないので、canonical ABI のグルーを手書きする必要がある。
- 推奨する分担:
  - **Odin**: shell core (layout + renderer + PAL)。ネイティブと `js_wasm32` の両方でビルドする
  - **component host**: ネイティブは wasmtime (Odin から C API を使う。不足があれば Rust の薄いホストを挟む)、ブラウザは jco
  - **アプリ (ゲスト)**: wit-bindgen 対応言語 (Rust, C, Go, JS, MoonBit など)。言語ではなく WIT が契約になる
- 早めに検証すべきリスク: wasmtime C API の component / resource サポートが Odin から実用になるか (spike で確認)

---

## 7. ロードマップ案

| Phase | 内容 | 学べること |
|---|---|---|
| 0 | UTM/QEMU で Linux VM を立て、sway / Hyprland を触る。`swaymsg -t get_tree` / `hyprctl -j clients` を AI から叩く。mac では AeroSpace CLI | コンポジタ IPC の粒度と限界 |
| 1 | ブラウザ Canvas 上の疑似デスクトップ: Odin (`js_wasm32`) でタイルレイアウト + ソフトウェアレンダラ。アプリはダミー | layout / render / PAL |
| 2 | `wit/shell.wit` を固め、jco でコンポーネントアプリをブラウザで動かす。agent API を MCP として公開し、preview / approve UI を作る | 権限・プレビューの UX (本題) |
| 3 | json-render 風のカタログで AI に UI を生成させ、同じツリーに流す | 生成 UI と権限の統合 |
| 4 | macOS 単体アプリ (Odin + SDL3 + wasmtime) | ネイティブホスト |
| 5 | Linux: nested → headless → VM の DRM/KMS | 本物のコンポジタ |

Phase 2 が本題なので、Phase 1 は最小限で切り上げる。

## 参考

- Wayland: https://wayland.freedesktop.org/ / Wayland Book: https://wayland-book.com/
- Hyprland の wlroots 離脱: https://hypr.land/news/independentHyprland
- WASI 0.3: https://bytecodealliance.org/articles/WASI-0.3
- Component Model: https://component-model.bytecodealliance.org/
- json-render: https://github.com/vercel-labs/json-render
