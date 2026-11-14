//! exrustc: インクリメンタル開発ビルド特化の小さな Rust (サブセット) コンパイラ。
//! CLI (`src/main.rs`)・LSP (`lsp`)・ベンチ (`benches/`) はすべて [`Session`] を通る。

pub mod cache;
pub mod codegen;
pub mod consteval;
pub mod driver;
pub mod enc;
pub mod front;
pub mod fxhash;
pub mod infer;
pub mod lexer;
pub mod link;
pub mod lsp;
pub mod prelude;
#[doc(hidden)]
pub mod testgen;

pub use driver::{BuildReport, Diagnostic, Options, Session, cache_path};
