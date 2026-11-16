//! crustc: インクリメンタル開発ビルド特化の小さな Rust (サブセット) コンパイラ。
//! CLI (`src/main.rs`)・LSP (`lsp`)・ベンチ (`benches/`) はすべて [`Session`] を通る。

pub mod cache;
pub mod closure;
pub mod codegen;
pub mod consteval;
pub mod ctors;
pub mod driver;
pub mod elfz;
pub mod enc;
pub mod front;
pub mod fxhash;
pub mod key;
pub mod infer;
pub mod solver;
pub mod lexer;
pub mod link;
pub mod lsp;
pub mod macros;
pub mod minclif;
pub mod parse;
pub mod ast_closure;
pub mod ast_types;
pub mod ast_summary;
pub mod ast_consteval;
pub mod ast_codegen;
pub mod ast_infer;
pub mod prelude;
pub mod program;
#[doc(hidden)]
pub mod testgen;
pub mod types;

pub use driver::{BuildReport, Diagnostic, Options, Session, cache_path};
