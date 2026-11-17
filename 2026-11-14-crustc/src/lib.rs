//! crustc: インクリメンタル開発ビルド特化の小さな Rust (サブセット) コンパイラ。
//! 中身は crates/ の crate に分かれている (このクレートは CLI と、それらの再輸出):
//! - `crustc-base`: 部品 (FxHash の表・キャッシュのキー・直列化)
//! - `crustc-syntax`: 字句・構文木・前処理 (cfg / マクロ / `mod` / 入れ子のアイテム)・アイテムの要約・prelude の選択
//! - `crustc-sema`: Program (名前・型・トレイトの表)・型推論
//! - `crustc-codegen`: acg (コード生成と型の検査)・Blob の直列化・リンク
//! - `crustc-core`: driver2 (キャッシュ・並列化・ワークスペース・check)。下の層を公開し直す
//! - `minclif`: Cranelift に似た API の小さな x86-64 バックエンド (外部 crate なし)
//! - `crustc-clif`: driver2 の Cranelift バックエンド (既定)
//! (base から core までは syn・Cranelift に依存せず、crustc 自身でもコンパイルできる)
//! LSP サーバ (crustc-lsp、エディタ向けの解析 crustc-ide を使う) は別のバイナリで、このクレートは依存しない

pub use crustc_core::*;
pub use crustc_clif as clif_backend;
pub use crustc_core::driver2::{Diagnostic, Options2, Report2, Session2};
