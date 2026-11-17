//! crustc のドライバ。driver2 (Session2) が解析・check (LSP の診断)・キャッシュと、feature `build` (既定) のときは
//! ワークスペースのビルド (並列化・機械語・リンク) をまとめる。
//! 層: base → syntax → sema → typeck (型の検査・命令の記録) → codegen (機械語・リンク: build のときだけ) → core (ここ)。
//! LSP は crustc-lsp (build なしで使う)、Cranelift のバックエンドは crustc-clif。下の層のモジュールはここからも
//! `crustc_core::lexer` のように引ける

pub use crustc_typeck::*;
#[cfg(feature = "build")]
pub use crustc_codegen::{link, lower, minclif};

pub mod driver2;
#[doc(hidden)]
pub mod testgen;
