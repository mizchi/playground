//! 最初の crustc のドライバ: syn で読み、Cranelift (cranelift-module / object) でコード生成する。
//! 今の主経路は crustc-core の driver2 (+ crustc-clif)。こちらは `crustc check` / LSP と、driver2 の結果を比べる基準に使う。
//! 共有するモジュール (lexer / prelude / program / types など) は crustc-core のものをそのまま使う

pub use crustc_core::*;

pub mod cache;
pub mod closure;
pub mod codegen;
pub mod consteval;
pub mod driver;
pub mod elfz;
pub mod front;
pub mod infer;
pub mod lsp;

pub use driver::{BuildReport, Diagnostic, Options, Session, cache_path};
