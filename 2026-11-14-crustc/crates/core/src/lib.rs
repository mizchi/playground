//! crustc のドライバ。driver2 (Session2) がキャッシュ・並列化・ワークスペースのビルドと check (LSP の診断) をまとめる。
//! 層: base (部品) → syntax (字句・構文・前処理・要約) → sema (Program・型推論) → codegen (acg・リンク) → core (ここ)。
//! LSP は crustc-lsp、Cranelift のバックエンドは crustc-clif。下の層のモジュールはここからも `crustc_core::lexer` のように引ける

pub use crustc_codegen::*;

pub mod driver2;
#[doc(hidden)]
pub mod testgen;
